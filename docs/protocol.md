# The control protocol

How a computer talks to Antelope Audio's Zen Quadro Synergy Core and Zen Studio+ over USB, as
Gazelle implements it. This is a developer document, not part of the user manual.

It was worked out for interoperability with hardware the author owns: from the vendor's own
control software (see [Reverse engineering](reverse-engineering.md) for how to repeat that with
your own copy), and then checked against the devices themselves where that was safe. Gazelle is
not affiliated with or endorsed by Antelope Audio.

The machine-readable half of this document is in `refs/schemas/`:

| File | What it holds |
|---|---|
| `refs/schemas/quadro_commands.json` | Every Zen Quadro command Gazelle knows: header, payload id, request fields, reply fields, and the cyclic report layouts |
| `refs/schemas/studio_commands.json` | The same for the Zen Studio+ |
| `refs/schemas/afx_parameters.json` | Each effect type's parameter commands, with ranges, defaults and display rules |
| `refs/schemas/quadro_topology.json`, `refs/schemas/studio_topology.json` | Input and output groups for routing, mixer size, and which source feeds what |

The Rust implementation is `crates/gazelle-audio-protocol` (fields, payloads, the command
registry) and `crates/gazelle-audio-transport` (framing, segmentation and matching replies to
requests). Both are tested byte for byte against vectors generated independently by
`crates/gazelle-audio-protocol/tests/ground_truth.py` and `gen_cyclic_gt.py`.

Where a statement is marked **confirmed**, it was seen on a device. Everything else is what the
vendor software does, which is strong evidence but not proof of what the device accepts.

## Transport

- Both devices are USB HID devices with a vendor-defined interface (usage page `0xFFA0`,
  interface 3). Windows' HID driver owns that interface, so software talks to it through the HID
  API rather than raw USB.
- Vendor ids: `0x23E5` (9189) on USB, `0x1D4B` (7499) on Thunderbolt.
- Product ids in application mode: Zen Quadro Synergy Core `0xA2F9`, Zen Studio+ `0xA100`. In
  boot (firmware update) mode they are `0xB2F9` and `0xB100`; a product id above `0xB000` (45056)
  means boot mode. Gazelle only talks to devices in application mode.
- Reports are 320 bytes with HID report id 0 (**confirmed** on both devices).
- Only one program can hold the interface. While the vendor's own background service runs, other
  software cannot open the devices at all.

## Header

Every message, in either direction, starts with a 16-byte little-endian header:

| Offset | Field | Type |
|---|---|---|
| 0 | `cmd` | u32 LE |
| 4 | `seq` | u32 LE |
| 8 | `ext2` | u32 LE |
| 12 | `ext3` | u32 LE |

