//! The watcher: one thread that waits for Gazelle to say the configuration has changed, and
//! re-plans when it does.
//!
//! **It is never the audio thread.** Nothing here runs on a callback, nothing here is called from
//! one, and the audio path does not know it exists. The watcher reads a file, builds a plan and
//! takes a lock, all of which are things the audio path will not do.
//!
//! The rules, which are the whole of what this module decides:
//!
//! - **A configuration that does not parse, or that names a device this PC does not have, is
//!   refused.** What is in force stays in force, the reason goes into the record and the event log,
//!   and the DAW is left alone. A DAW is never handed a broken plan.
//! - **If nothing is streaming, the new plan is simply adopted**, quietly, there and then.
//! - **If a DAW is streaming, the host is asked to reset**, which is the message a vendor driver
//!   sends when its own settings change, and the new plan is taken up when the DAW comes back
//!   through `createBuffers`. Audio drops for a moment exactly as a buffer size change does.
//! - **The thread exits cleanly**: when it is asked to stop, when the driver object it watches has
//!   gone, or when the thing it waits on has been closed underneath it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

use gazelle_audio_aggregate_status::map::{Wake, Waiter};
use gazelle_audio_stream_abi::raw::selector;

use crate::aggregate::Aggregate;
use crate::config::Config;
use crate::status::{GlitchWatch, PhaseWatch, Reporter, StallWatch};

/// How long a wait lasts when nobody signals. The watcher looks around on a timeout as well as on
/// a signal, which is how a device that stalled gets its line in the event log.
pub const LOOK_EVERY_MS: u32 = 250;

/// What one reload turned into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Nothing was streaming, so the new plan is in force now.
    Adopted,
    /// A DAW was streaming, so the host was asked to reset. The plan is taken up when it comes
    /// back for buffers.
    ResetAsked,
    /// It was refused, and what was in force stays in force.
    Refused,
}

/// What the watcher can do to the driver. Behind a trait so that every rule above is decided
/// against a driver made of data.
pub trait Reload: Send + Sync {
    /// The generation Gazelle has asked for.
    fn generation(&self) -> u64;
    /// Read the configuration file again.
    fn read_config(&self) -> Result<(Config, String), String>;
    /// Whether this configuration could be run at all, without opening anything: it parses, and
    /// every device it names is on this PC.
    fn check(&self, config: &Config) -> Result<(), String>;
    /// Whether a DAW has the driver open with its buffers made.
    fn is_streaming(&self) -> bool;
    /// Take it now. Only called when nothing is streaming.
    fn adopt(&self, config: Config, source: String, generation: u64) -> Result<(), String>;
    /// Take it at the next `createBuffers`. Only called when a DAW is streaming.
    fn queue(&self, config: Config, source: String, generation: u64);
    /// Ask the DAW to put the driver down and pick it up again.
    fn ask_host_to_reset(&self);
    fn refused(&self, generation: u64, why: &str);
    fn adopted(&self, generation: u64, source: &str);
    /// Everything that is noticed rather than asked for: a device that has stalled, or come back.
    fn look_around(&self);
    /// False once the driver this watches has gone.
    fn is_alive(&self) -> bool;
}

/// One reload, start to finish. This is the whole of the policy, and it is pure: everything it
/// touches is behind [`Reload`].
pub fn reload(target: &dyn Reload, generation: u64) -> Outcome {
    let (config, source) = match target.read_config() {
        Ok(read) => read,
        Err(why) => {
            target.refused(generation, &why);
            return Outcome::Refused;
        }
    };
    if let Err(why) = target.check(&config) {
        target.refused(generation, &why);
        return Outcome::Refused;
    }
    if target.is_streaming() {
        // The DAW is holding a plan. It gets the new one when it comes back for buffers, and the
        // way to make it come back is the message a vendor driver sends when its settings move.
        target.queue(config, source, generation);
        target.ask_host_to_reset();
        return Outcome::ResetAsked;
    }
    match target.adopt(config, source.clone(), generation) {
        Ok(()) => {
            target.adopted(generation, &source);
            Outcome::Adopted
        }
        Err(why) => {
            target.refused(generation, &why);
            Outcome::Refused
        }
    }
}

