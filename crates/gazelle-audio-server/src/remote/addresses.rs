//! This PC's addresses on its networks, for the addresses a phone would type or scan.
//!
//! Read from Windows' own list of adapters (`GetAdaptersAddresses`), not by opening a socket:
//! finding them sends nothing and listens on nothing. Only IPv4 addresses on adapters that are up
//! are kept, and loopback and self-assigned (169.254.x.x) ones are left out, since a phone can
//! reach neither. The adapter Windows would use for the internet comes first (`GetBestInterface`
//! asks the routing table, again without sending anything): on a PC with virtual adapters as
//! well as its Wi-Fi or Ethernet, that is almost always the one the phone shares.

use std::net::{IpAddr, Ipv4Addr};

use serde::Serialize;

/// One address a phone could use.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LanAddress {
    pub ip: IpAddr,
    /// On the adapter Windows routes the internet through: the likeliest one to work.
    pub primary: bool,
}

/// Whether an address is worth offering to a phone.
pub fn offerable(ip: Ipv4Addr) -> bool {
    !ip.is_loopback() && !ip.is_link_local() && !ip.is_unspecified() && !ip.is_multicast() && !ip.is_broadcast()
}

/// Put the primary addresses first and drop repeats, keeping the order Windows gave otherwise.
pub fn arrange(found: Vec<(Ipv4Addr, bool)>) -> Vec<LanAddress> {
    let mut out: Vec<LanAddress> = Vec::new();
    for primary in [true, false] {
        for (ip, is_primary) in &found {
            if *is_primary == primary && offerable(*ip) && !out.iter().any(|a| a.ip == IpAddr::V4(*ip)) {
                out.push(LanAddress { ip: IpAddr::V4(*ip), primary });
            }
        }
    }
    out
}

/// This PC's addresses now. Read afresh each time: a laptop moves between networks.
#[cfg(windows)]
pub fn this_pc() -> Vec<LanAddress> {
    use std::ptr::null;
    use windows_sys::Win32::Foundation::{ERROR_BUFFER_OVERFLOW, NO_ERROR};
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GetAdaptersAddresses, GetBestInterface, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST,
        IF_TYPE_SOFTWARE_LOOPBACK, IP_ADAPTER_ADDRESSES_LH,
    };
    use windows_sys::Win32::NetworkManagement::Ndis::IfOperStatusUp;
    use windows_sys::Win32::Networking::WinSock::{AF_INET, SOCKADDR_IN};

    // Any public address will do: the routing table is asked which adapter would carry it, and
    // nothing is sent to it.
    let mut best = 0u32;
    let has_best = unsafe { GetBestInterface(u32::from_ne_bytes([1, 1, 1, 1]), &mut best) } == NO_ERROR;

    let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
    let mut size = 16 * 1024u32;
    let mut buffer: Vec<u64>;
    let mut tries = 0;
    loop {
        // u64s, so the buffer has the alignment the structures need.
        buffer = vec![0u64; (size as usize).div_ceil(8)];
        let result = unsafe { GetAdaptersAddresses(u32::from(AF_INET), flags, null(), buffer.as_mut_ptr().cast(), &mut size) };
        match result {
            NO_ERROR => break,
            ERROR_BUFFER_OVERFLOW if tries < 3 => tries += 1,
            _ => return Vec::new(),
        }
    }

    let mut found = Vec::new();
    let mut adapter = buffer.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
    // SAFETY: Windows filled the buffer with a linked list of adapters that lives inside it, and
    // the buffer outlives this walk. Each address is read only after its family is checked.
    unsafe {
        while !adapter.is_null() {
            let a = &*adapter;
            if a.OperStatus == IfOperStatusUp && a.IfType != IF_TYPE_SOFTWARE_LOOPBACK {
                let primary = has_best && a.Anonymous1.Anonymous.IfIndex == best;
                let mut unicast = a.FirstUnicastAddress;
                while !unicast.is_null() {
                    let u = &*unicast;
                    let sockaddr = u.Address.lpSockaddr;
                    if !sockaddr.is_null() && (*sockaddr).sa_family == AF_INET && u.Address.iSockaddrLength as usize >= size_of::<SOCKADDR_IN>() {
                        let v4 = &*(sockaddr as *const SOCKADDR_IN);
                        found.push((Ipv4Addr::from(v4.sin_addr.S_un.S_addr.to_ne_bytes()), primary));
                    }
                    unicast = u.Next;
                }
            }
            adapter = a.Next;
        }
    }
    arrange(found)
}

/// Off Windows there is no adapter list read yet; the page then says it found no address, and the
/// person uses the one their system shows.
#[cfg(not(windows))]
pub fn this_pc() -> Vec<LanAddress> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v4(s: &str) -> Ipv4Addr {
        s.parse().unwrap()
    }

    #[test]
    fn loopback_and_self_assigned_addresses_are_not_offered() {
        assert!(offerable(v4("192.168.1.20")));
        assert!(offerable(v4("10.0.0.5")));
        assert!(!offerable(v4("127.0.0.1")));
        assert!(!offerable(v4("169.254.10.1")));
        assert!(!offerable(v4("0.0.0.0")));
    }

    #[test]
    fn the_primary_adapter_comes_first_and_nothing_is_listed_twice() {
        let arranged = arrange(vec![(v4("172.20.0.1"), false), (v4("192.168.1.20"), true), (v4("169.254.1.1"), false), (v4("172.20.0.1"), false)]);
        assert_eq!(
            arranged,
            vec![
                LanAddress { ip: IpAddr::V4(v4("192.168.1.20")), primary: true },
                LanAddress { ip: IpAddr::V4(v4("172.20.0.1")), primary: false },
            ]
        );
    }

    /// Reading the adapter list sends and listens on nothing, so it is safe to run here; what it
    /// finds depends on the PC, so only its shape is checked.
    #[test]
    fn reading_this_pcs_addresses_offers_only_usable_ones() {
        for address in this_pc() {
            let IpAddr::V4(ip) = address.ip else { panic!("only IPv4 is offered: {address:?}") };
            assert!(offerable(ip), "{ip}");
        }
    }
}
