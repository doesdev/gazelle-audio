# The Recording page

The Recording page (`#/recording`) records the aggregate's inputs to disk: channels from both interfaces in one take, lined up as a DAW recording through [Gazelle Aggregate](13-aggregate-page.md) would get them, one WAV file per channel. While it is armed it keeps the last stretch of audio in memory, so a take can start a little before you press **Record**: the moment you decided was worth keeping is already in it.

Gazelle opens the aggregate itself to do this, with the setup on the Aggregate page, its trims and its phase measurement. You need no DAW, and the rate and buffer size are the aggregate's.

> **Warning.** While the page is armed or recording, Gazelle holds the audio drivers of both interfaces. A DAW may not be able to use them until you disarm. Whether the Antelope drivers let a second program in at the same time has not been tried yet, so do not count on it either way. A measurement on the Aggregate page cannot run while the page is armed, and the page cannot arm while a measurement runs.

## Off, Armed and Recording

The **Transport** at the top shows where the recorder is, in large letters:

- **Off**, with a grey light. Choose a **Preset** and press **Arm**.
- **Armed**, with a hollow red ring that pulses. The interfaces are open, and the **pre-roll** is filling: the bar and the line under it say how many seconds are held and how many it can hold. Nothing is written to disk yet.
- **Count-in**, in amber, when Record was pressed with a [count-in](#a-count-in-before-a-take): the metronome counts its bars before the take starts.
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

## Metronome

The **Metronome** section plays a click into the outputs you choose, from inside the same session the recorder uses, so it is locked to the interfaces' own samples: every click lands on its beat to the sample, however long it runs, and never drifts from the audio you record.

> **Warning.** Running the metronome opens the interfaces through Gazelle Aggregate and holds their audio drivers, exactly as arming does, **even with nothing armed**. A DAW may not be able to use them until you stop it (and disarm), and a measurement on the Aggregate page cannot run while it plays.

**Start** and **Stop** are one button. The first time you press Start in a browser, the section says which outputs it plays to and that it holds the drivers, and starts from there; after that it is one press. Stopping lets the last click ring out. Beside the button, a light for each beat of the bar, the first one larger, shows where the click is.

- **Outputs.** Tick where it plays, interface by interface, named as the Aggregate page names them: one output for mono, or both of a pair, such as the two your headphones are on, to hear it on both sides. Every other output stays silent. The outputs are fixed while the interfaces are open, by the metronome or by Arm: stop the metronome and disarm to change them.
- **Tempo**, in quarter notes a minute, from 20 to 400 in steps of 0.1: type it, step it one BPM at a time with the minus and plus buttons, or **Tap** along with the beat; the tempo is the average of your last few taps, and a pause of two seconds starts the taps again.
- **Time signature.** Beats in a bar, 1 to 16, and the note a beat is: 2, 4, 8 or 16. A click is one beat, so 6/8 clicks eighth notes, as a DAW does, and at a tempo of 120 that is 240 clicks a minute.
- **Accent the first beat of the bar** plays the downbeat as a brighter, louder variant of the sound.
- **Clicks between the beats**: none, or two, three or four quieter ones in each beat (eighths, triplets or sixteenths when a beat is a quarter note).
- **Sound**: click, beep, woodblock, cowbell or tick. Gazelle makes each one itself; there are no sample files.
- **Volume**, in dBFS, from -60 to -6: the peak of the loudest click, the downbeat when it is accented. Other beats are 4 dB under it and the clicks between the beats 12 dB under. It starts at -18 dBFS, and **Gazelle never plays it louder than -6 dBFS**, whatever it is asked, in the engine itself and not only on this page.
- **Preview a bar** plays one bar, 12 dB under the volume, and stops. It works only while armed, so it never opens the interfaces by itself.

A change of tempo, signature or clicks between the beats takes effect at the next beat, and a change of sound, accent or volume at the next click, with no gap and no click of its own. A change of signature starts a new bar.

**The metronome is never in a take** unless you route its outputs back into an input you record, on the Routing page or with a cable.

### A count-in before a take

**Count-in before a take** is 0 to 4 bars. With a count-in, **Record** starts the click if it is not already playing, plays the bars, and starts the take on the downbeat after them; the transport says **Count-in** meanwhile, and which bar it is on. A click that is already playing counts from its next downbeat.

The take starts as if Record had been pressed on that downbeat: it **reaches back into the pre-roll**, which holds the count-in, so nothing you played during it is lost, and the take starts where the pre-roll starts. Its clock counts from the moment you pressed Record. **The downbeat is marked**: every file of the take carries a cue point named **Downbeat** on that sample, which a DAW that reads cue points shows as a marker to snap to, and the take's log says which sample it is.

**Stop during the count-in** cancels it, and no take is started. **Stop** after it ends the take and, when the count-in started the click, stops the click too. A click you started yourself keeps playing until you stop it.

### Follows Record

Tick **Follows Record** and the click plays whenever a take is recording and stops when it does. With a count-in as well, the count-in starts it.

### Where the metronome's settings are kept

In `metronome.json`, in `%APPDATA%\gazelle` beside `recording.json`: one metronome for this computer, not one per preset and not in the workspace. Its outputs are this computer's wiring, so a workspace carried to another computer does not start playing a click into whatever that computer has on those outputs. A phone may start and stop the metronome and change its tempo and volume; everything else about it is changed on the computer.

## Widget, hub and auto-arm

The page's **Widget, hub and auto-arm** section is for a computer that should be ready to record at any moment. It is shown on the computer only: a phone does not see it, and cannot change it. The same things are in the tray menu.

### The recording widget

**Open the recording widget** (or the tray's **Recording widget**) opens a small window, about 320 by 140, that stays on top of every other window and has no title bar:

- the light and the state, **Off**, **Armed** or **Recording**, red while recording, with the time since Record;
- the preset, and the pre-roll held;
- **Auto: Band** (or whichever preset) while auto-arm is on; press it twice to turn auto-arm off;
- one big button for what can be done now, **Arm**, **Record** or **Stop**, and a small **Disarm** beside it, which asks for a second press while recording;
- a small metronome button with its tempo on it, which starts and stops the click, and a light that flashes on every beat;
- a line that says what is wrong, when something is: audio lost, a disk getting full, a refusal, or the connection to Gazelle gone.

Drag it anywhere by its body; the buttons are pressed as usual. It arms with the preset the Transport would offer, or with auto-arm's preset while that is on. The first time Arm is pressed in the widget it reads **Confirm**, and a second press arms: there is no room in it for the explanation the page gives, which its tooltip carries instead.

The widget remembers where it was and how big, and whether it was open: open when Gazelle stops, it opens again at the next start, even a login start in the tray. If the monitor it was on has gone, it comes back to the top right of the main monitor. The **×** at its top right closes it, as the tray item and the page's button do. It scales with Windows' display scaling like any other window.

### The recording hub

**Open the recording hub** (or the tray's **Recording hub**) fills a screen, to be read from across the room:

- along the top, the preset and a clock;
- very large, the state and the time since Record, with the pre-roll under them;
- beside them, how long the disk would keep recording at this rate and number of channels, and the last few takes;
- every channel's meter, by the name Gazelle gives it;
- **Arm**, **Record** and **Stop** as big buttons, and **Disarm** small and off to the side, so it is not pressed by accident. While a take is recording it asks for a second press, as everywhere;
- the metronome, with **Start** and **Stop**, its tempo with **Tap**, and its beat;
- auto-arm, when it is on, with a button that turns it off.

**Space** records and stops, as on the page, even when a button has the focus, so it never presses a button by mistake. **Esc** leaves full screen, and the hub stays open as an ordinary window; **Full screen** (or F11) puts it back, and **Close the hub** closes it. It opens full screen on the monitor it was last on, or on the main monitor if that one has gone.

### Auto-arm

Tick **Auto-arm with** and choose a preset, and Gazelle arms with it:

- whenever Gazelle starts, and
- again when the interfaces drop out and come back. If they are gone for more than five seconds while armed, Gazelle disarms, which finishes any take's files, and arms again when they return. A moment's hiccup does not cut a take.

It does not fight you. **Disarming by hand pauses auto-arm** until Gazelle next starts or you arm again, so a DAW or a measurement on the Aggregate page can have the interfaces. While a measurement runs it waits for it. When arming is refused (a DAW holding the interfaces, a preset that no longer records anything), it says why, on the page, the widget and the hub, and tries again after 5 seconds, then 15, 30, a minute, two, and every five minutes after that; the interfaces coming back, or arming by hand, starts it again straight away. Every try and what came of it is written to the log.

> **Warning.** Always armed means Gazelle always holds the audio drivers of both interfaces, and a DAW may not be able to use them. Whether the Antelope drivers let a second program in at the same time has not been tried yet. Disarm first when a DAW needs the interfaces; auto-arm stays paused until Gazelle next starts.

Auto-arm is off unless you turn it on. **Turn auto-arm off** on the page's Transport, the hub, the widget's **Auto** button, or the tray turns it off again; it does not disarm what is armed. Its preset is kept while it is off, so the tray can turn it back on.

### Start in the recording hub

Tick **Start in the recording hub** and Gazelle opens the hub full screen whenever it starts, including a login start with [Start on boot](05-the-app.md#the-window-and-the-tray). The app's own window still starts in the tray at login: the hub is what a studio computer shows at login, and the app is a click away in the tray.

### Where these are kept

The widget and hub windows remember their places in `recording-widget.json` and `recording-hub.json`, and auto-arm and starting in the hub are kept in `recording.json`, all in `%APPDATA%\gazelle` beside `remote.json`. They are this computer's, not the workspace's: a workspace carried to another computer, or restored from a backup, does not start holding that computer's drivers or filling its screen.

## Quitting while recording

**Quit** in the tray, closing Gazelle from a terminal with Ctrl+C, **Restart to update**, and Windows shutting down, restarting or signing you out all finish any take first. When Windows ends the session while Gazelle is armed, its shutdown screen shows **Finishing a recording** for the moment that takes; Gazelle never stops the shutdown, it only asks for the time to close its files: the files are closed with their sizes written, the log is complete, and the drivers are let go of before Gazelle stops. Ending Gazelle by force (Task Manager's End task, or `taskkill /F`) cannot: each file keeps the audio up to the last whole two seconds, since that is how often its size is written into it, and it opens anywhere; the last moment of the take, up to two seconds, is lost, and the log stops before the line that says how the take ended. Windows lets go of the drivers either way. `taskkill /PID` without `/F` asks Gazelle to quit, and is the ordinary Quit.

## On the emulator

With `--backend loopback` the page records the emulator's test tones from both emulated interfaces, through the real aggregate, into `Gazelle loopback recordings` in the temporary folder, whatever a preset says. It is for trying the page, not for keeping anything.
