//! The audio path: the only code here that runs on a callback thread.
//!
//! One device's callback drives the DAW. Every other device's audio crosses a ring buffer, one
//! block at a time, and is held back by a delay so that every channel lines up.
//!
//! **What this code will not do, anywhere below this line.** It does not allocate, lock, format a
//! string, log, read a file, or call back into a vendor driver. Everything it needs was worked out
//! and allocated when the buffers were made. The only things shared between threads are the ring
//! buffers, which have one reader and one writer each, and a handful of atomics.
//!
//! **What it does when something goes wrong.** Silence, never a repeat: a ring with nothing in it
//! reads as silence, a device that stops calling back has its inputs read as silence and its
//! outputs muted, and every output buffer is written in full on every callback whatever else
//! happens. Nothing is ever handed to a device without being written first.

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use gazelle_audio_stream_abi::raw::{time_flags, CallbacksRaw, Samples, Time};
use gazelle_audio_stream_abi::sample;

use crate::config::Config;
use crate::delay::Delay;
use crate::plan::{ChannelRef, Plan};
use crate::ring::Ring;
use crate::status::{Glitches, Reporter};
use crate::sub::DeviceBuffers;

/// One of the aggregate's own buffers, the ones the DAW is given pointers into. Two halves, as the
/// interface wants, laid out one after the other.
pub struct DawBuffer {
    /// Taken out of a box, so that the pointers handed to the DAW and the slices used here never
    /// come from a reference to the whole of it.
    data: *mut i32,
    block: usize,
}

impl DawBuffer {
    fn new(block: usize) -> DawBuffer {
        // Silence from the moment it exists: an output buffer is never anything else until
        // something writes to it.
        DawBuffer { data: Box::into_raw(vec![0i32; block * 2].into_boxed_slice()).cast(), block }
    }

    /// The pointer the DAW is given for one half. The allocation behind it does not move for as
    /// long as the buffers exist.
    pub fn pointer(&self, half: usize) -> *mut std::ffi::c_void {
        // Safety: a pointer is taken, not a reference, and the buffer outlives every use of it.
        unsafe { self.data.add(half * self.block).cast() }
    }

    /// One half, for the callback thread. Only ever called from the master's callback.
    #[allow(clippy::mut_from_ref)]
    fn half(&self, half: usize) -> &mut [i32] {
        // Safety: the master's callback is the only thread that touches these between the DAW's
        // own use of them, and the DAW uses them only inside the callback we make.
        unsafe { std::slice::from_raw_parts_mut(self.data.add(half * self.block), self.block) }
    }
}

impl Drop for DawBuffer {
    fn drop(&mut self) {
        let whole = std::ptr::slice_from_raw_parts_mut(self.data, self.block * 2);
        drop(unsafe { Box::from_raw(whole) });
    }
}

/// One sub-device, as the audio path sees it.
pub struct DeviceStream {
    /// What its channels are called, for anything that reports.
    pub name: String,
    buffers: DeviceBuffers,
    input_type: i32,
    output_type: i32,
    input_width: usize,
    output_width: usize,
    block: usize,
    /// The device's own input blocks, waiting for the master. None on the master itself.
    pub in_ring: Option<Ring>,
    /// Blocks the master has left for this device to play. None on the master itself.
    pub out_ring: Option<Ring>,
    /// How many times this device has called back. Read by the master to see it is still alive.
    pub callbacks: AtomicU64,
    /// Set by the master when this device stopped calling back.
    pub stalled: AtomicBool,
    /// Set by the master, read by the device: write silence and play nothing you were given.
    pub mute_out: AtomicBool,
}

impl DeviceStream {
    fn inputs(&self) -> usize {
        self.buffers.inputs.len()
    }

    fn outputs(&self) -> usize {
        self.buffers.outputs.len()
    }

    /// Read the device's inputs for this half into `into`, one channel's run after another.
    fn read_inputs(&self, half: usize, into: &mut [i32]) {
        let block = self.block;
        let width = self.input_width;
        for (slot, pair) in self.buffers.inputs.iter().enumerate() {
            let run = &mut into[slot * block..(slot + 1) * block];
            let source = pair[half & 1];
            if source.is_null() {
                run.fill(0);
                continue;
            }
            // Safety: the device filled this buffer before calling back, and it is `block` samples
            // of `width` bytes, which is what it was asked for at createBuffers.
            let bytes = unsafe { std::slice::from_raw_parts(source, block * width) };
            if self.input_type == sample::INT32_LSB {
                // What both interfaces here report, and the same bytes we carry it in.
                unsafe { std::ptr::copy_nonoverlapping(source.cast::<i32>(), run.as_mut_ptr(), block) };
            } else {
                for (index, value) in run.iter_mut().enumerate() {
                    *value = sample::read(self.input_type, &bytes[index * width..]);
                }
            }
        }
    }

