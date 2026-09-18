//! The app's icon, drawn rather than shipped: a light "G" on a dark grey disc.
//!
//! One drawing serves the tray icon, the window's icon and the taskbar, each of which wants a
//! different size, so it is generated at whatever size is asked for rather than scaled from a
//! bitmap. Design cues only; nothing here is taken from Antelope's own artwork.

/// RGBA, top row first, antialiased by sampling each pixel 4x4.
pub fn rgba(size: usize) -> Vec<[u8; 4]> {
    const DISC: [f32; 3] = [0x2b as f32, 0x2d as f32, 0x31 as f32];
    const MARK: [f32; 3] = [0xe8 as f32, 0xa8 as f32, 0x38 as f32];
    const SAMPLES: usize = 4;
    let mut out = Vec::with_capacity(size * size);
    for py in 0..size {
        for px in 0..size {
            let (mut disc, mut mark) = (0.0f32, 0.0f32);
            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    // -1..1 across the icon, y up.
                    let x = ((px as f32 + (sx as f32 + 0.5) / SAMPLES as f32) / size as f32) * 2.0 - 1.0;
                    let y = 1.0 - ((py as f32 + (sy as f32 + 0.5) / SAMPLES as f32) / size as f32) * 2.0;
                    let r = (x * x + y * y).sqrt();
                    if r <= 0.97 {
                        disc += 1.0;
                        let angle = y.atan2(x).to_degrees();
                        // A ring open on the right above the bar, plus the bar itself.
                        let ring = (0.40..=0.66).contains(&r) && !(0.0..60.0).contains(&angle);
                        let bar = (-0.06..=0.12).contains(&y) && (0.05..=0.66).contains(&x);
                        if ring || bar {
                            mark += 1.0;
                        }
                    }
                }
            }
            let n = (SAMPLES * SAMPLES) as f32;
            let (coverage, m) = (disc / n, if disc > 0.0 { mark / disc } else { 0.0 });
            let mix = |i: usize| (DISC[i] + (MARK[i] - DISC[i]) * m).round() as u8;
            out.push([mix(0), mix(1), mix(2), (coverage * 255.0).round() as u8]);
        }
    }
    out
}
