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

## The alignment check

At the start of every session the aggregate measures where each interface's capture landed and lines them up ([The phase](13-aggregate-page.md#the-phase)), and then nothing looks again on its own. While Gazelle is armed it keeps looking: **once a second it sends the same short signal down the cable dedicated to the phase measurement**, and where the signal arrives says whether the follower is still exactly where it was lined up. The signal never reaches anything you hear, because that channel is routed to the cable and nowhere else ([The routing it needs](13-aggregate-page.md#the-routing-it-needs)).

A line under the pre-roll says what the check found:

- **Alignment checked 0.4 s ago: held.** The last signal arrived on the same sample as the one before it. This is what you want to see.
- **Alignment slipped by 32 samples at 1:23.5 into this take**, in the warning colour, with which interface is late or early. One interface's tracks have moved against the other's from that moment on. **Nothing is corrected**: the take keeps what was recorded, and its log says when it happened and by how much, so you can move that interface's tracks in your DAW. If it moves back, the line says it held again, and still mentions the slip.
- **Alignment is not being confirmed: no check signal has arrived**, in the warning colour, when the signal has stopped arriving: the cable is out, or its routing changed. For that stretch nobody knows whether the interfaces held.
- **Alignment is not being checked**, and why: no phase path is set up on the Aggregate page, the aggregate is set to the lowest latency (which lines nothing up), or the measurement at the start of the session heard nothing on the cable. In that last case nothing is sent at all, because a signal that never reached the cable is going wherever that channel is routed. Fix the cable or its routing, then disarm and arm again.

The check is compared with the alignment in force: the phase measured at the start of the session when the interfaces were lined up by it, or the reference its trim was measured at when that measurement was refused, so a measurement that went wrong at the start shows at the very first check.

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

The log, named like the files with `log` for the channel, says when the take started, its preset, rate and format, how much of it was pre-roll, its files, and anything that happened to it: audio the disk fell behind on (each gap is silence of the same length, so the files stay lined up, and the log says where in the take it was), audio the aggregate lost on an interface, whether the interfaces stayed lined up, and why the take ended if Gazelle ended it.

On the alignment, the log gives the answer first, in a line: **Alignment held: 48 checks, all at 0 samples**, or that it did not hold, with how many checks found the follower out of line and where it was at the end, or that some checks found no signal and so it is not known for those stretches, or that it was not checked and why. The lines after it say when: the time into the take and the sample where it slipped, by how many samples and which way, when it came back, and each stretch where no check signal arrived.

The **Takes** list shows the takes recorded since Gazelle started, newest first, with their folder and files, and a warning on a take the interfaces slipped out of line during.

## Into Cubase

With a **Cubase seed** set, every take also gets a **Cubase track archive** beside its files: an XML file named like them with `Cubase` for the channel, such as `2026-09-27 T001 Cubase.xml`. File > Import > Track Archive brings the whole take into a Cubase project in one step: a folder track with its own group channel, and in it one mono track per channel of the take, named after the channel, routed to that group, with its recording starting at the project start. The tracks play the take's own WAV files where they are; nothing is copied.

### The seed

The seed is a track archive you export once from your own tracking template, and every take's archive is made from it. So everything you set up on the template's tracks, their inserts, sends, colours and the routing into the group, is on the take's tracks too.

1. Open a project made from your tracking template. Its recording folder is a folder with group ("Recorded"), with at least one mono audio track in it ("Audio 01"), routed to that group.
2. Record a few seconds on that mono track. The seed needs a track with a recording on it, because the recording is what Gazelle copies for each channel. It does not matter what you record.
3. Select the folder track itself and choose File > Export > Selected Tracks. Save the archive somewhere it will stay, such as `C:\Cubase\Recorded.xml`. Referencing the files or copying them makes no difference to Gazelle.
4. On the Recording page, under **Into Cubase**, paste the archive's whole path and press Enter (a path in quotes, as Explorer's Copy as path gives it, is fine). Gazelle checks it at once and says what it found: the folder, its group, and how many mono tracks with a recording it can copy. A file that will not do is refused with the reason, and nothing changes.

If the folder has several mono tracks, the take's first channel gets a copy of the first, the second of the second, and so on; once they run out, the last one is used again. The seed's own recordings and any stereo tracks in it are left out.

**No seed** stops the archives. The seed is this computer's, kept in `recording.json` with auto-arm, and is set on the computer only. Gazelle reads the seed again for each take, so a seed set while armed counts from the next take, and a seed file that has since changed or gone is shown on the page.

### Importing a take