    /// Write `from` into the device's outputs for this half. Every sample of every channel.
    fn write_outputs(&self, half: usize, from: &[i32]) {
        let block = self.block;
        let width = self.output_width;
        for (slot, pair) in self.buffers.outputs.iter().enumerate() {
            let run = &from[slot * block..(slot + 1) * block];
            let target = pair[half & 1];
            if target.is_null() {
                continue;
            }
            // Safety: as `read_inputs`, for the buffer the device gave us to fill.
            if self.output_type == sample::INT32_LSB {
                unsafe { std::ptr::copy_nonoverlapping(run.as_ptr(), target.cast::<i32>(), block) };
            } else {
                let bytes = unsafe { std::slice::from_raw_parts_mut(target, block * width) };
                for (index, value) in run.iter().enumerate() {
                    sample::write(self.output_type, *value, &mut bytes[index * width..]);
                }
            }
        }
    }

    /// Silence, which is what everything that goes wrong sounds like here.
    fn silence_outputs(&self, half: usize) {
        let bytes = self.block * self.output_width;
        for pair in &self.buffers.outputs {
            let target = pair[half & 1];
            if !target.is_null() {
                // Safety: as above. Silence is zero in every sample type this driver converts.
                unsafe { std::ptr::write_bytes(target, 0, bytes) };
            }
        }
    }
}

/// What the master's thread keeps to itself.
struct Scratch {
    /// One run per device: its selected input channels, one after another.
    stage_in: Vec<Vec<i32>>,
    stage_out: Vec<Vec<i32>>,
    delay_in: Vec<Delay>,
    delay_out: Vec<Delay>,
    watch: Vec<Watch>,
    /// Which half of the aggregate's own buffers the DAW is using.
    half: usize,
}

/// How a device is watched for going away.
#[derive(Clone, Copy, Default)]
struct Watch {
    last: u64,
    missed: u32,
    moving: u32,
}

/// Everything the audio path touches. Made when the buffers are made, dropped when they are
/// disposed of, and shared with every sub-device in between.
pub struct Stream {
    pub devices: Vec<DeviceStream>,
    pub master: usize,
    pub block: usize,
    pub rate: f64,
    daw_in: Vec<DawBuffer>,
    daw_out: Vec<DawBuffer>,
    in_map: Vec<ChannelRef>,
    out_map: Vec<ChannelRef>,
    host: CallbacksRaw,
    /// Whether the DAW asked for the buffer callback that carries the time.
    time_info: bool,
    stall_after: u32,
    recover_after: u32,
    /// Samples the aggregate has handed the DAW, which is its position.
    position: AtomicU64,
    /// Which half the DAW is on, so a control call can see it without the scratch.
    published_half: AtomicUsize,
    running: AtomicBool,
    /// Where the block by block counters go. Writing through it is a compare and exchange and a
    /// run of plain stores into a mapped page; a reporter with nowhere to report does nothing at
    /// all, and either way this never waits, allocates or makes a call into the system.
    reporter: Arc<Reporter>,
    scratch: UnsafeCell<Scratch>,
}

/// The scratch is the master's callback thread's alone, the ring buffers have one reader and one
/// writer each, and everything else here is atomic or read only after it was made.
unsafe impl Sync for Stream {}
unsafe impl Send for Stream {}

