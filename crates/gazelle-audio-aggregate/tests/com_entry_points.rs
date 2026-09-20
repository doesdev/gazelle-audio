//! The driver as a DAW meets it: through `DllGetClassObject`, a class factory, and the vtable.
//!
//! Every other test calls the driver's own Rust. This one goes the whole way round the interface,
//! in this process, with devices made of data underneath, so that the vtable itself, the class
//! factory, the reference counting and the pointers handed back are exercised rather than assumed.

#![cfg(windows)]

use std::ffi::{c_void, CStr};
use std::ptr::{null, null_mut};
use std::sync::Arc;

use gazelle_aggregate::com::{self, Bench};
use gazelle_aggregate::config::{Alignment, Config, DeviceConfig};
use gazelle_aggregate::daw;
use gazelle_aggregate::fake::{FakeHost, FakePc, Spec};
use gazelle_aggregate::{CLASS_ID, DRIVER_NAME, DRIVER_VERSION};
use gazelle_audio_stream_abi::raw::{
    hresult, BufferInfoRaw, ChannelInfoRaw, ClassFactory, Guid, Object, Samples, Vtable, IID_ICLASSFACTORY, OK, TRUE,
};
use gazelle_audio_stream_abi::sample;

const BLOCK: i32 = 8;

fn spec(inputs: i32, outputs: i32) -> Spec {
    Spec { min: 2, max: 64, preferred: BLOCK, ..Spec::default() }.with_channels(inputs, outputs).with_latency(600, 700)
}

fn pc() -> Arc<FakePc> {
    Arc::new(
        FakePc::new()
            .with("Device A", "{AAAAAAAA-0000-0000-0000-000000000001}", r"c:\antelope\a.dll", spec(2, 2))
            .with("Device B", "{BBBBBBBB-0000-0000-0000-000000000002}", r"c:\antelope\b.dll", spec(2, 2)),
    )
}

fn config() -> Config {
    Config {
        devices: vec![
            DeviceConfig { key: Some("Device A".into()), name: Some("A".into()), ..DeviceConfig::default() },
            DeviceConfig { key: Some("Device B".into()), name: Some("B".into()), ..DeviceConfig::default() },
        ],
        alignment: Alignment::LowestLatency,
        ..Config::default()
    }
}

/// Ask the DLL for its class factory and make one driver object, the way COM does.
fn create() -> *mut Object {
    let mut factory: *mut c_void = null_mut();
    let answer = unsafe { com::DllGetClassObject(&CLASS_ID, &IID_ICLASSFACTORY, &mut factory) };
    assert_eq!(answer, hresult::S_OK, "the DLL did not hand back its class factory");
    assert!(!factory.is_null());

    let factory = factory as *mut ClassFactory;
    let mut object: *mut c_void = null_mut();
    // The convention this interface is published under uses the class id as the interface id.
    let answer = unsafe { ((*(*factory).vtable).create_instance)(factory, null_mut(), &CLASS_ID, &mut object) };
    assert_eq!(answer, hresult::S_OK, "the factory did not make a driver");
    assert!(!object.is_null());
    object.cast()
}

fn vtable(object: *mut Object) -> &'static Vtable {
    unsafe { &*(*object).vtable }
}

fn text(buffer: &[i8]) -> String {
    unsafe { CStr::from_ptr(buffer.as_ptr()) }.to_string_lossy().to_string()
}