The contents follow. On a request, `seq` is the length of the whole message, header included
(16 plus the payload). On a cyclic report it is the **CRC32 of the contents** (zlib's CRC32), and
a report whose CRC does not match is discarded.

## Segmentation

A message longer than one report travels in segments:

- `cmd` 8052 (`SEGMENT_SEND_ID`) from host to device, 8053 (`SEGMENT_RECV_ID`) from device to host.
- A segment's header carries `seq = chunk length + 16`, `ext2 = total length` and
  `ext3 = offset` of this chunk in the whole message.
- The device reassembles the host's 8052 segments; the host reassembles the device's 8053
  segments. The device wraps some reports in an 8053 segment even when they would fit in one
  report (**confirmed**), and the segment's `ext2` then gives the exact length.

## Requests and replies

A command is identified by its header: `report_id` (the `cmd` word), `ext2` and `ext3`, plus a
`payload_id` for commands that carry one.

| `report_id` | What it is |
|---|---|
| `0x70` | Set requests, told apart by `payload_id` (0 to 43) |
| `0x74` | Get requests, told apart by `ext2` (and for some by `ext3`) |
| `0xE0` | `set_tb_latency`: no `payload_id`, the fields follow the header directly |
| `0xE2` | `get_tb_latency` |

- **A reply** carries `cmd = report_id + 1` and the request's `ext2`. `cmd` 255 is a generic
  acknowledgement that answers any request.
- **A refusal** (**confirmed**) sets the top bit in both: `cmd = (report_id + 1) | 0x80000000`
  and `ext2 = ext2 | 0x80000000`, with `ext3` not echoed and the contents all zero.
- **Matching.** `seq` plays no part: a reply is recognised by `cmd` and `ext2` alone, so one
  request is outstanding per device at a time. Stale reports are discarded before a request is
  sent. The vendor software takes the very next report as the answer; Gazelle passes cyclic
  reports that arrive in between on as events and keeps waiting.
- **Timeout.** The vendor software gives a request 3 seconds.
- **`ext3` selectors.** A few gets take a selector in `ext3` rather than a payload:
  `get_mixer` (the mixer, 0 to 3), `get_routing` (the destination group) and
  `get_afx_strip_order` (the effect chain). The links reads use `ext3` for the kind of channel,
  below.

## Field grammar

The vendor software describes each command as a name, a header and a list of fields, and each
cyclic report as a list of fields. The schemas keep that form:

```json
"set_routing": {"report_id": "0x70", "ext2": 0, "ext3": 0, "payload_id": 18,
                "params": [{"name": "bank_idx", "type": "ubyte"},
                           {"name": "bank_configs", "type": "{\"elem_type\": \"ubyte * 2\", \"count\": 32}"}]}
```

A field is one of:

| Form | Example | Size |
|---|---|---|
| scalar | `"ubyte"` | the type's width |
| inline array | `"ubyte * N"` | element size times N |
| nested struct array | `{"fields": [[name, type], ...], "count": N}` | the struct's size times N |
| element array | `{"elem_type": "ubyte * K", "count": N}` | element size times N |
| scalar with width and default | `["density", "ubyte", 8, 100]` | the third element is a width in **bits**, the fourth the default |

- Types: `ubyte byte uint8 int8 short ushort uint16 int16 uint32 int32 float bool`, with the
  meaning of the matching C types; a bare `short` is **signed**.
- A field without a bit width takes its type's full width.
- Nested types are stored in the schemas as JSON-encoded strings; parse them before use. An
  `elem_type` may itself be a struct.
- `returns` is the reply layout of a get. A reply declared with a top-level `count` is that many
  entries, extracted as an `entries` list.
- Ten commands carry `auto_send_notification: true`: the device also pushes a notification
  when they are applied.

## Payload serialization

A set command's body is a small payload header followed by the user fields. Which form it takes
depends on `user_bytes`, the size of the user fields (sub-byte fields packed, so a struct of five
fields totalling 8 bits is one byte):

- **`user_bytes < 4`**: one header byte, `payload_id` in the low 6 bits and
  `nparams = user_bytes - 1` (never below 0) in the high 2 bits. No length byte.
  `set_pre_type` (payload id 15, 2 user bytes) starts `0x4F`: payload id 15, nparams 1. The body
  is 3 bytes.
- **`user_bytes >= 4`**: two header bytes, the same packed byte with `nparams` 3, then
  `nbytes = user_bytes`. `set_mixer` (payload id 20, 6 user bytes) starts `0xD4 0x06`. The body
  is 8 bytes.
- **No `payload_id`** (`set_tb_latency`): no header bytes at all; the body is just the fields
  (five int32, 20 bytes).

Sub-byte fields pack **least significant bit first**; full-width scalars are little-endian at
their width. A field the caller leaves out takes its declared default, else zero. So every set
sends every field: a command that sets a channel's level also sends its pan, mute and solo.

The ground-truth vectors in `crates/gazelle-audio-protocol/tests/ground_truth.json` were made
from these rules and match the bytes the vendor software sends.

## Cyclic reports

The devices push reports on their own, with no request:

- `0x73`: status and meters (preamps, gains, volumes, clock, meter bytes).
- `0x83`: effect meters, about 125 times a second (**confirmed**); see
  [The effect-meter report](#the-effect-meter-report).

Decoding differs from requests:

- Sub-byte scalars (`current_preset` is 3 bits, `power_on` 1 bit) are read least significant bit
  first from a running bit position; signed types sign-extend.
- A struct array's element size is computed in **bits**, not bytes: `preamps` is 12 elements of
  a five-field struct whose fields total 8 bits, so one byte each.
- The reader advances by element size times count, not by one element.
- `seq` must equal the CRC32 of the contents.

## Controller events

Report id 257 (`0x101`) carries knob and key events from the device's controls. After the
16-byte header:

- a byte counting key presses, then that many pairs of u16 (id, elapsed time);
- a byte counting key releases, then pairs in the same form;
- a byte counting rotary events, then that many (u16 id, i8 delta, u16 elapsed time).

## Command surface

The schemas hold what each model's own control software declares:

| | Zen Quadro | Zen Studio+ |
|---|---|---|
| Shared with the other model | 35 | 35 |
| This model only | 28 | 8 |
| Effect parameter commands (a set and a get per effect type) | 136 (68 types) | 74 (37 types) |
| **In the schema** | **199** | **117** |
| **Served by Gazelle** | **195** | **116** |

The Studio+'s own format declares more requests than this, mostly effect management; the
schema keeps the ones Gazelle has a use for. The Studio+-only commands are
`get_lines_links`, `set_line_gain`, `set_mixer_cfg`, `set_pre_phaseinv`, `set_talk`,
`set_tbk_enable`, `set_tbk_vol` and `set_trim`.

Gazelle does not serve the commands that manage a device's licence: `set_config_feature` (both
models) and, on the Quadro, `get_cmd_set_assignment`, `get_assignment_request` and
`get_assignment_status`. They stay in the schemas as the vendor declared them, but the server
refuses them as unknown. `get_feature_mask` is served, since it is how the app knows which
microphone emulations a device may use.

Each schema also records where it came from (`source`, `report_version`), anything the extractor
could not resolve (`unresolved_constants`, `unresolved_reply_counts`, `in_scope_zero_counts`,
all empty or explained), and the extractor's scope.

## Device notes

What the commands mean, from the vendor software's bindings and from the devices. Ids count from
0 unless stated.

### Mixer and meter value scales

**Mixer strips** (`set_mixer` on the Quadro, `set_mixer_cfg` on the Studio+):

- **Channels.** Device channel 0 is the master; strip `i` is device channel `i + 1`. Each device
  has four mixes of 32 strips, which are hardware buses: no command creates more.
- **`level`**: dB of attenuation, 0 to 90. 0 is 0 dB (the top of the fader) and 90 is -90 dB.
  The fader is linear in the value; there is no -inf on channel faders.
- **`pan`**: 2 to 62 in a 6-bit field, centre 32. The vendor fader snaps to 32 inside 27 to 38
  when dragged, and shows `value - 32`.
- **`send`** (Studio+ only, the reverb send): dB of attenuation, 0 (loudest) to 95, where 95 is
  -inf (**confirmed**).
- **`mute`, `solo`**: one bit each.
- **Reading.** `get_mixer` with the mixer in `ext3` answers 33 entries, master first. A Quadro
  entry is 2 bytes: level, then pan in bits 0 to 5, mute in bit 6, solo in bit 7. A Studio+
  entry adds a third byte, `send` (**confirmed**: 99-byte replies).
- **Links.** Links reads are `0x74`, `ext2` 11, with `ext3` giving the kind and one byte per
  pair: Quadro preamps 0 (1 pair), ADAT 1 (8), S/PDIF 2 (1), mixers 3 (64), effects 4; Studio+
  preamps 0 (6), lines 1 (4), ADAT 2 (8), S/PDIF 3 (1), mixers 4 (64), effects 5.
  `set_stereo_link(periph_id, channel_id, linked)` uses the same kind ids. For a mixer,
  `channel_id = (strip + mixer x 32) / 2`, and `get_mixer_links` answers 64 bytes, 16 per mixer,
  entry k covering channels 2k and 2k+1. A linked partner copies level, mute and solo, but not
  pan.

**Meters** (the cyclic `peaks_*` bytes):

- A byte is **dB below full scale**: 0 is full scale, 96 is the idle value. The vendor meters
  show 0 to 60 and latch a clip light at 0, cleared locally with no device command.
- The vendor's bar position, deflection 0 to 100 for byte v: 0 above 60; `(60 - v) x 0.5` for
  50 < v <= 60; `(50 - v) + 5` for 40 < v <= 50; `(40 - v) x 1.5 + 15` for 30 < v <= 40;
  `(30 - v) x 2 + 30` for 20 < v <= 30; `(20 - v) x 2.5 + 50` for 0 < v <= 20; 100 at 0.
  Marks at 0, 5, 10, 15, 20, 30, 40 and 60 dB.

**Meter sources** (`set_peak_source(bank_id, source_id)`, reported in `pm_bank_src`):

- Source ids: user bank A 0, user bank B 1, preamp 2, ADAT in 3, ADAT out 4, S/PDIF in 5,
  S/PDIF out 6, mixer 7, HP2 12, monitor 15, computer playback 16, line out 19.
- Studio+: bank 0 is the Meters page's selection; bank 1 follows the selected mixer.
- Quadro: only bank 0 is used, and only 16 of the 32 `peaks_mixer` bytes are read.

### Output ids

On the Quadro, each output's volume, mute and dim bind to `set_volume(id)`, `set_mute(id)` and
`set_dim(id)`: **Monitor 0, HP1 1, HP2 2, Line out 3**. The cyclic `volumes` array holds six
entries of {volume 8 bits, mute 1, dim 1, mono 1, trim 5}; entries 4 and 5 are bound to no
command. Volume is dB of attenuation, 0 to 96 with 96 as -inf (**confirmed** for Monitor).

### Studio+ output volumes

Volume and mute ids: **Monitor 0, HP1 1, HP2 2, Line out 3, Reamp 4**; 5 is the talkback level,
set with `set_tbk_vol`. They are reported as `monitor_vol`, `hp1_vol`, `hp2_vol`,
`line_out_vol`, `reamp_vol` and the matching `*_mute` bits. There is no dim command. The scale
is taken to be the Quadro's (0 to 96 dB of attenuation, 96 as -inf); not yet confirmed.

### Device presets

`preset_recall(preset_idx)` and `preset_save(preset_idx)` on both models, slots **1 to 5**. The
status report's 3-bit `current_preset` is the active slot. `set_predefined_preset` is an effect
preset, not one of these.

### Clock and sample rate

- `set_samp_rate(index)` into `32, 44.1, 48, 88.2, 96, 176.4, 192 kHz` (0 to 6), both models.
- `set_sync_source(index)`: Quadro `Internal, ADATx1, ADATx2, ADATx4, S/PDIF, USB` (0 to 5);
  Studio+ `Oven, W.C., ADAT, ADAT 2x, ADAT 4x, S/PDIF, USB` (0 to 6).
- The `0x73` report (**confirmed** on both): `sync_freq_hi/mid/low` are the measured rate in Hz,
  high byte first (`1, 119, 0` is 96000); `base_index` is the current rate's index; lock is
  `locked` on the Quadro and `locked_wc` on the Studio+.
- `set_spdif_src(on)` (Studio+) switches the sample-rate converter on the S/PDIF input, reported
  back as `spdif_src`. The Quadro's command table has it but nothing on the Quadro sends it.
- `set_usb_channels`, `usb_mode` and `usb_available_ch_ix` (Quadro): no vendor code sends or
  reads them in a way that shows their meaning. Gazelle binds nothing to them.

### Control Room controls

- **Trims**, both models: seven steps, `20 dBu` down to `14 dBu` (index 0 to 6), reported in the
  3-bit `monitor_trim`, `line_out_trim` and `adc_trim`. Quadro: `set_trim_config(trim_id,
  control, level)` with Monitor 0, Line out 1, `control` 1, and `level` 32 two-byte entries of
  which the first is `[index, 0]`. Studio+: `set_trim(id, index)` with Monitor 0, Line out 1,
  ADC 2.
- **Talkback**, Studio+ only: `set_talk(on)` (reported `talkback_on`); `set_tbk_vol(volume)` on
  the output volume scale; `set_tbk_enable(id, enabled)` sends it to HP1 0, HP2 1, Monitor 2.
- **Thunderbolt latency**: `set_tb_latency` and `get_tb_latency` are Thunderbolt settings, not
  talkback. `mode` is Fast 0, Normal 1, Safe 2; the other four fields are left at 0. The vendor
  software uses them only over Thunderbolt, so Gazelle, which is USB only, does not.
- **Panning law**, Quadro only: `set_panning_law(index)` into `0 dB, -6 dB, -3 dB, -4.5 dB`,
  read with `get_panning_law`.
- **DC coupling**, Quadro only: `set_dc_coupled(dc_coupled, side)` with side 0 the inputs and
  1 the outputs, reported as `dc_coupled_in` and `dc_coupled_out`.
- **Test oscillator**, both: `set_sine_gen(freq_left, freq_right, level, mute_left,
  mute_right)`, five fields in one byte, so every change carries all five. Frequencies
  `1 kHz, 440 Hz`; levels `0, -6, -12, -18 dBFS`.
- **Monitor out**, Quadro only: `set_monitor_out` is declared but nothing sends it; its meaning
  is unknown.
- **Mono**: no command on either model. The Quadro reports a `mono` bit per output. Gazelle
  makes a mix mono by centring its channels' pans, and lowers that mix's master by 6 dB (level
  is dB of attenuation, so level + 6, held to 90) to make up for summing both sides.

#### Hard mute

Quadro only: `set_hard_mute(value)`, 0 or 1, reported as `hard_mute`. It mutes every output at
once. The vendor software sets it around restoring a saved session, so nothing plays through
half-applied routing.

### Inputs and routing

**Preamps** (cyclic `preamps` and `preamp_gains`):

- **Type** (`type` on the Quadro, `pretype` on the Studio+, 4 bits): Mic 0, Line 1, Hi-Z 2.
  Hi-Z is on Quadro preamps 1 and 2 and Studio+ preamps 1 to 4. `set_pre_type(id, type)`.
- **Gain**: whole dB, no scaling. The range is each panel's own: the Quadro panel replaces the shared preamp table (`PreampModel.VALUES_CONFIG`: Mic 0 to 65, Line -6 to +20, Hi-Z 0 to 40) with its own `CustomizedPreampModel`, Mic 0 to 75, Line -6 to +20, Hi-Z 0 to 45; the Studio+ panel's knob is Mic 0 to 65, Line -6 to +20, Hi-Z 0 to 40.
- **48V**: `set_pre_phantom(id, on)`, for Mic only.
- **Phase**: `set_pre_phase_inv` (Quadro) or `set_pre_phaseinv` (Studio+), 0 or 1. The Quadro
  disables phase and 48V while mic emulation is on the channel.
- **High-pass filter**: reported only; neither model has a command to set it.
- **Links**: `set_stereo_link` with the kind ids above.

**Mic emulation** (Quadro only): `set_mic_emulation(preamp_ch, target, emu_model, ch_swap,
pattern)`, all five together, read back with `get_mic_emulations` (`0x74`, `ext2` 22), which
the Quadro declares as **two** entries.

- `target` is the microphone on the preamp: any 0, Edge Duo 1, Verge 2, Edge Solo 3, Edge
  Quadro 4, Accord 5, Edge Note 6.
- `emu_model` indexes that microphone's own list of emulations; the lists differ, and 0 is the
  microphone itself. `refs/tools/scripts/mic_emulations.py` generates the app's table.
- The Edge Duo takes two preamps and the Edge Quadro four, linked, each needing 48V.
- `ch_swap` swaps a microphone's front and rear membranes.
- `pattern` is a polar pattern in the model's own units, mapped linearly onto an angle where +1
  is omni, 0 cardioid and -1 figure-8. Only Edge Duo, Edge Quadro and Accord models have one.
- Which microphones and emulations the device may use is read from the device with
  `get_feature_mask` (`0x74`, `ext2` 17, `ext3` 1, a 290-byte reply). Gazelle only reads it, to
  grey out what is not available.

**Digital inputs** (`line_gains`, `adat_gains`, `spdif_gains`): -6 to +12 dB in 1 dB steps. The
Studio+ sets them with `set_line_gain`, `set_adat_gain` and `set_spdif_gain`; the Quadro reports
ADAT and S/PDIF gains but its own software never sends those commands.

**Routing** (`set_routing`, `get_routing`):

- `set_routing(bank_idx, 32 x (source group, channel))` replaces one destination group. Each
  destination takes one source; a source feeds any number of destinations. `(0, 0)` is preamp 1,
  not silence: MUTE is a source group of its own.
- Group ids are positions in the topology files' `inputs` and `outputs` lists. Quadro: MUTE 10,
  MIX CH1 to 4 are destinations 8 to 11. Studio+: MUTE 11, MIX CH1 to 4 are 10 to 13.
- `get_routing` is `0x74`, `ext2` 3, with the destination group in `ext3`. The reply is
  `bank_idx` then pairs (64 declared on the Quadro, 32 on the Studio+). There is no cyclic
  routing field.

### Effects (AFX) and reverb

- **Chains.** A chain processes what routing sends to AFX IN k and plays on AFX OUT k. Quadro: 6
  user chains (plus 8 more for the effects-to-DAW feature where the device enables it); Studio+:
  16. Eight slots each. An empty chain passes its input straight through (**confirmed**).
- **Slots** are `{type, inst}`: `type` is the effect type, **0 for empty**; `inst` indexes that
  type's instances (16 per type; 4 for the Studio+'s Guitar Amp and Cabinet).