/// The loop the thread runs. It returns, rather than being killed, on every way out.
pub fn watch(waiter: &dyn Waiter, target: &dyn Reload, stop: &AtomicBool) {
    let mut seen = target.generation();
    loop {
        if stop.load(Ordering::Acquire) || !target.is_alive() {
            return;
        }
        if waiter.wait(LOOK_EVERY_MS) == Wake::Closed {
            return;
        }
        if stop.load(Ordering::Acquire) || !target.is_alive() {
            return;
        }
        target.look_around();
        let now = target.generation();
        if now != seen {
            seen = now;
            reload(target, now);
        }
    }
}

/// The real driver, as the watcher sees it.
///
/// It holds a **weak** reference: the watcher must never be the reason a driver object stays
/// alive, and a DAW that released the driver while the thread was between waits finds nothing here
/// and the thread goes home.
pub struct DriverWatch {
    driver: Weak<Mutex<Aggregate>>,
    reporter: Arc<Reporter>,
    stalls: Mutex<StallWatch>,
    glitches: Mutex<GlitchWatch>,
    phases: Mutex<PhaseWatch>,
}

impl DriverWatch {
    pub fn new(driver: &Arc<Mutex<Aggregate>>, reporter: Arc<Reporter>) -> DriverWatch {
        DriverWatch {
            driver: Arc::downgrade(driver),
            reporter,
            stalls: Mutex::new(StallWatch::new()),
            glitches: Mutex::new(GlitchWatch::new()),
            phases: Mutex::new(PhaseWatch::new()),
        }
    }

    /// Do something with the driver, if it is still there. The lock is never held across a call
    /// into the DAW: that is what deadlocks a host that is calling us at the same time.
    fn with<R>(&self, what: impl FnOnce(&mut Aggregate) -> R) -> Option<R> {
        let driver = self.driver.upgrade()?;
        let mut aggregate = driver.lock().ok()?;
        Some(what(&mut aggregate))
    }
}

impl Reload for DriverWatch {
    fn generation(&self) -> u64 {
        self.reporter.generation()
    }

    fn read_config(&self) -> Result<(Config, String), String> {
        match crate::config::config_path() {
            Some(path) => Config::read(&path).map(|config| (config, path.display().to_string())),
            None => Err("there is no APPDATA on this machine, so there is no configuration to read".to_string()),
        }
    }

    fn check(&self, config: &Config) -> Result<(), String> {
        self.with(|aggregate| aggregate.check(config)).unwrap_or(Ok(()))
    }

    fn is_streaming(&self) -> bool {
        self.with(|aggregate| aggregate.has_buffers()).unwrap_or(false)
    }

    fn adopt(&self, config: Config, source: String, generation: u64) -> Result<(), String> {
        match self.with(|aggregate| aggregate.reconfigure(config, source, generation)) {
            Some(answer) => answer,
            None => Err("the driver was released while its configuration was being read".to_string()),
        }
    }

    fn queue(&self, config: Config, source: String, generation: u64) {
        self.with(|aggregate| aggregate.queue(config, source, generation));
    }

    fn ask_host_to_reset(&self) {
        // Take the stream out from under the lock and let go of it before calling the DAW: the
        // host may be inside one of our own calls on another thread this instant.
        let stream = self.with(|aggregate| aggregate.stream().cloned()).flatten();
        if let Some(stream) = stream {
            stream.forward(selector::RESET_REQUEST, 0);
            self.reporter.note(gazelle_audio_aggregate_status::events::Event::ResetAsked, "the settings changed");
        }
    }

    fn refused(&self, generation: u64, why: &str) {
        self.reporter.refused(generation, why);
    }

    fn adopted(&self, generation: u64, source: &str) {
        self.reporter.note(gazelle_audio_aggregate_status::events::Event::Adopted, source);
        self.reporter.update(|area| area.generation_in_force = generation);
        self.stalls.lock().expect("not poisoned").clear();
        self.glitches.lock().expect("not poisoned").clear();
        self.phases.lock().expect("not poisoned").clear();
    }

