//! A driver DLL and a PC made of data, for tests. Every call is recorded, so a test can see what
//! was asked of the "driver" as well as what came back.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use super::asio::tests::{quadro, studio};
use super::{CallError, DeviceProperties, DriverApi, DriverHost, DriverInfo, Handle};

pub struct FakeDll {
    pub api_version: u32,
    pub accepts: bool,
    pub info: DriverInfo,
    /// The serial of each device the driver lists.
    pub serials: Vec<String>,
    pub rate: u32,
    /// `None`: the DLL does not export `GetASIOInstanceCount`.
    pub instance_count: Option<u32>,
    pub asio: Vec<u8>,
    /// Exports the DLL lacks.
    pub missing: Vec<&'static str>,
    /// Calls that answer this status instead.
    pub failing: HashMap<&'static str, u32>,
    pub calls: Mutex<Vec<String>>,
    pub open: Mutex<Vec<Handle>>,
}

impl FakeDll {
    /// The Quadro's driver as found on 2026-09-18: API 5.12, driver 5.68.0.
    pub fn quadro(serial: &str) -> Self {
        FakeDll {
            api_version: (5 << 16) | 12,
            accepts: true,
            info: DriverInfo { api_major: 5, api_minor: 12, driver_major: 5, driver_minor: 68, driver_sub: 0, flags: 0 },
            serials: vec![serial.into()],
            rate: 44100,
            instance_count: Some(1),
            asio: quadro(),
            missing: Vec::new(),
            failing: HashMap::new(),
            calls: Mutex::new(Vec::new()),
            open: Mutex::new(Vec::new()),
        }
    }

    /// The Studio+'s: API 5.7, driver 5.0.0, and no `GetASIOInstanceCount`.
    pub fn studio(serial: &str) -> Self {
        FakeDll {
            api_version: (5 << 16) | 7,
            info: DriverInfo { api_major: 5, api_minor: 7, driver_major: 5, driver_minor: 0, driver_sub: 0, flags: 0 },
            instance_count: None,
            asio: studio(),
            ..FakeDll::quadro(serial)
        }
    }

    fn call(&self, name: &'static str) -> Result<(), CallError> {
        self.calls.lock().unwrap().push(name.into());
        if self.missing.contains(&name) {
            return Err(CallError::Missing(name));
        }
        match self.failing.get(name) {
            Some(&code) => Err(CallError::Status { call: name, code, text: "TSTATUS_INVALID_PARAMETER".into() }),
            None => Ok(()),
        }
    }

    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

impl DriverApi for FakeDll {
    fn api_version(&self) -> Result<u32, CallError> {
        self.call("TUSBAUDIO_GetApiVersion").map(|()| self.api_version)
    }
    fn check_api_version(&self, _major: u32, _minor: u32) -> Result<bool, CallError> {
        self.call("TUSBAUDIO_CheckApiVersion").map(|()| self.accepts)
    }
    fn driver_info(&self) -> Result<DriverInfo, CallError> {
        self.call("TUSBAUDIO_GetDriverInfo").map(|()| self.info)
    }
    fn enumerate_devices(&self) -> Result<(), CallError> {
        self.call("TUSBAUDIO_EnumerateDevices")
    }
    fn device_count(&self) -> Result<u32, CallError> {
        self.call("TUSBAUDIO_GetDeviceCount").map(|()| self.serials.len() as u32)
    }
    fn open_device(&self, index: u32) -> Result<Handle, CallError> {
        self.call("TUSBAUDIO_OpenDeviceByIndex")?;
        let handle = 0x1000 + index as Handle;
        self.open.lock().unwrap().push(handle);
        Ok(handle)
    }
    fn close_device(&self, handle: Handle) {
        self.calls.lock().unwrap().push("TUSBAUDIO_CloseDevice".into());
        self.open.lock().unwrap().retain(|&h| h != handle);
    }
    fn device_properties(&self, handle: Handle) -> Result<DeviceProperties, CallError> {
        self.call("TUSBAUDIO_GetDeviceProperties")?;
        Ok(DeviceProperties { vid: 0x23e5, pid: 1, serial: self.serials[handle - 0x1000].clone(), product: "Zen".into() })
    }
    fn current_sample_rate(&self, _handle: Handle) -> Result<u32, CallError> {
        self.call("TUSBAUDIO_GetCurrentSampleRate").map(|()| self.rate)
    }
    fn asio_instance_count(&self) -> Option<Result<u32, CallError>> {
        let count = self.instance_count?;
        Some(self.call("TUSBAUDIO_GetASIOInstanceCount").map(|()| count))
    }
    fn asio_instance_info(&self, _index: u32) -> Result<Vec<u8>, CallError> {
        self.call("TUSBAUDIO_GetASIOInstanceInfo").map(|()| self.asio.clone())
    }
}

/// The PC: DLLs by path (or why each fails to load), and registry values.
#[derive(Default)]
pub struct FakePc {
    pub dlls: Vec<(PathBuf, Result<Arc<FakeDll>, String>)>,
    pub cannot_look: Option<String>,
    pub registry: HashMap<(String, String), u32>,
    pub registry_error: Option<String>,
    pub loads: AtomicUsize,
}

pub const QUADRO_DLL: &str = r"C:\Program Files\Antelope Audio\Zen Quadro Synergy Core USB Audio Driver\x64\Zen_Quadro_Synergy_Coreapi_x64.dll";
pub const STUDIO_DLL: &str = r"C:\Program Files\Antelope Audio\ZenStudioTB USB Audio Driver\W10_x64\ZenStudioTBapi_x64.dll";
pub const QUADRO_SAFE_MODE: &str = r"SYSTEM\CurrentControlSet\Services\Zen_Quadro_Synergy_Core\ParametersDriver\Settings\AsioInstance0";
pub const STUDIO_SAFE_MODE: &str = r"SYSTEM\CurrentControlSet\Services\ZenStudioTB\ParametersDriver\Settings";

impl FakePc {
    /// This PC as found on 2026-09-18: both drivers, Safe Mode on for both, each where its driver
    /// keeps it.
    pub fn both(quadro: Arc<FakeDll>, studio: Arc<FakeDll>) -> Self {
        let mut pc = FakePc::default();
        pc.dlls.push((QUADRO_DLL.into(), Ok(quadro)));
        pc.dlls.push((STUDIO_DLL.into(), Ok(studio)));
        pc.registry.insert((QUADRO_SAFE_MODE.into(), "AsioSafeMode".into()), 1);
        pc.registry.insert((STUDIO_SAFE_MODE.into(), "AsioSafeMode".into()), 1);
        pc
    }

    pub fn loads(&self) -> usize {
        self.loads.load(Ordering::SeqCst)
    }
}

impl DriverHost for FakePc {
    fn find_api_dlls(&self) -> Result<Vec<PathBuf>, String> {
        match &self.cannot_look {
            Some(why) => Err(why.clone()),
            None => Ok(self.dlls.iter().map(|(path, _)| path.clone()).collect()),
        }
    }
    fn load(&self, dll: &Path) -> Result<Arc<dyn DriverApi>, String> {
        self.loads.fetch_add(1, Ordering::SeqCst);
        let (_, found) = self.dlls.iter().find(|(path, _)| path == dll).expect("only listed DLLs are loaded");
        found.clone().map(|dll| dll as Arc<dyn DriverApi>)
    }
    fn registry_u32(&self, subkey: &str, name: &str) -> Result<Option<u32>, String> {
        if let Some(why) = &self.registry_error {
            return Err(why.clone());
        }
        Ok(self.registry.get(&(subkey.to_string(), name.to_string())).copied())
    }
}
