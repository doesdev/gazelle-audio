//! Device management: identity, the sync worker, and its async handle.

pub mod cyclic_loopback;
pub mod descriptor;
pub mod handle;
pub mod hotplug;
pub mod manager;
pub mod mixer_loopback;
pub mod read_loopback;
pub mod routing_loopback;
pub mod routing_memory;
pub mod usb;
pub mod worker;