    fn look_around(&self) {
        let Some(area) = self.reporter.snapshot() else { return };
        let count = (area.device_count as usize).min(area.devices.len());
        let now: Vec<bool> = area.devices[..count].iter().map(|device| device.stalled != 0).collect();
        let changes = self.stalls.lock().expect("not poisoned").changes(&now);
        for (index, stalled) in changes {
            self.reporter.stall_changed(area.devices[index].name.get(), stalled);
        }

        // The audio path counted these; deciding whether one of them is worth a line is this
        // thread's work, and only the first of a session ever is.
        let lost: Vec<(u64, u64)> = area.devices[..count].iter().map(|device| (device.dropped, device.starved)).collect();
        let first = self.glitches.lock().expect("not poisoned").first(&lost);
        for (index, dropped, starved) in first {
            self.reporter.first_glitch(area.devices[index].name.get(), dropped, starved);
        }

        // And the same again for a phase measurement that has just come to something. The audio
        // path measured it and wrote three numbers; this is where they become the line that
        // survives the session.
        let states: Vec<u32> = area.devices[..count].iter().map(|device| device.phase_state).collect();
        let settled = self.phases.lock().expect("not poisoned").settled(&states);
        for index in settled {
            let device = &area.devices[index];
            self.reporter.phase_settled(device.name.get(), device.phase_state, device.phase_measured, device.phase_applied);
        }
    }

    fn is_alive(&self) -> bool {
        self.driver.strong_count() > 0
    }
}

/// The thread itself, and the two things it takes to stop it cleanly.
pub struct Watcher {
    stop: Arc<AtomicBool>,
    /// Signalled to wake the thread out of its wait so that it can notice it has been stopped.
    wake: Arc<dyn WakeUp>,
    thread: Option<std::thread::JoinHandle<()>>,
}

/// Something that can be poked to end a wait early. The real one is the named event; this is
/// separate from [`Waiter`] because the stopping end holds it while the waiting end is inside a
/// wait.
pub trait WakeUp: Send + Sync {
    fn poke(&self);
}

impl Watcher {
    /// Start watching. The thread owns the waiter and the target; everything it needs to be
    /// stopped is here.
    pub fn start(waiter: Box<dyn Waiter>, target: Box<dyn Reload>, wake: Arc<dyn WakeUp>) -> Watcher {
        let stop = Arc::new(AtomicBool::new(false));
        let mine = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("gazelle aggregate watcher".to_string())
            .spawn(move || watch(waiter.as_ref(), target.as_ref(), &mine))
            .ok();
        Watcher { stop, wake, thread }
    }

    /// Ask the thread to stop, wake it so that it notices, and wait for it. Called before anything
    /// the thread can reach is dropped.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.wake.poke();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DeviceConfig;
    use std::sync::atomic::AtomicU64;

    /// A driver made of data: it records what was done to it and answers what a test told it to.
    struct Fake {
        generation: AtomicU64,
        streaming: AtomicBool,
        alive: AtomicBool,
        /// What `read_config` answers.
        config: Mutex<Result<(Config, String), String>>,
        /// What `check` answers.
        objection: Mutex<Option<String>>,
        /// What `adopt` answers.
        adoption: Mutex<Option<String>>,
        done: Mutex<Vec<String>>,
        /// What is in force, which a refusal must leave alone.
        in_force: Mutex<String>,
    }

    fn named(key: &str) -> Config {
        Config {
            devices: vec![DeviceConfig { key: Some(key.to_string()), ..DeviceConfig::default() }],
            ..Config::default()
        }
    }

    impl Fake {
        fn new() -> Arc<Fake> {
            Arc::new(Fake {
                generation: AtomicU64::new(0),
                streaming: AtomicBool::new(false),
                alive: AtomicBool::new(true),
                config: Mutex::new(Ok((named("Device B"), "a test".to_string()))),
                objection: Mutex::new(None),
                adoption: Mutex::new(None),
                done: Mutex::new(Vec::new()),
                in_force: Mutex::new("Device A".to_string()),
            })
        }

        fn did(&self) -> Vec<String> {
            self.done.lock().expect("not poisoned").clone()
        }

        fn note(&self, what: &str) {
            self.done.lock().expect("not poisoned").push(what.to_string());
        }
    }

    impl Reload for Fake {
        fn generation(&self) -> u64 {
            self.generation.load(Ordering::Acquire)
        }

        fn read_config(&self) -> Result<(Config, String), String> {
            match &*self.config.lock().expect("not poisoned") {
                Ok((config, source)) => Ok((config.clone(), source.clone())),
                Err(why) => Err(why.clone()),
            }
        }

        fn check(&self, _config: &Config) -> Result<(), String> {
            match self.objection.lock().expect("not poisoned").clone() {
                Some(why) => Err(why),
                None => Ok(()),
            }
        }

        fn is_streaming(&self) -> bool {
            self.streaming.load(Ordering::Acquire)
        }

