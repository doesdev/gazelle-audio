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

use probe::{buffer_mismatch, clocks, drift, fitted_clocks, gap, rate_of, Opened, Options, Outcome, Verdict, NO_HARDWARE};

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
    /// Move every driver to this rate before it streams, as a DAW does. The probe's only write.
    #[arg(long, value_name = "HZ")]
    set_rate: Option<f64>,
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

    let options = Options { proceed: cli.yes, seconds: cli.seconds, only: cli.only, rate: cli.rate, set_rate: cli.set_rate };
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

    let together = if outcome.targets.len() > 1 { "Both instantiated and started together" } else { "Instantiated and started" };
    println!("{together}: {}", yes_no(outcome.all_started()));
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

    // The plainest measurement of the two, and the one to believe over a long run: the drivers are
    // read together, so their counts can simply be subtracted. No timestamps, no fitting. A count
    // moves a buffer at a time, so a gap only shows once the clocks have pulled a buffer apart:
    // at one sample a second and 512 samples to a buffer, that is eight minutes.
    if let Some(gap) = gap(&outcome.series) {
        println!("The gap between the two sample counts, read together {} times over {:.1} s:", outcome.series.first().map_or(0, |s| s.points.len()), gap.seconds);
        println!("  started at    {} samples", gap.first);
        println!("  ended         {:+} samples from there (widest {:+})", gap.last, gap.widest);
        println!("  growing at    {:+.3} samples per second", gap.samples_per_second);
        if gap.last == 0 && gap.widest == 0 {
            println!("  The counts never parted. Either one clock feeds both, or the run was too");
            println!("  short for a difference to reach a whole buffer: {} samples at this rate.", 512);
        }
        println!();
    }

    // The clocks as a line fitted through readings taken all through the run. A single reading is
    // quantised to a buffer and to a millisecond, so this is the measurement to believe; the two
    // endpoint figures below are kept because they show that quantising for what it is.
    if let Some(fitted) = fitted_clocks(&outcome.series) {
        println!("The two clocks, fitted through {} readings each:", fitted.readings);
        for (name, rate) in fitted.names.iter().zip(fitted.rates.iter()) {
            println!("  {name:<12} {rate:.3} samples per second");
        }
        println!("  difference    {:+.3} samples per second ({:+.2} parts per million)", fitted.samples_per_second, fitted.parts_per_million);
        match fitted.verdict {
            Verdict::InStep => {
                println!("  One clock. The two devices keep the same sample count, so an aggregate of");
                println!("  them needs no resampling: only a fixed offset between the two streams.");
            }
            Verdict::Drifting => {
                println!("  Two clocks. They pull apart by {:.1} samples a second, which is about", fitted.samples_per_second.abs());
                println!("  {:.0} samples an hour, so an aggregate would need the cable and the clock", fitted.samples_per_second.abs() * 3600.0);
                println!("  source set, or it would have to resample.");
            }
        }
        println!();
    }

    // The drivers' own sample positions, which measure the two clocks far more finely than counting
    // whole buffers can: a crystal pair tens of parts per million apart moves less than one callback
    // in a run this short, and that shows up here.
    if let Some(clocks) = clocks(&outcome.positions) {
        println!("The same, from the first and last readings only (quantised, kept for comparison):");
        for (name, rate) in clocks.names.iter().zip(clocks.rates.iter()) {
            println!("  {name:<12} {rate:.3} samples per second");
        }
        // The raw readings too: a rate is samples over the driver's own timestamps, so two drivers
        // that time by different clocks would be compared wrongly. These show whether they agree.
        for p in &outcome.positions {
            let (samples, nanos) = (p.last.samples - p.first.samples, p.last.nanos - p.first.nanos);
            println!("    {:<10} {samples} samples over {:.6} s (first {} at {} ns)", p.name, nanos as f64 / 1e9, p.first.samples, p.first.nanos);
        }
        // Samples against samples, which needs no timestamp at all: two positions read microseconds
        // apart, so a locked pair must advance by the same count.
        if let (Some(a), Some(b)) = (outcome.positions.first(), outcome.positions.get(1)) {
            let (da, db) = (a.last.samples - a.first.samples, b.last.samples - b.first.samples);
            let ppm = if db != 0 { (da as f64 / db as f64 - 1.0) * 1e6 } else { 0.0 };
            println!("  samples against samples: {da} and {db}, {:+} apart ({ppm:+.2} parts per million)", da - db);
        }
        println!("  difference    {:+.3} samples per second ({:+.2} parts per million)", clocks.samples_per_second, clocks.parts_per_million);
        println!("  These two rates are quantised by the readings themselves, so read the fit above.");
        println!();
    }

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
