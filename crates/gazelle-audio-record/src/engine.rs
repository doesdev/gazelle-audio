//! **The one session the recorder and the metronome share**, and who is holding it.
//!
//! ```text
//!   metronome only     open ------------------------------------ close
//!   recorder only      open ------------------------------------ close
//!   both               open --(metronome)--+--(recorder joins)--+--(metronome stops)-- close
//! ```
//!
//! Either of them opens the aggregate when it needs it and nothing has it open; the other then
//! joins the session already running, with no restart and no gap: the session was opened with every
//! input and the metronome's outputs, so arming only hangs a tap on the callback, and starting the
//! metronome only tells its generator to play. Stopping one leaves the other running. The session
//! closes when neither holds it.
//!
//! The session lives on a thread of its own (the *host* thread), which holds the calibration's turn
//! and the drivers' apartment for as long as the session is open, as a measurement's thread does.
//!
//! **The metronome's outputs are fixed while the session is open**: they are buffers the aggregate
//! made when it opened, and asking for others means closing it. So a change of outputs is refused
//! while it is open ([`Engine::set_outputs`]), with the sentence that says what to do.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use gazelle_aggregate::config::Config;
use gazelle_aggregate::status::Glitches;
use gazelle_calibrate::Pick;

use crate::host::{OpenRequest, Opened, Session, Shared};
use crate::latency::LatencySlot;
use crate::metronome::Params;
use crate::recorder::Environment;
use crate::sim::Timing;

/// Who holds the session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum User {
    Recorder,
    Metronome,
}

/// What a holder of the session gets.
#[derive(Clone)]
pub struct Lease {
    pub shared: Arc<Shared>,
    pub opened: Opened,
    pub dropouts: Arc<Mutex<Vec<Glitches>>>,
    /// The latencies the aggregate reports, as of the host thread's last look.
    pub latency: LatencySlot,
}

/// How long opening waits for the drivers.
const OPEN_TIMEOUT: Duration = Duration::from_secs(30);
/// How often the host thread reads the counters and the log.
const HOUSEKEEPING: Duration = Duration::from_millis(50);

/// What the session is opened with, besides the aggregate's setup.
struct Setup {
    outputs: Vec<Pick>,
    params: Params,
}

struct Open {
    recorder: bool,
    metronome: bool,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    /// The outputs it was opened with.
    outputs: Vec<Pick>,
}