        fn adopt(&self, config: Config, _source: String, _generation: u64) -> Result<(), String> {
            if let Some(why) = self.adoption.lock().expect("not poisoned").clone() {
                self.note("adopt refused");
                return Err(why);
            }
            self.note("adopt");
            *self.in_force.lock().expect("not poisoned") =
                config.devices.first().and_then(|d| d.key.clone()).unwrap_or_default();
            Ok(())
        }

        fn queue(&self, _config: Config, _source: String, _generation: u64) {
            self.note("queue");
        }

        fn ask_host_to_reset(&self) {
            self.note("reset");
        }

        fn refused(&self, generation: u64, why: &str) {
            self.note(&format!("refused {generation}: {why}"));
        }

        fn adopted(&self, generation: u64, _source: &str) {
            self.note(&format!("adopted {generation}"));
        }

        fn look_around(&self) {
            self.note("look");
        }

        fn is_alive(&self) -> bool {
            self.alive.load(Ordering::Acquire)
        }
    }

    /// A wait that answers from a script, and can move the generation as Gazelle would just
    /// before it wakes the thread. Once the script runs out it says the event has been closed,
    /// which is how every one of these tests ends.
    struct Script {
        answers: Mutex<Vec<Wake>>,
        /// What the generation becomes before each wake.
        moves: Mutex<Vec<u64>>,
        asking: Option<Arc<Fake>>,
    }

    impl Script {
        fn of(answers: &[Wake]) -> Script {
            Script { answers: Mutex::new(answers.iter().rev().copied().collect()), moves: Mutex::new(Vec::new()), asking: None }
        }

        /// The same, with Gazelle bumping the generation to each of `moves` in turn.
        fn asking(answers: &[Wake], fake: &Arc<Fake>, moves: &[u64]) -> Script {
            Script {
                answers: Mutex::new(answers.iter().rev().copied().collect()),
                moves: Mutex::new(moves.iter().rev().copied().collect()),
                asking: Some(Arc::clone(fake)),
            }
        }
    }

    impl Waiter for Script {
        fn wait(&self, _millis: u32) -> Wake {
            if let Some(fake) = &self.asking {
                if let Some(to) = self.moves.lock().expect("not poisoned").pop() {
                    fake.generation.store(to, Ordering::Release);
                }
            }
            self.answers.lock().expect("not poisoned").pop().unwrap_or(Wake::Closed)
        }
    }

    #[test]
    fn a_change_with_nothing_streaming_is_taken_up_quietly() {
        let fake = Fake::new();
        assert_eq!(reload(fake.as_ref(), 1), Outcome::Adopted);
        assert_eq!(fake.did(), vec!["adopt", "adopted 1"]);
        assert_eq!(*fake.in_force.lock().unwrap(), "Device B");
    }

    #[test]
    fn a_change_while_a_daw_is_streaming_asks_the_host_to_reset_exactly_once() {
        let fake = Fake::new();
        fake.streaming.store(true, Ordering::Release);
        assert_eq!(reload(fake.as_ref(), 4), Outcome::ResetAsked);
        assert_eq!(fake.did(), vec!["queue", "reset"], "queued first, so the plan is there when it comes back");
        assert_eq!(fake.did().iter().filter(|what| *what == "reset").count(), 1);
        assert_eq!(*fake.in_force.lock().unwrap(), "Device A", "nothing has changed under the DAW yet");
    }

    #[test]
    fn a_change_with_nothing_streaming_never_asks_the_host_for_anything() {
        let fake = Fake::new();
        reload(fake.as_ref(), 1);
        assert!(!fake.did().iter().any(|what| what == "reset"), "{:?}", fake.did());
        assert!(!fake.did().iter().any(|what| what == "queue"));
    }

    #[test]
    fn a_configuration_that_does_not_parse_is_refused_and_the_plan_in_force_stays() {
        let fake = Fake::new();
        *fake.config.lock().unwrap() = Err("aggregate.json is not valid: expected a comma".to_string());
        assert_eq!(reload(fake.as_ref(), 2), Outcome::Refused);
        assert_eq!(fake.did(), vec!["refused 2: aggregate.json is not valid: expected a comma"]);
        assert_eq!(*fake.in_force.lock().unwrap(), "Device A");
    }

