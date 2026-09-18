use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use super::asio::tests::from_words;
use super::fake::{FakeDll, FakePc, QUADRO_DLL, QUADRO_SAFE_MODE, STUDIO_DLL, STUDIO_SAFE_MODE};
use super::*;

const QUADRO: &str = "1000000000001";
const STUDIO: &str = "1000000000003";

fn both() -> (Arc<FakeDll>, Arc<FakeDll>, FakePc) {
    let quadro = Arc::new(FakeDll::quadro(QUADRO));
    let studio = Arc::new(FakeDll::studio(STUDIO));
    let pc = FakePc::both(quadro.clone(), studio.clone());
    (quadro, studio, pc)
}

fn settings(answer: DriverAnswer) -> DriverSettings {
    match answer {
        DriverAnswer::Read(settings) => settings,
        other => panic!("expected a reading, got {other:?}"),
    }
}

fn message(answer: DriverAnswer) -> String {
    match answer {
        DriverAnswer::NoDriver { message } | DriverAnswer::NotFound { message } | DriverAnswer::Failed { message } => message,
        DriverAnswer::Read(settings) => panic!("expected no reading, got {settings:?}"),
    }
}

#[test]
fn the_quadro_reads_as_its_panel_showed() {
    let (_, _, pc) = both();
    let read = settings(read_device(&pc, QUADRO));
    assert_eq!(read.dll, QUADRO_DLL);
    assert_eq!(read.service, "Zen_Quadro_Synergy_Core");
    assert_eq!(read.api_version, "5.12");
    assert!(read.api_known);
    assert_eq!(read.driver_version, Reading::Read { value: "5.68.0".into() });
    assert_eq!(read.sample_rate, Reading::Read { value: 44100 });
    assert_eq!(read.asio_instances, Some(1));
    let Reading::Read { value: asio } = read.asio else { panic!("{:?}", read.asio) };
    assert_eq!((asio.buffer_size, asio.input_latency, asio.output_latency), (512, 571, 632));
    assert_eq!(read.safe_mode, Reading::Read { value: true });
}

#[test]
fn the_studio_reads_as_its_panel_showed_with_no_instance_count_and_flat_safe_mode() {
    let (quadro, studio, pc) = both();
    let read = settings(read_device(&pc, STUDIO));
    assert_eq!(read.service, "ZenStudioTB");
    assert_eq!(read.api_version, "5.7");
    assert_eq!(read.driver_version, Reading::Read { value: "5.0.0".into() });
    assert_eq!(read.asio_instances, None, "its API cannot say");
    assert_eq!(read.asio_instance, 0);
    let Reading::Read { value: asio } = read.asio else { panic!("{:?}", read.asio) };
    assert_eq!((asio.buffer_size, asio.input_latency, asio.output_latency), (512, 568, 585));
    assert_eq!(read.safe_mode, Reading::Read { value: true });
    assert!(!studio.calls().contains(&"TUSBAUDIO_GetASIOInstanceCount".to_string()));
    // The Quadro's driver was asked first and does not list it: nothing past its properties.
    assert!(!quadro.calls().contains(&"TUSBAUDIO_GetASIOInstanceInfo".to_string()));
}

#[test]
fn a_driver_device_is_tied_by_serial_not_by_order() {
    let quadro = Arc::new(FakeDll { serials: vec!["1".into(), QUADRO.into()], ..FakeDll::quadro(QUADRO) });
    let pc = FakePc::both(quadro.clone(), Arc::new(FakeDll::studio(STUDIO)));
    assert_eq!(settings(read_device(&pc, QUADRO)).service, "Zen_Quadro_Synergy_Core");
    let opens = quadro.calls().iter().filter(|c| *c == "TUSBAUDIO_OpenDeviceByIndex").count();
    assert_eq!(opens, 2, "the second device is the one");
    assert_eq!(settings(read_device(&pc, &format!(" {QUADRO} "))).service, "Zen_Quadro_Synergy_Core", "whitespace is not identity");
}