#[test]
fn a_daw_can_open_the_driver_run_it_and_close_it_again() {
    let _order = daw::session();
    let pc = pc();
    {
        let pc = Arc::clone(&pc);
        com::install_bench(move || Bench {
            host: Box::new(FakeHost { pc: Arc::clone(&pc) }),
            config: config(),
            source: "the test bench".to_string(),
        });
    }

    let object = create();
    let calls = vtable(object);

    // What it calls itself, before anything is opened.
    let mut name = [0i8; 32];
    unsafe { (calls.get_driver_name)(object, name.as_mut_ptr()) };
    assert_eq!(text(&name), DRIVER_NAME);
    assert_eq!(unsafe { (calls.get_driver_version)(object) }, DRIVER_VERSION);

    // Open it.
    assert_eq!(unsafe { (calls.init)(object, null_mut()) }, TRUE, "init refused");

    let (mut inputs, mut outputs) = (0i32, 0i32);
    assert_eq!(unsafe { (calls.get_channels)(object, &mut inputs, &mut outputs) }, OK);
    assert_eq!((inputs, outputs), (4, 4), "two devices of two channels each, as one device");

    let mut info = ChannelInfoRaw { channel: 2, is_input: 1, ..ChannelInfoRaw::default() };
    assert_eq!(unsafe { (calls.get_channel_info)(object, &mut info) }, OK);
    assert_eq!(text(&info.name), "B 1", "channels are named so a person can tell the devices apart");
    assert_eq!(info.sample_type, sample::INT32_LSB);

    let (mut min, mut max, mut preferred, mut granularity) = (0i32, 0i32, 0i32, 0i32);
    assert_eq!(unsafe { (calls.get_buffer_size)(object, &mut min, &mut max, &mut preferred, &mut granularity) }, OK);
    assert_eq!(preferred, BLOCK);

    let (mut latency_in, mut latency_out) = (0i32, 0i32);
    assert_eq!(unsafe { (calls.get_latencies)(object, &mut latency_in, &mut latency_out) }, OK);
    assert_eq!((latency_in, latency_out), (600 + BLOCK, 700 + BLOCK), "one figure each way for the whole aggregate");

    assert_eq!(unsafe { (calls.can_sample_rate)(object, 96_000.0) }, OK);
    assert_ne!(unsafe { (calls.can_sample_rate)(object, 1.0) }, OK, "a rate no device has");
    let mut rate = 0f64;
    assert_eq!(unsafe { (calls.get_sample_rate)(object, &mut rate) }, OK);
    assert_eq!(rate, 96_000.0);

    // Make the buffers: every channel, inputs then outputs, as a DAW usually asks.
    let mut infos: Vec<BufferInfoRaw> = (0..inputs)
        .map(|channel| BufferInfoRaw { is_input: 1, channel, buffers: [null_mut(); 2] })
        .chain((0..outputs).map(|channel| BufferInfoRaw { is_input: 0, channel, buffers: [null_mut(); 2] }))
        .collect();
    let host = daw::callbacks();
    let total = infos.len() as i32;
    assert_eq!(unsafe { (calls.create_buffers)(object, infos.as_mut_ptr(), total, BLOCK, &host) }, OK);
    assert!(infos.iter().all(|info| !info.buffers[0].is_null() && !info.buffers[1].is_null()), "every buffer got a pointer");
    // The driver's buffers start silent, which is what an output buffer must be before anything
    // is written to it.
    for info in infos.iter().filter(|info| info.is_input == 0) {
        for half in info.buffers {
            let run = unsafe { std::slice::from_raw_parts(half as *const i32, BLOCK as usize) };
            assert!(run.iter().all(|&sample| sample == 0), "an output buffer was handed out full of something");
        }
    }

    let (heard, played): (Vec<_>, Vec<_>) = infos.iter().partition(|info| info.is_input != 0);
    daw::with(|session| {
        session.buffers(
            BLOCK as usize,
            heard.iter().map(|info| info.buffers).collect(),
            played.iter().map(|info| info.buffers).collect(),
        )
    });

    // A channel the DAW asked for now says so.
    let mut info = ChannelInfoRaw { channel: 0, is_input: 1, ..ChannelInfoRaw::default() };
    unsafe { (calls.get_channel_info)(object, &mut info) };
    assert_eq!(info.is_active, 1);

    // Run it.
    assert_eq!(unsafe { (calls.start)(object) }, OK);
    let (a, b) = (pc.device("Device A"), pc.device("Device B"));
    for round in 0..3usize {
        let half = round & 1;
        a.set_input(0, half, &daw::tone(BLOCK as usize, 100 + round as i32 * 10));
        b.set_input(0, half, &daw::tone(BLOCK as usize, 300 + round as i32 * 10));
        b.fire(half);
        a.fire(half);
    }
    assert_eq!(daw::with(|session| session.calls), 3, "the DAW was called once per block of the master");
    let heard_blocks = daw::with(|session| session.heard.clone());
    assert_eq!(heard_blocks[2][0], daw::tone(BLOCK as usize, 120), "the master's own input");
    assert_eq!(heard_blocks[2][2], daw::tone(BLOCK as usize, 320), "and the other device's, over the ring");

    let mut position = Samples::default();
    let mut stamp = Samples::default();
    assert_eq!(unsafe { (calls.get_sample_position)(object, &mut position, &mut stamp) }, OK);
    assert_eq!(position.to_i64(), BLOCK as i64 * 3);

    // And close it.
    assert_eq!(unsafe { (calls.stop)(object) }, OK);
    assert_eq!(unsafe { (calls.dispose_buffers)(object) }, OK);
    assert!(!a.is_started() && !b.is_started());

    assert_eq!(unsafe { com::DllCanUnloadNow() }, hresult::S_FALSE, "an object is still alive");
    assert_eq!(unsafe { (calls.release)(object) }, 0);
    assert_eq!(unsafe { com::DllCanUnloadNow() }, hresult::S_OK, "and now nothing is");
    com::clear_bench();
}