    #[test]
    fn a_configuration_that_names_no_device_on_this_pc_is_refused_before_anything_is_touched() {
        let fake = Fake::new();
        *fake.objection.lock().unwrap() =
            Some("Device C is in the configuration but no driver on this PC matches it".to_string());
        assert_eq!(reload(fake.as_ref(), 3), Outcome::Refused);
        assert_eq!(fake.did().len(), 1, "nothing was adopted, queued or reset: {:?}", fake.did());
        assert!(fake.did()[0].contains("Device C"));
        assert_eq!(*fake.in_force.lock().unwrap(), "Device A");
    }

    #[test]
    fn a_configuration_that_a_device_refuses_when_it_is_opened_leaves_the_old_one_in_force() {
        let fake = Fake::new();
        *fake.adoption.lock().unwrap() = Some("Device B refused 96000 Hz".to_string());
        assert_eq!(reload(fake.as_ref(), 5), Outcome::Refused);
        assert_eq!(fake.did(), vec!["adopt refused", "refused 5: Device B refused 96000 Hz"]);
        assert_eq!(*fake.in_force.lock().unwrap(), "Device A");
    }

    #[test]
    fn the_watcher_reloads_once_per_generation_and_looks_around_on_every_wake() {
        let fake = Fake::new();
        let script = Script::of(&[Wake::TimedOut, Wake::Signalled, Wake::TimedOut, Wake::Closed]);
        let stop = AtomicBool::new(false);
        watch(&script, fake.as_ref(), &stop);
        // Nobody asked for anything, so nothing was reloaded, and it looked around each time.
        assert_eq!(fake.did(), vec!["look", "look", "look"]);
    }

    #[test]
    fn a_generation_that_has_moved_is_reloaded_once_and_not_again() {
        let fake = Fake::new();
        // Gazelle asks once, before the first wake, and never again. The second wake finds the
        // same number and must do nothing with it.
        let script = Script::asking(&[Wake::Signalled, Wake::Signalled, Wake::Closed], &fake, &[7, 7]);
        let stop = AtomicBool::new(false);
        watch(&script, fake.as_ref(), &stop);
        let done = fake.did();
        assert_eq!(done.iter().filter(|what| *what == "adopt").count(), 1, "{done:?}");
        assert_eq!(done.iter().filter(|what| *what == "look").count(), 2);
    }

    #[test]
    fn a_generation_that_moves_twice_is_reloaded_twice() {
        let fake = Fake::new();
        let script = Script::asking(&[Wake::Signalled, Wake::Signalled, Wake::Closed], &fake, &[1, 2]);
        watch(&script, fake.as_ref(), &AtomicBool::new(false));
        assert_eq!(fake.did().iter().filter(|what| *what == "adopt").count(), 2, "{:?}", fake.did());
    }

    #[test]
    fn a_generation_that_was_already_moved_before_the_watcher_started_is_not_a_change() {
        // The driver read the file at init, so the number that was there when the thread started
        // is the number in force. Reloading it again would be a needless re-plan.
        let fake = Fake::new();
        fake.generation.store(7, Ordering::Release);
        watch(&Script::of(&[Wake::Signalled, Wake::Closed]), fake.as_ref(), &AtomicBool::new(false));
        assert_eq!(fake.did(), vec!["look"]);
    }

    #[test]
    fn the_watcher_goes_home_when_it_is_stopped_when_the_driver_goes_and_when_the_event_closes() {
        // Stopped.
        let fake = Fake::new();
        let stop = AtomicBool::new(true);
        watch(&Script::of(&[Wake::Signalled]), fake.as_ref(), &stop);
        assert!(fake.did().is_empty(), "it never even waited");

        // The driver object went away.
        let gone = Fake::new();
        gone.alive.store(false, Ordering::Release);
        watch(&Script::of(&[Wake::Signalled]), gone.as_ref(), &AtomicBool::new(false));
        assert!(gone.did().is_empty());

        // The thing it waits on was closed underneath it.
        let closed = Fake::new();
        watch(&Script::of(&[Wake::Closed]), closed.as_ref(), &AtomicBool::new(false));
        assert!(closed.did().is_empty());
    }

    #[test]
    fn a_reload_a_reader_asked_for_while_the_driver_is_going_away_is_refused_rather_than_half_done() {
        let fake = Fake::new();
        fake.alive.store(false, Ordering::Release);
        // `reload` itself does not check aliveness: the loop does, before and after every wait, so
        // the last thing a dying driver can do is nothing.
        let stop = AtomicBool::new(false);
        watch(&Script::of(&[Wake::Signalled, Wake::Closed]), fake.as_ref(), &stop);
        assert!(fake.did().is_empty());
    }
}
