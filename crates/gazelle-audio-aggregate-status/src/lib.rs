//! **The wire format between the Gazelle Aggregate driver and Gazelle.**
//!
//! The driver runs inside whatever process opened it, which is a DAW, and it has to work with
//! Gazelle closed: its configuration file is its only requirement. When Gazelle *is* running, the
//! two need to say things to each other, and this crate is the only place either of them learns
//! how. Neither depends on the other; both depend on this.
//!
//! There are three things here, and they are deliberately different shapes.
//!
//! 1. **Live status is a shared memory record**, [`record::StatusRecord`]. It is fixed size, has no
//!    pointers and allocates nothing, so a reader maps it and reads it without trusting a word of
//!    the writer's memory. Writing it from the audio path is a handful of atomic stores and no
//!    syscall at all. A file would have been worse in every way that matters here: a periodic write
//!    from an audio process is at the mercy of antivirus and of the filesystem's own metadata
//!    churn, and it caps how often anything can be reported.
//! 2. **Durable events are a small log file**, whose line format is [`events`]. One line is
//!    appended when something happens that a person may want to read about tomorrow: a refusal, a
//!    stall and its recovery, a session starting and ending. Nothing is ever written on a timer, so
//!    the file is small and every line in it means something. It survives the driver exiting, which
//!    is exactly when someone wants to know why last night's session would not start.
//! 3. **Gazelle asks for a reload** by writing the configuration file, bumping
//!    [`record::ControlArea::generation`] and signalling the named event. The driver's watcher
//!    thread, never its audio thread, notices and re-plans.
//!
//! # Who writes what
//!
//! The record has two areas and they have one writer each. **The driver writes only
//! [`record::DriverArea`]. Gazelle writes only [`record::ControlArea`].** Neither reads the other's
//! area expecting it to hold still, and neither writes into the other's. The header (the magic
//! number, the format version and the size) is written once, by whoever creates the section, and
//! then never again.
//!
//! # The seqlock
//!
//! The driver's area is bigger than any one atomic, so it is published under a sequence counter.
//! The writer makes the counter **odd** before it touches the area and **even** again afterwards. A
//! reader takes the counter, copies the area, takes the counter again, and accepts what it copied
//! only if the counter was even both times and did not change. If it did, the reader tries again;
//! after [`read::ATTEMPTS`] tries it gives up and says the writer is busy rather than handing back
//! something half old. No reader ever blocks a writer, and no writer ever waits for a reader, which
//! is what makes this safe to do from an audio callback.
//!
//! There is a second, much smaller rule for the writer's own side: the driver has two threads that
//! write the record (the audio thread every block, and the watcher thread when the plan or a
//! refusal changes), so [`publish::Publisher`] holds a one word gate. The watcher waits for it. The
//! audio thread never does: if it cannot take the gate it skips that block's update and writes the
//! next one instead. Losing one block's worth of counters is not worth a moment of jitter.

pub mod events;
pub mod map;
pub mod names;
pub mod publish;
pub mod read;
pub mod record;

pub use events::{Event, EventLine};
pub use map::{Mapping, Scratch};
pub use publish::Publisher;
pub use read::{Reader, Snapshot, StatusError};
pub use record::{ControlArea, DeviceStatus, DriverArea, StatusRecord, FORMAT_VERSION, MAGIC, MAX_DEVICES};

#[cfg(windows)]
pub mod windows;