- **Reading.** The Quadro reads one chain at a time with `get_afx_strip_order`, chain in `ext3`;
  the Studio+ reads all 16 with `get_afx_order`.
- **Writing.** `set_afx_order(chain, slots[8])` replaces a chain, effects packed from the first
  slot. Inserting sets the instance's `enabled` to 1 and removing sets it to 0 (**confirmed** on
  the Quadro for one effect; reordering, several effects and the Studio+ are not yet tried).
- **Bypass.** `set_afx_bypass(instance, type, enabled)`: 1 processing, 0 bypassed.
- **Links.** `get_afx_links` (Quadro `ext3` 4, Studio+ 5), byte k for chains 2k and 2k+1.
- **Parameters** are a command pair per effect type: `set_<effect>_conf` (`0x70`, payload 21 on
  the Quadro, 20 on the Studio+) and `get_<effect>_conf` (`0x74`, `ext2` 7). Every change resends
  every parameter. `refs/schemas/afx_parameters.json` has ranges, defaults and display rules.
- **Instance counts.** `get_afx_available_instances` gives free instances per type.
- **Reverb**, one per device: `get_reverb_config` and `set_reverb_config`, ranges room size,
  colour and richness 0 to 100, predelay and late reflection delay 0 to 150, reverb level 1 to
  100 shown as `20 log10(v / 25)` dB. `density` always goes out as 100. Quadro returns:
  `set_reverb_return(r, level, mute)`; Quadro sends: `set_reverb_send`, on mix 1's channels. On
  the Studio+ the send is `set_mixer_cfg`'s `send`.

### The effect-meter report

Report `0x83`, pushed about 125 times a second with nothing asked (**confirmed**).

- **Two bytes per effect: peak, then gain reduction.**
- **Quadro: variable length.** The schema declares 304 bytes, but the device sends exactly what
  is loaded (**confirmed**): two bytes per **loaded** effect, chain by chain in chain order, then
  one byte per preamp for the mic emulation meters (4). With nothing loaded it is four bytes of
  96.
- **Studio+: fixed.** 16 chains x 8 peaks, then 16 x 8 gain reductions, where the index is the
  effect's position among the chain's loaded effects, not its slot.
- **The peak byte** is dB below full scale, like every other meter (**confirmed**).
- **The gain-reduction byte's** unit depends on the effect: the vendor shows plain dB for some,
  quarter-dB for others, and a percentage of quarter-dB for others. Gazelle shows the raw value
  until a hardware check settles the scale.
