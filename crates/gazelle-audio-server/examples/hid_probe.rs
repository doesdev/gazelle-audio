//! Read-only probe: lists HID interfaces, and the Antelope ones in particular, with what the
//! transport needs. It opens nothing and writes nothing, so it is safe to run while the devices are
//! in use (decision 0012: never drive real hardware blindly).
//!
//! `cargo run -p gazelle-audio-server --example hid_probe`

const ANTELOPE_VID: u16 = 0x23e5;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut api = hidapi::HidApi::new()?;
    api.refresh_devices()?;
    let all: Vec<_> = api.device_list().collect();
    println!("{} HID interfaces visible", all.len());
    for info in all.iter().take(8) {
        println!("  vid={:04x} pid={:04x} usage_page={:#06x} product={:?}", info.vendor_id(), info.product_id(), info.usage_page(), info.product_string());
    }
    let ours: Vec<_> = all.iter().filter(|d| d.vendor_id() == ANTELOPE_VID).collect();
    println!("{} Antelope HID interface(s)", ours.len());
    for info in ours {
        println!(
            "  pid={:04x} usage_page={:#06x} usage={:#06x} interface={} serial={:?} product={:?}\n    path={:?}",
            info.product_id(),
            info.usage_page(),
            info.usage(),
            info.interface_number(),
            info.serial_number(),
            info.product_string(),
            info.path(),
        );
    }

    // With `--read`, listen to each device for a moment. This only receives: nothing is sent, so the
    // devices keep doing whatever they were doing (decision 0012).
    if std::env::args().any(|a| a == "--read") {
        for info in all.iter().filter(|d| d.vendor_id() == ANTELOPE_VID) {
            let device = match info.open_device(&api) {
                Ok(d) => d,
                Err(e) => {
                    println!("  {:04x}: could not open: {e}", info.product_id());
                    continue;
                }
            };
            let mut buf = [0u8; 1024];
            for n in 0..3 {
                match device.read_timeout(&mut buf, 500) {
                    Ok(0) => println!("  {:04x} read {n}: nothing within 500 ms", info.product_id()),
                    Ok(len) => println!(
                        "  {:04x} read {n}: {len} bytes, cmd={:#x} head={:02x?}",
                        info.product_id(),
                        u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]),
                        &buf[..16.min(len)],
                    ),
                    Err(e) => println!("  {:04x} read {n}: {e}", info.product_id()),
                }
            }
        }
    }

    // With `--dump FILE`, save the packets read from the Quadro as hex lines, for a test fixture.
    let dump = std::env::args().skip_while(|a| a != "--dump").nth(1);
    if let Some(path) = dump {
        let info = all
            .iter()
            .find(|d| d.vendor_id() == ANTELOPE_VID && d.product_id() == 0xa2f9)
            .ok_or("no Quadro attached")?;
        let device = info.open_device(&api)?;
        let mut buf = [0u8; 1024];
        let mut lines = vec![
            "# Inbound HID reports read live from a Zen Quadro Synergy Core (hardware session 2,".to_string(),
            "# 2026-09-15), one per line as hex, exactly as the HID stack delivered them. The device".to_string(),
            "# wraps its replies in 8053 receive segments, which is the traffic no USBPcap capture held.".to_string(),
            "# Regenerate: cargo run -p gazelle-audio-server --example hid_probe -- --dump FILE".to_string(),
        ];
        let mut kept = 0;
        while kept < 120 {
            match device.read_timeout(&mut buf, 1000) {
                Ok(0) => break,
                Ok(len) => {
                    lines.push(buf[..len].iter().map(|b| format!("{b:02x}")).collect::<String>());
                    kept += 1;
                }
                Err(e) => return Err(e.into()),
            }
        }
        std::fs::write(&path, lines.join("
") + "
")?;
        println!("wrote {kept} packets to {path}");
    }
    Ok(())
}
