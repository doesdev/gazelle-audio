//! Which USB host controller an interface is on.
//!
//! This is not a curiosity. Phase 0 found, at the hardware, that two of these interfaces on one
//! host controller cannot both stream: a controller reserves isochronous bandwidth and endpoints,
//! and 16 in / 16 out plus 24 in / 24 out is more than one of them had. The second driver failed
//! at `createBuffers` with Windows saying "not enough USB resources", which is a message no one
//! could act on. Gazelle can see the cause: a device's parent chain names its controller.
//!
//! The walk is the configuration manager's: locate the device node by its instance id, then ask
//! for its parent over and over, reading each node's description, until a node that says it is a
//! host controller. The PC sits behind [`UsbTopology`] so the comparison above it is decided
//! against a device tree made of data.

use std::sync::Arc;

use serde::Serialize;

/// The host controller a device hangs off.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct UsbController {
    /// The controller's device instance id, which is what tells two of them apart.
    pub instance_id: String,
    /// Its description, as Windows names it.
    pub description: String,
}

/// The PC's device tree, as far as finding a device's host controller.
pub trait UsbTopology: Send + Sync {
    /// The host controller the interface with this vendor id, product id and serial is on.
    fn controller_of(&self, vid: u16, pid: u16, serial: Option<&str>) -> Result<UsbController, String>;
}

