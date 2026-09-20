//! Phase 0 of the aggregate driver: can both Antelope USB drivers be opened, started and run in
//! **one process**, and do they stay in step when the S/PDIF cable locks them together?
//!
//! This is a **host**, not a driver. It contains no Steinberg SDK code and compiles against none:
//! the driver object's vtable is declared from the interface's public shape in `probe/windows.rs`,
//! and this crate builds with the SDK absent. It is MIT like the rest of the workspace. The
//! aggregate **driver** planned after this will be a separate component under GPLv3, because the
//! SDK it must use is offered as GPLv3 or under a signed agreement with Steinberg.
//!
//! What it does to the hardware: it reads. It never sets a sample rate, a clock source or a
//! buffer size, never opens a vendor control panel, and writes nothing but zeroes, to the output
//! buffers it was handed. Input is never read and never copied anywhere.
//!
//! A bare run lists every ASIO driver on the PC and stops. `--yes` is what opens anything, and
//! `GAZELLE_NO_HARDWARE=1` refuses outright.

mod probe;

use std::process::ExitCode;

use clap::Parser;

use probe::{buffer_mismatch, drift, rate_of, Opened, Options, Outcome, Verdict, NO_HARDWARE};

/// Open both Antelope USB ASIO drivers in one process and see whether they coexist.
#[derive(Parser, Debug)]
#[command(name = "gazelle-audio-aggregate-probe", version, about, long_about = None)]
struct Cli {
    /// Actually open the drivers. Without it the probe lists what it would open and stops.
    #[arg(long)]
    yes: bool,
    /// How long to let the drivers run, in seconds.
    #[arg(long, default_value_t = 20, value_name = "SECONDS")]
    seconds: u64,
    /// Run one driver alone: "quadro", "studio", or any part of a driver's name.
    #[arg(long, value_name = "NAME")]
    only: Option<String>,
    /// Ask each driver whether it can run at this rate. Asked, never set.
    #[arg(long, value_name = "HZ")]
    rate: Option<f64>,
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    if let Some(said) = probe::refusal(std::env::var(NO_HARDWARE).ok().as_deref()) {
        eprintln!("{said}");
        return ExitCode::from(2);
    }

    let options = Options { proceed: cli.yes, seconds: cli.seconds, only: cli.only, rate: cli.rate };
    here(&options)
}

#[cfg(windows)]
fn here(options: &Options) -> ExitCode {
    let host = match probe::windows::ThisPc::new() {
        Ok(host) => host,
        Err(message) => {
            eprintln!("COM would not start: {message}");
            return ExitCode::FAILURE;
        }
    };
    match probe::run(&host, options) {
        Ok(outcome) => report(&outcome, options),
        Err(message) => {
            eprintln!("The probe could not read this PC's ASIO drivers: {message}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(windows))]
fn here(_options: &Options) -> ExitCode {
    eprintln!("This probe opens Windows audio drivers and runs on Windows only.");
    ExitCode::FAILURE
}

/// Everything found, printed after the run: nothing here happens on a driver's thread.
#[cfg_attr(not(windows), allow(dead_code))]
fn report(outcome: &Outcome, options: &Options) -> ExitCode {
    println!("ASIO drivers registered on this PC");
    println!();
    for entry in &outcome.entries {
        let mark = if entry.target().is_some() { "->" } else { "  " };
        println!("{mark} {}", entry.key);
        println!("     class id {}", entry.clsid);
        match &entry.dll {
            Ok(dll) => println!("     dll      {dll}"),
            Err(why) => println!("     dll      could not be read: {why}"),
        }
    }
    println!();

    if outcome.targets.is_empty() {
        println!("Nothing to open: no driver on this PC matches what the probe targets.");
        return ExitCode::SUCCESS;
    }

    println!("The probe would open, in this order:");
    for target in &outcome.targets {
        println!("  {} as \"{}\"", target.key, target.target().unwrap_or("unnamed"));
    }
    println!();

    if outcome.listed_only {
        println!("That is all a run without --yes does. Nothing was opened and nothing was started.");
        println!("Add --yes to open them, with your monitors and amplifiers down.");
        return ExitCode::SUCCESS;
    }

    for opened in &outcome.opened {
        print_driver(opened, options);
    }

    if let Some(sizes) = buffer_mismatch(&outcome.opened) {
        println!("Note: the drivers do not agree on a preferred buffer size.");
        for (name, size) in sizes {
            println!("  {name}: {size} samples");
        }
        println!("  The probe opened each at its own size. An aggregate driver will need them matched.");
        println!();
    }

    if let Some(failure) = &outcome.failure {
        println!("It did not get through.");
        println!("  {} failed at {}", failure.driver, failure.step.call());
        println!("  the driver said: {}", failure.message);
        println!("  drivers already open at that moment: {}", failure.already_open);
        println!();
        println!("That is an answer, not a crash: everything opened was stopped and released.");
        return ExitCode::FAILURE;
    }

    println!("Both instantiated and started together: {}", yes_no(outcome.all_started()));
    println!("Ran for {:.3} s", outcome.elapsed.as_secs_f64());
    println!();
    for run in &outcome.runs {
        println!(
            "  {:<8} {:>8} callbacks  {:>9.2} per second  {:>12} samples at {} per buffer",
            run.name,
            run.callbacks,
            rate_of(run, outcome.elapsed),
            run.samples(),
            run.buffer_size
        );
    }
    println!();

    match drift(&outcome.runs, outcome.elapsed) {
        Some(drift) => {
            println!("{} minus {}:", drift.a, drift.b);
            println!("  {:+} callbacks ({:+.4} per second)", drift.callbacks, drift.callbacks_per_second);
            println!("  {:+} samples ({:+.3} per second)", drift.samples, drift.samples_per_second);
            match drift.verdict {
                Verdict::InStep => println!("  In step. The two devices are running off one clock."),
                Verdict::Drifting => {
                    println!("  Drifting. The two devices are not running off one clock, so an aggregate");
                    println!("  of them would need resampling or a corrected cable and clock setting.");
                }
            }
        }
        None => println!("One driver ran, so there is nothing to compare."),
    }
    ExitCode::SUCCESS
}

#[cfg_attr(not(windows), allow(dead_code))]
fn print_driver(opened: &Opened, options: &Options) {
    let d = &opened.description;
    println!("{} (opened as \"{}\")", d.name, opened.name);
    println!("  driver version   {}", d.version);
    println!("  channels         {} in, {} out", d.inputs, d.outputs);
    println!(
        "  buffer sizes     min {}, max {}, preferred {}, granularity {}",
        d.buffers.min, d.buffers.max, d.buffers.preferred, d.buffers.granularity
    );
    println!("  sample rate      {} Hz", d.rate);
    println!("  latency          {} in, {} out (samples)", d.latency_in, d.latency_out);
    println!("  channel 0 in     {}", d.input_format);
    println!("  channel 0 out    {}", d.output_format);
    if let (Some(hz), Some(can)) = (options.rate, opened.can_rate) {
        println!("  can do {hz} Hz    {}", yes_no(can));
    }
    if d.clocks.is_empty() {
        println!("  clock sources    none reported");
    } else {
        for clock in &d.clocks {
            let mark = if clock.current { " (current)" } else { "" };
            println!("  clock source {:<3} {}{mark}", clock.index, clock.name);
        }
    }
    println!();
}

#[cfg_attr(not(windows), allow(dead_code))]
fn yes_no(yes: bool) -> &'static str {
    if yes {
        "yes"
    } else {
        "no"
    }
}
