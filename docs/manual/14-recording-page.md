# The Recording page

The Recording page (`#/recording`) records the aggregate's inputs to disk: channels from both interfaces in one take, lined up as a DAW recording through [Gazelle Aggregate](13-aggregate-page.md) would get them, one WAV file per channel. While it is armed it keeps the last stretch of audio in memory, so a take can start a little before you press **Record**: the moment you decided was worth keeping is already in it.

Gazelle opens the aggregate itself to do this, with the setup on the Aggregate page, its trims and its phase measurement. You need no DAW, and the rate and buffer size are the aggregate's.

> **Warning.** While the page is armed or recording, Gazelle holds the audio drivers of both interfaces. A DAW may not be able to use them until you disarm. Whether the Antelope drivers let a second program in at the same time has not been tried yet, so do not count on it either way. A measurement on the Aggregate page cannot run while the page is armed, and the page cannot arm while a measurement runs.

## Off, Armed and Recording

The **Transport** at the top shows where the recorder is, in large letters:

- **Off**, with a grey light. Choose a **Preset** and press **Arm**.
- **Armed**, with a hollow red ring that pulses. The interfaces are open, and the **pre-roll** is filling: the bar and the line under it say how many seconds are held and how many it can hold. Nothing is written to disk yet.
- **Recording**, in red, with a solid red light and the time since you pressed Record.

**Arm** opens the interfaces and reserves the memory for the pre-roll. The first time you press it in a browser, the page says what it does and asks once; after that it is one press. If it cannot arm, it says why in a sentence: a channel the aggregate does not have, too little free memory, an interface that would not open, or a measurement running.

**Record** starts a take. The take begins with the oldest audio the pre-roll still holds and runs on into what is being played now, with nothing missing between the two. **Stop** ends it and finishes its files; the page stays armed, and the pre-roll starts filling again for the next take. A take never reaches back into audio the previous take already has, so takes recorded one after another follow each other with no gap and no overlap.

Record and Stop are each one press, taken at the next block of audio: missing the start of a take is the worse mistake. While the Recording page is shown and no field or button has the keyboard focus, **Space** does the same: Record while armed, Stop while recording. It never arms.

**Disarm** lets go of the drivers and gives the memory back. While a take is being recorded it stops the take too, so it asks for a second press within a few seconds.

Below the buttons, the transport warns when something needs you: audio lost because the disk fell behind, audio the aggregate itself lost on an interface, a disk getting full, or a driver asking to be restarted (disarm and arm again when you can; the take so far is safe).

## The pre-roll

A preset asks for a share of the memory that is free when you press Arm: 10 % unless you choose otherwise. That is worked out into seconds from the aggregate's rate and the number of channels, and shown beside the bar. It is always at least 5 seconds, and never more than half of the free memory or 4 GB. A quarter of what is reserved is kept back as room to write the take out while the audio keeps coming, so the seconds shown are the part Record can reach back into.

**Keep at most** in a preset limits the pre-roll to that many seconds, and reserves less memory to match.

The memory is taken at Arm, all of it at once, so nothing about it can fail later in the middle of a take.

## Channels

While armed, each channel being recorded, named as the aggregate names it (the name a DAW would show, and the name its file takes), with its level.

## Presets

A preset says what to record and how:

- **Name.**
- **The channels**, ticked interface by interface, named as on the Aggregate page. Channels from both interfaces go into one take. An input the Aggregate page keeps for the phase measurement is not offered.
- **Format**: 24-bit integer, what the converters produce and the default, or 32-bit float.
- **Pre-roll memory**: the share of free memory, and **Keep at most**, in seconds.
- **Folder**: where the files go, as a whole path. Empty means `Gazelle Recordings` in your Documents folder. It is made if it is not there.
- **File names**: a pattern. `{channel}` is the channel's name, `{take}` the take's number, `{date}` and `{time}` when the take's first sample was recorded, and `{preset}` the preset's name; `{channel}` and `{take}` are needed. Empty means `{date} T{take} {channel}`, which gives `2026-09-27 T001 Vocal mic (Quadro 1).wav`.

Presets live in the [workspace](12-workspace-page.md), so they travel with a backup. A preset cannot be changed while Gazelle is armed with it; disarm first. Presets are made and changed on the computer: a phone can choose one and arm with it, but not edit it, because a preset names a folder on the computer.

## Takes and their files

Each take is one mono Broadcast WAV file per channel, and a log beside them. Every file of a take starts at the same moment and says so in its own header, as a time of day counted in samples, so a DAW that imports them together puts them at the same place on its timeline. A file that grows past 4 GB carries on as RF64, which DAWs read the same way.

**A file is never overwritten.** If a name is taken, the take number moves on until every name of the take is new.

The log, named like the files with `log` for the channel, says when the take started, its preset, rate and format, how much of it was pre-roll, its files, and anything that happened to it: audio the disk fell behind on (each gap is silence of the same length, so the files stay lined up, and the log says where in the take it was), audio the aggregate lost on an interface, and why the take ended if Gazelle ended it.

The **Takes** list shows the takes recorded since Gazelle started, newest first, with their folder and files.

## The disk

Gazelle checks the free space when you press Record and refuses a take with less than a minute of room. While recording it looks every second: below about ten minutes of room the transport warns, and before the disk is full it stops the take the ordinary way and finishes its files while there is still room to. If a write fails anyway, the take stops, and what was written is kept. Each file's size is written into its header every two seconds, so a file cut short by a crash still opens.

## On the emulator

With `--backend loopback` the page records the emulator's test tones from both emulated interfaces, through the real aggregate, into `Gazelle loopback recordings` in the temporary folder, whatever a preset says. It is for trying the page, not for keeping anything.
