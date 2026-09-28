//! The aggregate as the web app's end to end tests see it, **compiled into debug builds only**.
//!
//! Those tests drive a real server through a real browser, on the loopback backend, and the rule is
//! that nothing they start reads or changes anything of the machine's own: no driver registry, no
//! device tree, no driver's shared record. Most of them answer `GET /api/v1/aggregate` in the
//! browser. A test that needs the server's own readiness answer, worked out from the loopback's real
//! routing (the phase path's reasons are), sets `GAZELLE_TEST_AGGREGATE=1`, and the server then runs
//! the aggregate on the same fakes its own tests use: a registry listing the two models' drivers and
//! Gazelle Aggregate, registered; a device tree with nothing in it; a driver that has published
//! nothing; and an elevator that runs nothing.
//!
//! In a release build (`--release`, which is how every shipped binary is built) [`test_service`] is
//! constant: the variable is never read, and a release cannot be talked into running on fakes.

#[cfg(any(debug_assertions, test))]
use std::path::PathBuf;
use std::sync::Arc;

use crate::aggregate::service::AggregateService;
use crate::device::manager::DeviceManager;
use crate::driver::DriverService;

pub const VAR: &str = "GAZELLE_TEST_AGGREGATE";

/// The aggregate on fakes, when the build and the environment allow it, else nothing.
pub fn test_service(devices: &Arc<DeviceManager>, driver: &Arc<DriverService>, export_path: &std::path::Path) -> Option<Arc<AggregateService>> {
    #[cfg(debug_assertions)]
    if std::env::var(VAR).is_ok_and(|value| value == "1") {
        return Some(fake(devices.clone(), driver.clone(), export_path.to_path_buf()));
    }
    let _ = (devices, driver, export_path);
    None
}

#[cfg(debug_assertions)]
fn fake(devices: Arc<DeviceManager>, driver: Arc<DriverService>, export_path: PathBuf) -> Arc<AggregateService> {
    use crate::aggregate::elevate::FakeElevator;
    use crate::aggregate::registry::{FakeRegistry, AGGREGATE_CLSID, AGGREGATE_NAME};
    use crate::aggregate::status::NoLink;
    use crate::aggregate::usb::FakeTopology;

    const DLL: &str = r"C:\gazelle-test\gazelle_aggregate.dll";
    let registry = FakeRegistry {
        entries: vec![
            FakeRegistry::entry("Zen Quadro Synergy Core", "{00000000-0000-0000-0000-00000000000A}", r"C:\gazelle-test\quadro.dll"),
            FakeRegistry::entry("Zen Studio+", "{00000000-0000-0000-0000-00000000000B}", r"C:\gazelle-test\studio.dll"),
            FakeRegistry::entry(AGGREGATE_NAME, AGGREGATE_CLSID, DLL),
        ],
        present: [DLL.to_string()].into_iter().collect(),
        failure: None,
    };
    Arc::new(AggregateService {
        devices,
        driver,
        registry: Arc::new(registry),
        topology: Arc::new(FakeTopology::default()),
        link: Arc::new(NoLink::default()),
        elevator: Arc::new(FakeElevator::default()),
        export_path,
        dll_candidates: vec![PathBuf::from(DLL)],
        bundled: crate::aggregate::bundled::Status::NotCarried,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry_set::RegistrySet;

    #[test]
    fn without_the_variable_the_server_runs_the_real_aggregate() {
        // Tests never set it, so this is the answer every test run gets from here.
        if std::env::var(VAR).is_ok() {
            return;
        }
        let devices = DeviceManager::new(RegistrySet::builtin().unwrap());
        let driver = DriverService::for_this_pc();
        assert!(test_service(&devices, &driver, std::path::Path::new("aggregate.json")).is_none());
    }

    #[test]
    fn the_fakes_list_both_models_and_a_registered_aggregate() {
        let devices = DeviceManager::new(RegistrySet::builtin().unwrap());
        let service = fake(devices, DriverService::for_this_pc(), PathBuf::from("aggregate.json"));
        let keys: Vec<String> = service.registry.entries().unwrap().into_iter().map(|entry| entry.key).collect();
        assert_eq!(keys, ["Zen Quadro Synergy Core", "Zen Studio+", "Gazelle Aggregate"]);
        let answer = service.answer(&crate::workspace::model::Workspace::default());
        assert!(answer.registration.registered && answer.registration.dll_present, "{:?}", answer.registration);
    }
}