1. Open a project made from your tracking template, at the rate the take was recorded at. It can keep its Recorded folder.
2. Choose File > Import > Track Archive and pick the take's `Cubase.xml`.
3. Cubase asks how to bring the archive in. Point the archive's folder at the project's existing Recorded folder: the take's tracks are added inside it, one per channel, and no second folder or group is made. Leave copying the files into the project off, so the tracks keep playing the take's files in their own folder. If Cubase offers to convert the files, the take's rate or format differs from the project's: make the project match instead.

Each track is named after its channel, and its recording starts at the very start of the project and plays the take's own file.

This was first done in Cubase on 2026-09-30, with all 28 inputs of a take. Not yet tried: a take recorded in 32-bit float. Gazelle names the format as Cubase does for 24-bit files.

If a take's archive cannot be made, the take is not affected: its files and log are complete, and the log says why there is no archive.

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

The take starts as if Record had been pressed on that downbeat: it **reaches back into the pre-roll**, which holds the count-in, so nothing you played during it is lost, and the take starts where the pre-roll starts. Its clock counts from the moment you pressed Record. **The downbeat is marked**: every file of the take carries a cue point named **Downbeat**, which a DAW that reads cue points shows as a marker to snap to, and the take's log says which sample it is.

**The marker is where your playing lands, not where the click was sent.** You hear the click a little after Gazelle sends it (the output latency), and what you play in time with it reaches the take a little after that (the input latency), so a performance in time with the click sits one round trip later in the files. Gazelle places the Downbeat that much after the click: by the output and input latencies Gazelle Aggregate reports, which are the figures a DAW is given for the same job (each interface's own driver figures, a block for the interface that does not drive the callback, the trims a measurement on the Aggregate page wrote, and the phase measured in this session), plus the **Latency offset** below. The audio itself is not moved: the files are what the inputs received, as in a DAW. The take's log says how far the marker was placed and from what, for example `Downbeat placed 2362 samples after the click: output 1311 + input 1051 reported by the aggregate, offset 0 (0.00 ms).` If the drivers report no latency, the log says so and only the offset is used.

**Latency offset**, in milliseconds from -100 to 100 and 0 to start with, moves the marker further on (or back, when negative) for what the drivers cannot know: a converter's own delay, monitoring through a mixer, how far you sit from a speaker. To set it, record a count-in with the click routed back into an input you record (or play a sharp sound exactly on the click), and look at where it lands against the Downbeat in your DAW: if it is late, add that many milliseconds; if early, take them off. A take uses the offset set when it finishes.

**Stop during the count-in** cancels it, and no take is started. **Stop** after it ends the take and, when the count-in started the click, stops the click too. A click you started yourself keeps playing until you stop it.

### Follows Record

Tick **Follows Record** and the click plays whenever a take is recording and stops when it does. With a count-in as well, the count-in starts it.

### Where the metronome's settings are kept

In `metronome.json`, in `%APPDATA%\gazelle` beside `recording.json`: one metronome for this computer, not one per preset and not in the workspace. Its outputs and its latency offset are this computer's wiring, so a workspace carried to another computer does not start playing a click into whatever that computer has on those outputs. A phone may start and stop the metronome and change its tempo and volume; everything else about it is changed on the computer.

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

The widget and hub windows remember their places in `recording-widget.json` and `recording-hub.json`, and auto-arm, starting in the hub and the [Cubase seed](#into-cubase) are kept in `recording.json`, all in `%APPDATA%\gazelle` beside `remote.json`. They are this computer's, not the workspace's: a workspace carried to another computer, or restored from a backup, does not start holding that computer's drivers or filling its screen.

## Quitting while recording

**Quit** in the tray, closing Gazelle from a terminal with Ctrl+C, **Restart to update**, and Windows shutting down, restarting or signing you out all finish any take first. When Windows ends the session while Gazelle is armed, its shutdown screen shows **Finishing a recording** for the moment that takes; Gazelle never stops the shutdown, it only asks for the time to close its files: the files are closed with their sizes written, the log is complete, and the drivers are let go of before Gazelle stops. Ending Gazelle by force (Task Manager's End task, or `taskkill /F`) cannot: each file keeps the audio up to the last whole two seconds, since that is how often its size is written into it, and it opens anywhere; the last moment of the take, up to two seconds, is lost, and the log stops before the line that says how the take ended. Windows lets go of the drivers either way. `taskkill /PID` without `/F` asks Gazelle to quit, and is the ordinary Quit.

## On the emulator

With `--backend loopback` the page records the emulator's test tones from both emulated interfaces, through the real aggregate, into `Gazelle loopback recordings` in the temporary folder, whatever a preset says. It is for trying the page, not for keeping anything. With a phase path set up, the emulator carries the phase cable too, so the alignment check runs and holds.