/// Whether a node's description or instance id says it is a host controller, which is where the
/// walk up the parent chain stops.
///
/// Two tests, because Windows names these things several ways: a description saying so ("USB
/// xHCI Compliant Host Controller", "USB4 Host Router"), or a node that is no longer on the USB
/// bus at all, which on every PC these run on is the PCI device the controller is.
pub fn is_host_controller(instance_id: &str, description: &str) -> bool {
    let described = description.to_ascii_lowercase();
    described.contains("host controller") || described.contains("host router") || instance_id.to_ascii_uppercase().starts_with(r"PCI\")
}

/// Whether two devices are on the same controller.
pub fn same_controller(a: &UsbController, b: &UsbController) -> bool {
    a.instance_id.eq_ignore_ascii_case(&b.instance_id)
}

/// This PC's device tree on Windows; elsewhere, one that knows of no controllers.
pub fn for_this_pc() -> Arc<dyn UsbTopology> {
    #[cfg(windows)]
    {
        Arc::new(windows::ThisPc)
    }
    #[cfg(not(windows))]
    {
        Arc::new(Elsewhere)
    }
}

#[cfg(not(windows))]
struct Elsewhere;

#[cfg(not(windows))]
impl UsbTopology for Elsewhere {
    fn controller_of(&self, _vid: u16, _pid: u16, _serial: Option<&str>) -> Result<UsbController, String> {
        Err("USB host controllers are only read on Windows".into())
    }
}

#[cfg(windows)]
pub mod windows {
    //! The configuration manager. Read only: nothing here enables, disables or restarts anything.

    use std::ptr::null_mut;

    use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
        CM_Get_Device_ID_ListW, CM_Get_Device_ID_List_SizeW, CM_Get_Device_IDW, CM_Get_DevNode_PropertyW, CM_Get_Parent,
        CM_Locate_DevNodeW, CM_GETIDLIST_FILTER_ENUMERATOR, CM_GETIDLIST_FILTER_PRESENT, CM_LOCATE_DEVNODE_NORMAL, CR_SUCCESS,
    };
    use windows_sys::Win32::Devices::Properties::DEVPKEY_Device_DeviceDesc;
    use windows_sys::Win32::Foundation::DEVPROPKEY;

    use super::{is_host_controller, UsbController, UsbTopology};

    /// How far up the parent chain the walk will go before giving up. A device is three or four
    /// nodes below its controller; anything past this is a tree that is not what we think it is.
    const MAX_DEPTH: usize = 16;

    pub struct ThisPc;

    impl UsbTopology for ThisPc {
        fn controller_of(&self, vid: u16, pid: u16, serial: Option<&str>) -> Result<UsbController, String> {
            let instance = instance_of(vid, pid, serial)?;
            controller_above(&instance)
        }
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn from_wide(chars: &[u16]) -> String {
        let len = chars.iter().position(|&c| c == 0).unwrap_or(chars.len());
        String::from_utf16_lossy(&chars[..len])
    }

    /// Every present device instance id under the `USB` enumerator whose id names this vendor and
    /// product. The filter is a prefix, so it is the enumerator plus the hardware id.
    fn instance_ids(filter: &str) -> Result<Vec<String>, String> {
        let filter = wide(filter);
        let flags = CM_GETIDLIST_FILTER_ENUMERATOR | CM_GETIDLIST_FILTER_PRESENT;
        let mut len = 0u32;
        let status = unsafe { CM_Get_Device_ID_List_SizeW(&mut len, filter.as_ptr(), flags) };
        if status != CR_SUCCESS {
            return Err(format!("the device list could not be sized ({status:#x})"));
        }
        let mut buffer = vec![0u16; len as usize + 2];
        let status = unsafe { CM_Get_Device_ID_ListW(filter.as_ptr(), buffer.as_mut_ptr(), buffer.len() as u32, flags) };
        if status != CR_SUCCESS {
            return Err(format!("the device list could not be read ({status:#x})"));
        }
        // A multi-string: each id NUL terminated, the list closed by an empty one.
        Ok(buffer
            .split(|&c| c == 0)
            .filter(|part| !part.is_empty())
            .map(String::from_utf16_lossy)
            .collect())
    }

    /// The device instance id of the interface with this vendor id, product id and serial.
    fn instance_of(vid: u16, pid: u16, serial: Option<&str>) -> Result<String, String> {
        let hardware = format!(r"USB\VID_{vid:04X}&PID_{pid:04X}");
        let ids = instance_ids(&hardware)?;
        if ids.is_empty() {
            return Err(format!("no device matching {hardware} is attached"));
        }
        // The serial, when the device has one, is the last part of its instance id. With no
        // serial there is one answer to give, and it is only unambiguous when there is one device.
        match serial.map(str::trim).filter(|s| !s.is_empty()) {
            Some(serial) => ids
                .into_iter()
                .find(|id| id.rsplit('\\').next().is_some_and(|tail| tail.eq_ignore_ascii_case(serial)))
                .ok_or_else(|| format!("no device matching {hardware} reports serial {serial}")),
            None if ids.len() == 1 => Ok(ids.into_iter().next().expect("one id")),
            None => Err(format!("{} devices match {hardware} and none was named by serial", ids.len())),
        }
    }

    fn property(node: u32, key: &DEVPROPKEY) -> Option<String> {
        let mut kind = 0u32;
        let mut len = 0u32;
        unsafe { CM_Get_DevNode_PropertyW(node, key, &mut kind, null_mut(), &mut len, 0) };
        if len == 0 {
            return None;
        }
        let mut buffer = vec![0u8; len as usize];
        let status = unsafe { CM_Get_DevNode_PropertyW(node, key, &mut kind, buffer.as_mut_ptr(), &mut len, 0) };
        if status != CR_SUCCESS {
            return None;
        }
        let chars: Vec<u16> = (0..buffer.len() / 2).map(|at| u16::from_le_bytes([buffer[2 * at], buffer[2 * at + 1]])).collect();
        Some(from_wide(&chars))
    }

    fn device_id(node: u32) -> String {
        let mut buffer = [0u16; 512];
        let status = unsafe { CM_Get_Device_IDW(node, buffer.as_mut_ptr(), buffer.len() as u32, 0) };
        if status == CR_SUCCESS {
            from_wide(&buffer)
        } else {
            String::new()
        }
    }

    /// Walk up from a device instance id until a node that says it is a host controller.
    fn controller_above(instance: &str) -> Result<UsbController, String> {
        let mut node = 0u32;
        let status = unsafe { CM_Locate_DevNodeW(&mut node, wide(instance).as_ptr(), CM_LOCATE_DEVNODE_NORMAL) };
        if status != CR_SUCCESS {
            return Err(format!("{instance} is not a device node on this PC ({status:#x})"));
        }
        for _ in 0..MAX_DEPTH {
            let mut parent = 0u32;
            let status = unsafe { CM_Get_Parent(&mut parent, node, 0) };
            if status != CR_SUCCESS {
                return Err(format!("the chain above {instance} stopped before a host controller ({status:#x})"));
            }
            let id = device_id(parent);
            let description = property(parent, &DEVPKEY_Device_DeviceDesc).unwrap_or_default();
            if is_host_controller(&id, &description) {
                return Ok(UsbController { instance_id: id, description });
            }
            node = parent;
        }
        Err(format!("no host controller was found above {instance}"))
    }
}

/// A device tree made of data, for tests: vendor id, product id and serial to controller.
#[derive(Default)]
pub struct FakeTopology {
    pub controllers: std::collections::BTreeMap<String, Result<UsbController, String>>,
}

impl FakeTopology {
    /// The key a lookup is made under.
    pub fn key(vid: u16, pid: u16, serial: Option<&str>) -> String {
        format!("{vid:04x}:{pid:04x}:{}", serial.unwrap_or_default())
    }

    pub fn on(mut self, vid: u16, pid: u16, serial: &str, instance_id: &str) -> Self {
        let controller = UsbController { instance_id: instance_id.into(), description: format!("{instance_id} Host Controller") };
        self.controllers.insert(Self::key(vid, pid, Some(serial)), Ok(controller));
        self
    }

    pub fn unknown(mut self, vid: u16, pid: u16, serial: &str, why: &str) -> Self {
        self.controllers.insert(Self::key(vid, pid, Some(serial)), Err(why.into()));
        self
    }
}

impl UsbTopology for FakeTopology {
    fn controller_of(&self, vid: u16, pid: u16, serial: Option<&str>) -> Result<UsbController, String> {
        self.controllers
            .get(&Self::key(vid, pid, serial))
            .cloned()
            .unwrap_or_else(|| Err("this device is not in the tree".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_node_is_a_host_controller_by_what_it_says_or_by_leaving_the_usb_bus() {
        assert!(is_host_controller(r"PCI\VEN_8086&DEV_15C1\3&11", "Intel(R) USB 3.20 eXtensible Host Controller"));
        assert!(is_host_controller(r"PCI\VEN_8086&DEV_15C0\3&11", "USB4 Host Router"), "a router counts");
        assert!(is_host_controller(r"PCI\VEN_1022&DEV_1234\0", ""), "a PCI node with no description still counts");
        assert!(!is_host_controller(r"USB\VID_23E5&PID_A2F9\1000000000001", "Zen Quadro Synergy Core"));
        assert!(!is_host_controller(r"USB\ROOT_HUB30\4&1", "USB Root Hub (USB 3.0)"), "a root hub is below the controller");
    }

    #[test]
    fn two_devices_are_on_one_controller_when_the_instance_ids_match() {
        let a = UsbController { instance_id: r"PCI\VEN_8086&DEV_15C1\3&11".into(), description: "one".into() };
        let b = UsbController { instance_id: r"pci\ven_8086&dev_15c1\3&11".into(), description: "one, shouted".into() };
        let c = UsbController { instance_id: r"PCI\VEN_8086&DEV_15C0\3&11".into(), description: "another".into() };
        assert!(same_controller(&a, &b));
        assert!(!same_controller(&a, &c));
    }

    #[test]
    fn the_fake_tree_answers_by_vendor_product_and_serial() {
        let tree = FakeTopology::default().on(0x23e5, 0xa2f9, "Q1", r"PCI\A").unknown(0x23e5, 0xa100, "S1", "unplugged");
        assert_eq!(tree.controller_of(0x23e5, 0xa2f9, Some("Q1")).unwrap().instance_id, r"PCI\A");
        assert_eq!(tree.controller_of(0x23e5, 0xa100, Some("S1")).unwrap_err(), "unplugged");
        assert!(tree.controller_of(0x23e5, 0xa2f9, Some("nobody")).is_err());
    }
}