#[test]
fn every_opened_handle_is_closed() {
    let (quadro, studio, pc) = both();
    read_device(&pc, STUDIO);
    read_device(&pc, "nobody");
    assert!(quadro.open.lock().unwrap().is_empty());
    assert!(studio.open.lock().unwrap().is_empty());
    let failing = Arc::new(FakeDll { failing: [("TUSBAUDIO_GetDeviceProperties", 7)].into(), ..FakeDll::quadro(QUADRO) });
    let pc = FakePc::both(failing.clone(), Arc::new(FakeDll::studio(STUDIO)));
    read_device(&pc, QUADRO);
    assert!(failing.open.lock().unwrap().is_empty(), "closed on the failing path too");
}

#[test]
fn a_device_no_driver_lists_is_not_found() {
    let (_, _, pc) = both();
    let answer = read_device(&pc, "999");
    assert!(matches!(answer, DriverAnswer::NotFound { .. }), "{answer:?}");
    assert_eq!(message(answer), "No driver API on this PC lists this device.");
}

#[test]
fn a_pc_with_no_driver_is_an_ordinary_answer() {
    let answer = read_device(&FakePc::default(), QUADRO);
    assert_eq!(answer, DriverAnswer::NotFound { message: "No driver API found on this PC.".into() });
}

#[test]
fn not_being_able_to_look_says_why() {
    let pc = FakePc { cannot_look: Some("the uninstall key could not be opened".into()), ..FakePc::default() };
    let answer = read_device(&pc, QUADRO);
    assert!(matches!(answer, DriverAnswer::Failed { .. }));
    assert!(message(answer).contains("the uninstall key could not be opened"));
}

#[test]
fn a_dll_that_will_not_load_is_named_and_the_others_still_read() {
    let mut pc = FakePc::both(Arc::new(FakeDll::quadro(QUADRO)), Arc::new(FakeDll::studio(STUDIO)));
    pc.dlls[0].1 = Err("LoadLibrary failed: the specified module could not be found (126)".into());
    assert_eq!(settings(read_device(&pc, STUDIO)).service, "ZenStudioTB");
    let answer = read_device(&pc, QUADRO);
    assert!(matches!(answer, DriverAnswer::Failed { .. }));
    let message = message(answer);
    assert!(message.contains("Zen_Quadro_Synergy_Coreapi_x64.dll: LoadLibrary failed"), "{message}");
}

#[test]
fn a_missing_export_is_a_quiet_message() {
    let quadro = Arc::new(FakeDll { missing: vec!["TUSBAUDIO_EnumerateDevices"], ..FakeDll::quadro(QUADRO) });
    let pc = FakePc::both(quadro, Arc::new(FakeDll::studio(STUDIO)));
    let message = message(read_device(&pc, QUADRO));
    assert!(message.contains("its API has no TUSBAUDIO_EnumerateDevices"), "{message}");
}

#[test]
fn a_missing_read_export_leaves_the_other_settings() {
    let quadro = Arc::new(FakeDll { missing: vec!["TUSBAUDIO_GetASIOInstanceInfo"], ..FakeDll::quadro(QUADRO) });
    let pc = FakePc::both(quadro, Arc::new(FakeDll::studio(STUDIO)));
    let read = settings(read_device(&pc, QUADRO));
    assert_eq!(read.asio, Reading::Unread { message: "Could not be read: its API has no TUSBAUDIO_GetASIOInstanceInfo.".into() });
    assert_eq!(read.sample_rate, Reading::Read { value: 44100 });
    assert_eq!(read.safe_mode, Reading::Read { value: true });
}

#[test]
fn a_failing_status_names_the_call_and_the_status() {
    let quadro = Arc::new(FakeDll { failing: [("TUSBAUDIO_GetCurrentSampleRate", 0xee00_0005)].into(), ..FakeDll::quadro(QUADRO) });
    let pc = FakePc::both(quadro, Arc::new(FakeDll::studio(STUDIO)));
    let read = settings(read_device(&pc, QUADRO));
    assert_eq!(
        read.sample_rate,
        Reading::Unread { message: "Could not be read: TUSBAUDIO_GetCurrentSampleRate answered TSTATUS_INVALID_PARAMETER (status 0xee000005).".into() }
    );
    let quadro = Arc::new(FakeDll { failing: [("TUSBAUDIO_GetDriverInfo", 1)].into(), ..FakeDll::quadro(QUADRO) });
    let pc = FakePc::both(quadro, Arc::new(FakeDll::studio(STUDIO)));
    assert!(matches!(settings(read_device(&pc, QUADRO)).driver_version, Reading::Unread { .. }));
}