#[test]
fn the_object_answers_for_itself_and_counts_who_holds_it() {
    let _order = daw::session();
    {
        let pc = pc();
        com::install_bench(move || Bench {
            host: Box::new(FakeHost { pc: Arc::clone(&pc) }),
            config: config(),
            source: "the test bench".to_string(),
        });
    }
    let object = create();
    let calls = vtable(object);

    // Asked for itself, it hands itself back and counts the new holder.
    let mut again: *mut c_void = null_mut();
    assert_eq!(unsafe { (calls.query_interface)(object, &CLASS_ID, &mut again) }, hresult::S_OK);
    assert_eq!(again.cast(), object);
    assert_eq!(unsafe { (calls.release)(object) }, 1, "one holder left");

    // Asked for something else, it says no and hands back nothing.
    let stranger = Guid { data1: 0xDEAD, data2: 0, data3: 0, data4: [0; 8] };
    let mut nothing: *mut c_void = null_mut();
    assert_eq!(unsafe { (calls.query_interface)(object, &stranger, &mut nothing) }, hresult::E_NOINTERFACE);
    assert!(nothing.is_null());

    assert_eq!(unsafe { (calls.release)(object) }, 0);
    com::clear_bench();
}

#[test]
fn calls_made_in_the_wrong_order_are_refused_rather_than_obeyed() {
    let _order = daw::session();
    {
        let pc = pc();
        com::install_bench(move || Bench {
            host: Box::new(FakeHost { pc: Arc::clone(&pc) }),
            config: config(),
            source: "the test bench".to_string(),
        });
    }
    let object = create();
    let calls = vtable(object);

    // Before init there is nothing to say.
    let (mut inputs, mut outputs) = (0i32, 0i32);
    assert_ne!(unsafe { (calls.get_channels)(object, &mut inputs, &mut outputs) }, OK);

    // Started before its buffers were made, it refuses and says why.
    assert_ne!(unsafe { (calls.start)(object) }, OK);
    let mut message = [0i8; 124];
    unsafe { (calls.get_error_message)(object, message.as_mut_ptr()) };
    assert!(text(&message).contains("buffers"), "{}", text(&message));

    // A class id that is not ours is not our business.
    let stranger = Guid { data1: 1, data2: 2, data3: 3, data4: [0; 8] };
    let mut nothing: *mut c_void = null_mut();
    assert_eq!(
        unsafe { com::DllGetClassObject(&stranger, &IID_ICLASSFACTORY, &mut nothing) },
        hresult::CLASS_E_CLASSNOTAVAILABLE
    );

    // There is no window to open, and nothing in the future selector.
    assert_ne!(unsafe { (calls.control_panel)(object) }, OK);
    assert_ne!(unsafe { (calls.future)(object, 1, null_mut()) }, OK);

    // One clock source, which is "whatever the devices are set to".
    let mut sources = [gazelle_audio_stream_abi::raw::ClockSourceRaw::default()];
    let mut count = 1i32;
    assert_eq!(unsafe { (calls.get_clock_sources)(object, sources.as_mut_ptr(), &mut count) }, OK);
    assert_eq!(count, 1);
    assert!(!text(&sources[0].name).is_empty());
    assert_eq!(unsafe { (calls.set_clock_source)(object, 0) }, OK);
    assert_ne!(unsafe { (calls.set_clock_source)(object, 7) }, OK);

    assert_eq!(unsafe { (calls.release)(object) }, 0);
    com::clear_bench();
}

/// Nothing here should have left a pointer behind; this is the check that the DLL could be
/// unloaded after a session, which is what a DAW does when it switches driver.
#[test]
fn nothing_is_left_holding_the_dll_after_a_session() {
    let _order = daw::session();
    {
        let pc = pc();
        com::install_bench(move || Bench {
            host: Box::new(FakeHost { pc: Arc::clone(&pc) }),
            config: config(),
            source: "the test bench".to_string(),
        });
    }
    let object = create();
    let calls = vtable(object);
    assert_eq!(unsafe { (calls.init)(object, null_mut()) }, TRUE);
    assert_eq!(unsafe { (calls.release)(object) }, 0);
    assert_eq!(unsafe { com::DllCanUnloadNow() }, hresult::S_OK);
    let _ = null::<c_void>();
    com::clear_bench();
}