/// The shared session. One per server.
pub struct Engine {
    env: Arc<dyn Environment>,
    setup: Mutex<Setup>,
    /// Taken for the whole of an open or a close, so they happen one at a time.
    open: Mutex<Option<Open>>,
    /// What is open now, read without waiting behind an open.
    current: Mutex<Option<Lease>>,
    /// How many sessions have been opened, for the tests to tell a join from a reopen.
    opened_count: std::sync::atomic::AtomicU64,
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Engine {
    pub fn new(env: Arc<dyn Environment>) -> Engine {
        Engine {
            env,
            setup: Mutex::new(Setup { outputs: Vec::new(), params: Params::default() }),
            open: Mutex::new(None),
            current: Mutex::new(None),
            opened_count: std::sync::atomic::AtomicU64::new(0),
        }
    }

    pub fn env(&self) -> &Arc<dyn Environment> {
        &self.env
    }

    /// The session open now, if one is.
    pub fn lease(&self) -> Option<Lease> {
        locked(&self.current).clone()
    }

    pub fn is_open(&self) -> bool {
        locked(&self.current).is_some()
    }

    /// Whether `user` holds the session.
    pub fn holds(&self, user: User) -> bool {
        match self.open.try_lock() {
            Ok(open) => open.as_ref().is_some_and(|open| match user {
                User::Recorder => open.recorder,
                User::Metronome => open.metronome,
            }),
            // Something is being opened or closed: nobody may assume they hold it yet.
            Err(_) => false,
        }
    }

    pub fn sessions_opened(&self) -> u64 {
        self.opened_count.load(Ordering::Acquire)
    }

    /// The metronome's musical choices, now and for every session opened from here on.
    pub fn set_params(&self, params: Params) {
        locked(&self.setup).params = params;
        if let Some(lease) = self.lease() {
            lease.shared.generator.set_params(params);
        }
    }

    pub fn params(&self) -> Params {
        locked(&self.setup).params
    }

    /// The metronome's outputs. Refused while a session is open with other ones.
    pub fn set_outputs(&self, outputs: Vec<Pick>) -> Result<(), String> {
        let open = locked(&self.open);
        if let Some(open) = open.as_ref() {
            if open.outputs != outputs {
                return Err(OUTPUTS_FIXED.into());
            }
        }
        locked(&self.setup).outputs = outputs;
        Ok(())
    }

    pub fn outputs(&self) -> Vec<Pick> {
        locked(&self.setup).outputs.clone()
    }

    /// **Hold the session** for `user`, opening it with `config` if nothing has it open. Answers
    /// once it is open, or with the sentence that says why not.
    pub fn acquire(&self, user: User, config: &Config, source: &str) -> Result<Lease, String> {
        let mut open = locked(&self.open);
        if open.is_none() {
            let setup = locked(&self.setup);
            let request = OpenRequest { config: config.clone(), source: source.to_string(), outputs: setup.outputs.clone(), params: setup.params };
            drop(setup);
            let (lease, stop, thread) = self.start(request.clone())?;
            self.opened_count.fetch_add(1, Ordering::AcqRel);
            *locked(&self.current) = Some(lease);
            *open = Some(Open { recorder: false, metronome: false, stop, thread: Some(thread), outputs: request.outputs });
        }
        let held = open.as_mut().expect("opened above");
        match user {
            User::Recorder => held.recorder = true,
            User::Metronome => held.metronome = true,
        }
        Ok(self.lease().expect("open while held"))
    }

    /// **Let go** for `user`. The session closes, and the drivers are let go, once neither holds it.
    pub fn release(&self, user: User) {
        let mut open = locked(&self.open);
        let Some(held) = open.as_mut() else { return };
        match user {
            User::Recorder => held.recorder = false,
            User::Metronome => held.metronome = false,
        }
        if held.recorder || held.metronome {
            return;
        }
        let mut closing = open.take().expect("checked above");
        closing.stop.store(true, Ordering::Release);
        if let Some(thread) = closing.thread.take() {
            let _ = thread.join();
        }
        *locked(&self.current) = None;
    }

    fn start(&self, request: OpenRequest) -> Result<(Lease, Arc<AtomicBool>, JoinHandle<()>), String> {
        let env = Arc::clone(&self.env);
        let stop = Arc::new(AtomicBool::new(false));
        let (reply, answer) = mpsc::channel::<Result<Lease, String>>();
        let stopping = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("gazelle-audio-host".into())
            .spawn(move || host_thread(env, request, reply, stopping))
            .map_err(|why| format!("the audio host could not start: {why}"))?;
        match answer.recv_timeout(OPEN_TIMEOUT) {
            Ok(Ok(lease)) => Ok((lease, stop, thread)),
            Ok(Err(why)) => {
                let _ = thread.join();
                Err(why)
            }
            Err(_) => {
                stop.store(true, Ordering::Release);
                Err("the interfaces did not open within 30 seconds, so Gazelle gave up: another program may be holding them".into())
            }
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.release(User::Recorder);
        self.release(User::Metronome);
    }
}

/// Why the outputs cannot change now.
pub const OUTPUTS_FIXED: &str = "the metronome's outputs are fixed while the interfaces are open: stop the metronome and disarm, then change them";

/// The host thread: opens the session, answers, pumps until told to stop, and closes.
fn host_thread(env: Arc<dyn Environment>, request: OpenRequest, reply: mpsc::Sender<Result<Lease, String>>, stop: Arc<AtomicBool>) {
    // A vendor driver is a COM object, and this thread is ours, so it says so first, as a
    // measurement's thread does.
    #[cfg(windows)]
    let _com = gazelle_calibrate::session::Apartment::enter();
    let host = match env.host() {
        Ok(host) => host,
        Err(why) => {
            let _ = reply.send(Err(why));
            return;
        }
    };
    let mut session = match Session::open(host, &request, env.reporter()) {
        Ok(session) => session,
        Err(why) => {
            let _ = reply.send(Err(why));
            return;
        }
    };
    let mut pump = env.pump(Timing { rate: session.opened.rate, block: session.opened.block }, session.opened.master);
    let lease = Lease { shared: session.shared(), opened: session.opened.clone(), dropouts: session.dropouts(), latency: session.latency() };
    if reply.send(Ok(lease)).is_err() {
        // Opening was given up on: nobody wants this session.
        return;
    }
    let mut index = 0usize;
    let mut looked = Instant::now();
    while !stop.load(Ordering::Acquire) {
        if !pump(index) {
            break;
        }
        index = index.wrapping_add(1);
        if looked.elapsed() >= HOUSEKEEPING {
            looked = Instant::now();
            session.housekeeping();
        }
    }
    session.close();
}

/// At the hardware the devices' own threads call back, and the host thread only waits.
pub fn wait_for_the_devices() -> Box<dyn FnMut(usize) -> bool + Send> {
    Box::new(|_| {
        std::thread::sleep(Duration::from_millis(2));
        true
    })
}