#[test]
fn an_api_the_dll_refuses_is_not_read() {
    let quadro = Arc::new(FakeDll { api_version: 6 << 16, accepts: false, ..FakeDll::quadro(QUADRO) });
    let pc = FakePc::both(quadro.clone(), Arc::new(FakeDll::studio(STUDIO)));
    let message = message(read_device(&pc, QUADRO));
    assert!(message.contains("its API (6.0) is not one Gazelle can read"), "{message}");
    assert!(!quadro.calls().contains(&"TUSBAUDIO_EnumerateDevices".to_string()), "nothing past the check");
}

#[test]
fn an_unseen_api_the_dll_accepts_is_read_and_marked() {
    let quadro = Arc::new(FakeDll { api_version: (5 << 16) | 30, ..FakeDll::quadro(QUADRO) });
    let pc = FakePc::both(quadro, Arc::new(FakeDll::studio(STUDIO)));
    let read = settings(read_device(&pc, QUADRO));
    assert_eq!(read.api_version, "5.30");
    assert!(!read.api_known);
    assert!(matches!(read.asio, Reading::Read { .. }));
}

#[test]
fn nonsense_in_the_structure_is_could_not_be_read_not_a_number() {
    // A layout that moved by one word: the count lands where the buffer was.
    let mut words = vec![44100, 0, 0, 0x10000, 571, 632, 512, 9, 8];
    words.extend([16, 32, 64, 128, 256, 512, 1024, 2048]);
    let quadro = Arc::new(FakeDll { asio: from_words(&words), ..FakeDll::quadro(QUADRO) });
    let pc = FakePc::both(quadro, Arc::new(FakeDll::studio(STUDIO)));
    let Reading::Unread { message } = settings(read_device(&pc, QUADRO)).asio else { panic!("read nonsense") };
    assert!(message.starts_with("Could not be read: "), "{message}");
}

#[test]
fn an_implausible_current_rate_is_not_shown() {
    let quadro = Arc::new(FakeDll { rate: 3, ..FakeDll::quadro(QUADRO) });
    let pc = FakePc::both(quadro, Arc::new(FakeDll::studio(STUDIO)));
    assert!(matches!(settings(read_device(&pc, QUADRO)).sample_rate, Reading::Unread { .. }));
}

#[test]
fn a_count_that_is_not_a_count_is_refused() {
    let quadro = Arc::new(FakeDll { serials: (0..65).map(|n| n.to_string()).collect(), ..FakeDll::quadro(QUADRO) });
    let pc = FakePc::both(quadro, Arc::new(FakeDll::studio(STUDIO)));
    assert!(message(read_device(&pc, QUADRO)).contains("65 devices"));
}

#[test]
fn a_driver_with_no_asio_instance_says_so() {
    let quadro = Arc::new(FakeDll { instance_count: Some(0), ..FakeDll::quadro(QUADRO) });
    let pc = FakePc::both(quadro.clone(), Arc::new(FakeDll::studio(STUDIO)));
    let read = settings(read_device(&pc, QUADRO));
    assert_eq!(read.asio, Reading::Unread { message: "The driver has no ASIO instance.".into() });
    assert!(!quadro.calls().contains(&"TUSBAUDIO_GetASIOInstanceInfo".to_string()));
}