impl Stream {
    /// Build the audio path. `buffers` is one entry per device in plan order; `in_map` and
    /// `out_map` are the aggregate channels the DAW actually asked for, in its own order.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        plan: &Plan,
        config: &Config,
        rate: f64,
        buffers: Vec<DeviceBuffers>,
        in_map: Vec<ChannelRef>,
        out_map: Vec<ChannelRef>,
        host: CallbacksRaw,
        time_info: bool,
        reporter: Arc<Reporter>,
    ) -> Stream {
        let block = plan.block.max(0) as usize;
        let mut devices = Vec::new();
        let mut stage_in = Vec::new();
        let mut stage_out = Vec::new();
        let mut delay_in = Vec::new();
        let mut delay_out = Vec::new();
        for (index, (device, device_buffers)) in plan.devices.iter().zip(buffers).enumerate() {
            let ins = device_buffers.inputs.len();
            let outs = device_buffers.outputs.len();
            let buffered = index != plan.master;
            stage_in.push(vec![0i32; ins * block]);
            stage_out.push(vec![0i32; outs * block]);
            delay_in.push(Delay::new(ins, device.pad_in.max(0) as usize));
            delay_out.push(Delay::new(outs, device.pad_out.max(0) as usize));
            devices.push(DeviceStream {
                name: device.name.clone(),
                input_type: device.input_type,
                output_type: device.output_type,
                input_width: device.input_width,
                output_width: device.output_width,
                block,
                in_ring: buffered.then(|| Ring::new(config.ring_buffers, ins * block)),
                out_ring: buffered.then(|| Ring::new(config.ring_buffers, outs * block)),
                callbacks: AtomicU64::new(0),
                stalled: AtomicBool::new(false),
                mute_out: AtomicBool::new(false),
                buffers: device_buffers,
            });
        }
        let count = devices.len();
        let daw_in = (0..in_map.len()).map(|_| DawBuffer::new(block)).collect();
        let daw_out = (0..out_map.len()).map(|_| DawBuffer::new(block)).collect();
        Stream {
            devices,
            master: plan.master,
            block,
            rate,
            daw_in,
            daw_out,
            in_map,
            out_map,
            host,
            time_info,
            stall_after: config.stall_after_buffers.max(1),
            recover_after: config.recover_after_buffers.max(1),
            position: AtomicU64::new(0),
            published_half: AtomicUsize::new(0),
            running: AtomicBool::new(false),
            reporter,
            scratch: UnsafeCell::new(Scratch {
                stage_in,
                stage_out,
                delay_in,
                delay_out,
                watch: vec![Watch::default(); count],
                half: 0,
            }),
        }
    }

    /// The pointer for one of the DAW's own buffers.
    pub fn input_pointer(&self, index: usize, half: usize) -> *mut std::ffi::c_void {
        self.daw_in[index].pointer(half)
    }

    pub fn output_pointer(&self, index: usize, half: usize) -> *mut std::ffi::c_void {
        self.daw_out[index].pointer(half)
    }

    /// Let the audio through. Until this is called every device is kept silent and the DAW is
    /// never called.
    pub fn run(&self) {
        self.position.store(0, Ordering::Release);
        self.running.store(true, Ordering::Release);
    }

    /// Stop the audio. Devices that are still calling back write silence.
    pub fn halt(&self) {
        self.running.store(false, Ordering::Release);
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }

    /// Samples the aggregate has handed the DAW, and the system time it was last read at.
    pub fn position(&self) -> (i64, i64) {
        (self.position.load(Ordering::Acquire) as i64, crate::now_nanos())
    }

    /// One device has called back. This is the whole of the audio path's entry.
    pub fn device_callback(&self, device: usize, half: usize) {
        let Some(sub) = self.devices.get(device) else { return };
        sub.callbacks.fetch_add(1, Ordering::Relaxed);
        if device == self.master {
            self.master_callback(half);
        } else {
            self.follower_callback(sub, half);
        }
    }

    /// A device that is not driving the callback: hand over what it heard, play what it was left.
    fn follower_callback(&self, sub: &DeviceStream, half: usize) {
        let muted = sub.mute_out.load(Ordering::Acquire) || !self.is_running();
        let played = if muted {
            if let Some(ring) = &sub.out_ring {
                // Throw away what was left while it was away, so it never plays late.
                ring.drain();
            }
            false
        } else {
            match &sub.out_ring {
                Some(ring) => ring.pop_with(|block| sub.write_outputs(half, block)),
                None => false,
            }
        };
        if !played {
            sub.silence_outputs(half);
        }
        if !muted && self.is_running() {
            if let Some(ring) = &sub.in_ring {
                ring.push_with(|block| sub.read_inputs(half, block));
            }
        }
    }

    /// The device that drives everything: gather every device's inputs, call the DAW, scatter its
    /// outputs back out.
    fn master_callback(&self, half: usize) {
        // Safety: the master's callback thread is the only one that ever touches the scratch, and
        // a driver does not call one of these on two threads at once.
        let scratch = unsafe { &mut *self.scratch.get() };
        let master = &self.devices[self.master];
        if !self.is_running() {
            master.silence_outputs(half);
            return;
        }
        let block = self.block;
        let daw_half = scratch.half;

        // 1. Every device's inputs, into its own staging run, held back to line up.
        for (index, sub) in self.devices.iter().enumerate() {
            let stage = &mut scratch.stage_in[index];
            if index == self.master {
                sub.read_inputs(half, stage);
            } else {
                let stalled = sub.stalled.load(Ordering::Acquire);
                let got = !stalled && sub.in_ring.as_ref().is_some_and(|ring| ring.pop_with(|b| stage.copy_from_slice(b)));
                if !got {
                    stage.fill(0);
                }
            }
            scratch.delay_in[index].process(stage, block);
        }

        // 2. The channels the DAW asked for, in the order it asked for them.
        for (index, channel) in self.in_map.iter().enumerate() {
            let source = &scratch.stage_in[channel.device];
            let run = &source[channel.slot * block..(channel.slot + 1) * block];
            self.daw_in[index].half(daw_half).copy_from_slice(run);
        }

        // 3. The DAW's output buffers are cleared before it is given them, so that a host which
        //    writes nothing plays silence rather than whatever was there two blocks ago.
        for buffer in &self.daw_out {
            buffer.half(daw_half).fill(0);
        }

        // 4. The DAW.
        self.call_host(daw_half);

        // 5. What it wrote, back out to the devices. Every channel of every device is cleared
        //    first, so that a channel the DAW did not ask for is silence rather than whatever was
        //    last there, whatever changes above this line.
        for stage in scratch.stage_out.iter_mut() {
            stage.fill(0);
        }
        for (index, channel) in self.out_map.iter().enumerate() {
            let target = &mut scratch.stage_out[channel.device];
            target[channel.slot * block..(channel.slot + 1) * block].copy_from_slice(self.daw_out[index].half(daw_half));
        }
        for (index, sub) in self.devices.iter().enumerate() {
            let stage = &mut scratch.stage_out[index];
            scratch.delay_out[index].process(stage, block);
            if index == self.master {
                sub.write_outputs(half, stage);
            } else if !sub.stalled.load(Ordering::Acquire) {
                if let Some(ring) = &sub.out_ring {
                    ring.push_with(|slot| slot.copy_from_slice(stage));
                }
            }
        }

        // 6. Who is still with us.
        self.watch_devices(scratch);

        self.position.fetch_add(block as u64, Ordering::AcqRel);
        scratch.half ^= 1;
        self.published_half.store(scratch.half, Ordering::Release);

        // 7. What anyone watching is told. Nothing here waits: if the record is being written by
        //    the watcher thread this instant, this block's counters are skipped and the next
        //    block's are written instead.
        self.publish();
    }

    /// The block by block half of the status record: the counters, the time of this block, and the
    /// gap between each device and the master, which is the number a person actually wants. Zero
    /// while the devices are locked, and growing in one direction when they are not.
    fn publish(&self) {
        if !self.reporter.is_publishing() {
            return;
        }
        let master_seen = self.devices[self.master].callbacks.load(Ordering::Relaxed);
        let position = self.position.load(Ordering::Relaxed);
        let nanos = crate::now_nanos();
        let block = self.block as i64;
        self.reporter.try_update(|area| {
            area.callbacks = master_seen;
            area.position = position;
            area.last_block_nanos = nanos;
            for (index, sub) in self.devices.iter().enumerate() {
                let Some(into) = area.devices.get_mut(index) else { break };
                let seen = sub.callbacks.load(Ordering::Relaxed);
                into.callbacks = seen;
                into.streaming = 1;
                into.stalled = u32::from(sub.stalled.load(Ordering::Relaxed));
                into.gap = if index == self.master {
                    0
                } else {
                    // Each callback is one block of samples, on both devices, so the difference in
                    // callbacks is the difference in samples.
                    (seen as i64 - master_seen as i64) * block
                };
                into.dropped = sub.in_ring.as_ref().map_or(0, Ring::dropped) + sub.out_ring.as_ref().map_or(0, Ring::dropped);
                into.starved = sub.in_ring.as_ref().map_or(0, Ring::starved) + sub.out_ring.as_ref().map_or(0, Ring::starved);
            }
        });
    }

    /// Call the DAW, with the time if it asked for it. The aggregate's time is the master's: its
    /// callbacks are what move the sample position on.
    fn call_host(&self, half: usize) {
        if self.time_info {
            let mut time = Time::default();
            time.info.speed = 1.0;
            time.info.sample_position = Samples::from_i64(self.position.load(Ordering::Acquire) as i64);
            time.info.system_time = Samples::from_i64(crate::now_nanos());
            time.info.sample_rate = self.rate;
            time.info.flags = time_flags::SYSTEM_TIME_VALID
                | time_flags::SAMPLE_POSITION_VALID
                | time_flags::SAMPLE_RATE_VALID
                | time_flags::SPEED_VALID;
            // Safety: the DAW gave us this function pointer at createBuffers and it is valid until
            // disposeBuffers, which cannot run while this call is in flight.
            unsafe { (self.host.buffer_switch_time_info)(&mut time, half as i32, 1) };
        } else {
            unsafe { (self.host.buffer_switch)(half as i32, 1) };
        }
    }

    /// A device that has stopped calling back is declared stalled: its inputs read as silence and
    /// its outputs are muted, and the aggregate keeps running. Nothing here restarts anything: a
    /// vendor driver is never touched from a callback thread.
    fn watch_devices(&self, scratch: &mut Scratch) {
        for (index, sub) in self.devices.iter().enumerate() {
            if index == self.master {
                continue;
            }
            let watch = &mut scratch.watch[index];
            let seen = sub.callbacks.load(Ordering::Relaxed);
            if seen == watch.last {
                watch.missed += 1;
                watch.moving = 0;
            } else {
                watch.last = seen;
                watch.missed = 0;
                watch.moving += 1;
            }
            let stalled = sub.stalled.load(Ordering::Relaxed);
            if !stalled && watch.missed >= self.stall_after {
                sub.stalled.store(true, Ordering::Release);
                sub.mute_out.store(true, Ordering::Release);
                // Whatever it did hand over is old by now.
                if let Some(ring) = &sub.in_ring {
                    ring.drain();
                }
                scratch.delay_in[index].clear();
                scratch.delay_out[index].clear();
            } else if stalled && watch.moving >= self.recover_after {
                sub.stalled.store(false, Ordering::Release);
                sub.mute_out.store(false, Ordering::Release);
            }
        }
    }

    /// A message from one of the sub-devices, on its way to the DAW. Reset requests, rate changes
    /// and latency changes are all the DAW's business, not something this driver can answer.
    pub fn forward(&self, selector: i32, value: i32) -> i32 {
        use gazelle_audio_stream_abi::raw::selector as which;
        match selector {
            which::RESET_REQUEST | which::RESYNC_REQUEST | which::LATENCIES_CHANGED | which::BUFFER_SIZE_CHANGE
            | which::OVERLOAD => {
                // Safety: as `call_host`.
                unsafe { (self.host.message)(selector, value, std::ptr::null_mut(), std::ptr::null_mut()) }
            }
            _ => 0,
        }
    }

    /// A sub-device saying its rate moved, on its way to the DAW.
    pub fn forward_rate(&self, hz: f64) {
        unsafe { (self.host.sample_rate_did_change)(hz) };
    }

    /// What each device has lost so far, in device order. Read off the audio path: the counters
    /// are the rings' own, and reading them is what turns a session into a line of the event log
    /// and a measurement into one that can be trusted or not.
    pub fn glitches(&self) -> Vec<Glitches> {
        self.devices
            .iter()
            .map(|device| Glitches {
                device: device.name.clone(),
                dropped: device.in_ring.as_ref().map_or(0, Ring::dropped) + device.out_ring.as_ref().map_or(0, Ring::dropped),
                starved: device.in_ring.as_ref().map_or(0, Ring::starved) + device.out_ring.as_ref().map_or(0, Ring::starved),
            })
            .collect()
    }

    /// Whether a device is currently counted as gone.
    pub fn is_stalled(&self, device: usize) -> bool {
        self.devices.get(device).is_some_and(|d| d.stalled.load(Ordering::Acquire))
    }

    /// How many channels the DAW asked for.
    pub fn daw_inputs(&self) -> usize {
        self.daw_in.len()
    }

    pub fn daw_outputs(&self) -> usize {
        self.daw_out.len()
    }

    /// Which half of its buffers the DAW was last handed.
    pub fn half(&self) -> usize {
        self.published_half.load(Ordering::Acquire)
    }

    /// How many channels one device opened, for anything that reports.
    pub fn device_channels(&self, device: usize) -> (usize, usize) {
        match self.devices.get(device) {
            Some(sub) => (sub.inputs(), sub.outputs()),
            None => (0, 0),
        }
    }
}
