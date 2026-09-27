//! **The recorder with no hardware**: the aggregate crate's own fake PC, with both Antelope
//! interfaces made of data, and a pump that makes their blocks happen at the real rate with a test
//! signal on every input.
//!
//! This is what `--backend loopback` records from, so the Recording page can be tried, tested and
//! photographed without a driver anywhere near it. The aggregate is the real one (the real plan,
//! rings, delays and audio path); only the vendor drivers are fakes, exactly as in every test here.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gazelle_aggregate::fake::{FakeDevice, FakePc, Spec};

/// The registry keys the fake interfaces go by, which a setup names them with.
pub const QUADRO_KEY: &str = "Zen Quadro Synergy Core";
pub const STUDIO_KEY: &str = "ZenStudioTB ASIO Driver";

/// This PC as phase 0 found it, made of data: a Quadro with 16 inputs and a Studio+ with 24.
pub fn simulated_pc() -> Arc<FakePc> {
    Arc::new(
        FakePc::new()
            .with(
                QUADRO_KEY,
                "{12217625-CB57-11EE-908D-7085C2FB2DD5}",
                r"c:\program files\antelope audio\zen quadro\zen_quadro.dll",
                Spec { preferred: 256, ..Spec::named(QUADRO_KEY) }.with_channels(16, 16).with_latency(639 + 256, 799),
            )
            .with(
                STUDIO_KEY,
                "{AE4A4452-A316-11E5-A113-080027F6C1F4}",
                r"c:\program files\antelope audio\zenstudiotb\zenstudiotb.dll",
                Spec { preferred: 256, ..Spec::named(STUDIO_KEY) }.with_channels(24, 24).with_latency(639, 700),
            ),
    )
}

/// What the pump needs to know once the aggregate is open.
#[derive(Clone, Copy, Debug)]
pub struct Timing {
    pub rate: f64,
    pub block: usize,
}

/// A pump for the fake PC: every device's blocks, at the rate the aggregate runs at, the devices in
/// `order` (the ones that follow first, the one driving the callback last, as the hardware does).
///
/// Each input carries a tone of its own, a few steps apart, at about -18 dBFS, swelling slowly so a
/// meter moves.
pub fn pump(order: Vec<Arc<FakeDevice>>, timing: Timing) -> impl FnMut(usize) -> bool + Send {
    let started = Instant::now();
    let block = timing.block.max(1);
    let rate = timing.rate.max(1.0);
    let mut fired: u64 = 0;
    let mut samples = vec![0i32; block];
    move |_index| {
        let due = (started.elapsed().as_secs_f64() * rate / block as f64) as u64;
        // Far behind (a paused debugger, a sleeping laptop) is skipped, not played back in a rush.
        if due > fired + 256 {
            fired = due - 1;
        }
        while fired < due {
            let half = (fired & 1) as usize;
            let first = fired * block as u64;
            for (device, fake) in order.iter().enumerate() {
                for channel in 0..fake.input_count() {
                    tone(&mut samples, first, rate, device, channel);
                    fake.set_input(channel, half, &samples);
                }
            }
            for fake in &order {
                fake.fire(half);
            }
            fired += 1;
        }
        std::thread::sleep(Duration::from_millis(1));
        true
    }
}

/// One block of one input's tone.
fn tone(into: &mut [i32], first: u64, rate: f64, device: usize, channel: usize) {
    let hz = 110.0 * 2f64.powf((channel as f64 + device as f64 * 0.5) / 6.0);
    let level = 0.125 * (0.6 + 0.4 * (std::f64::consts::TAU * 0.2 * first as f64 / rate + channel as f64).sin());
    for (i, sample) in into.iter_mut().enumerate() {
        let t = (first + i as u64) as f64 / rate;
        let value = level * (std::f64::consts::TAU * hz * t).sin();
        // The top 24 bits, as a converter fills them.
        *sample = ((value * 8_388_607.0) as i32) << 8;
    }
}