#[test]
fn safe_mode_off_and_missing_and_unreadable() {
    let (_, _, mut pc) = both();
    pc.registry.insert((QUADRO_SAFE_MODE.into(), "AsioSafeMode".into()), 0);
    assert_eq!(settings(read_device(&pc, QUADRO)).safe_mode, Reading::Read { value: false });

    pc.registry.clear();
    assert_eq!(
        settings(read_device(&pc, STUDIO)).safe_mode,
        Reading::Unread { message: "The driver keeps no Safe Mode setting in the registry.".into() }
    );

    // The per-instance value wins over a flat one.
    pc.registry.insert((QUADRO_SAFE_MODE.into(), "AsioSafeMode".into()), 0);
    let flat = QUADRO_SAFE_MODE.trim_end_matches(r"\AsioInstance0").to_string();
    pc.registry.insert((flat, "AsioSafeMode".into()), 1);
    assert_eq!(settings(read_device(&pc, QUADRO)).safe_mode, Reading::Read { value: false });

    pc.registry_error = Some("access denied (5)".into());
    assert_eq!(settings(read_device(&pc, QUADRO)).safe_mode, Reading::Unread { message: "Could not be read: access denied (5).".into() });
    let _ = STUDIO_SAFE_MODE;
}

#[test]
fn the_service_is_the_dll_name_without_its_suffix() {
    assert_eq!(service_of(Path::new(QUADRO_DLL)).as_deref(), Some("Zen_Quadro_Synergy_Core"));
    assert_eq!(service_of(Path::new(STUDIO_DLL)).as_deref(), Some("ZenStudioTB"));
    assert_eq!(service_of(Path::new(r"C:\x\ZenStudioTBAPI_X64.DLL")).as_deref(), Some("ZenStudioTB"));
    assert_eq!(service_of(Path::new(r"C:\x\api_x64.dll")), None);
    assert_eq!(service_of(Path::new(r"C:\x\other.dll")), None);
}

#[test]
fn only_read_functions_may_be_resolved() {
    for name in READ_EXPORTS {
        let verb = name.trim_start_matches("TUSBAUDIO_");
        for forbidden in ["Set", "Load", "Start", "Dfu", "Enable", "Disable", "Write", "Store"] {
            assert!(!verb.starts_with(forbidden), "{name} is not a read");
        }
    }
}

#[test]
fn answers_are_cached_briefly_and_refreshed_on_demand() {
    let (_, _, pc) = both();
    let pc = Arc::new(pc);
    let service = DriverService::new(pc.clone(), Duration::from_secs(60));
    let first = service.read("serial:x", QUADRO, false);
    assert!(!first.cached);
    assert_eq!(pc.loads(), 1, "the Quadro's DLL is listed first and answers");
    let again = service.read("serial:x", QUADRO, false);
    assert!(again.cached);
    assert_eq!(again.read_at_ms, first.read_at_ms);
    assert_eq!(pc.loads(), 1, "served from the cache");
    let fresh = service.read("serial:x", QUADRO, true);
    assert!(!fresh.cached);
    assert_eq!(pc.loads(), 2);

    let expired = DriverService::new(pc.clone(), Duration::ZERO);
    expired.read("serial:x", QUADRO, false);
    assert!(!expired.read("serial:x", QUADRO, false).cached, "an old answer is read again");
}

#[test]
fn the_report_serialises_flat_with_its_state() {
    let (_, _, pc) = both();
    let service = DriverService::new(Arc::new(pc), CACHE_FOR);
    let json = serde_json::to_value(service.read("serial:1000000000001", QUADRO, false)).unwrap();
    assert_eq!(json["device_id"], "serial:1000000000001");
    assert_eq!(json["state"], "read");
    assert_eq!(json["driver_version"], serde_json::json!({ "state": "read", "value": "5.68.0" }));
    assert_eq!(json["asio"]["value"]["input_latency"], 571);
    assert_eq!(json["asio"]["value"]["buffer_sizes"], serde_json::json!([8, 16, 32, 64, 128, 256, 512, 1024, 2048]));
    assert_eq!(json["safe_mode"], serde_json::json!({ "state": "read", "value": true }));
    let none = serde_json::to_value(DriverReport { device_id: "x".into(), read_at_ms: 1, cached: false, answer: DriverAnswer::NotFound { message: "m".into() } }).unwrap();
    assert_eq!(none, serde_json::json!({ "device_id": "x", "read_at_ms": 1, "cached": false, "state": "not_found", "message": "m" }));
}
