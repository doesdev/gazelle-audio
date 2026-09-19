// The explain mode's text: one entry per key an element carries in `data-explain`. A chunk of its own,
// fetched the first time the mode is turned on (`explain.ts`), so the app does not carry it for
// everyone who never does. Nothing else may import it statically; `test/bundle-split.test.ts` checks.
//
// Written from the app's code and the decision log. Where what a control does on the hardware has not
// been measured, an entry says what the app knows and stops there. `{name}` is the element's own
// name for the thing ("Preamp 3", "HP1", "Vox"), or the entry's `name` when it gives none.
//
// Keys are whole literals in the elements, never put together from parts, so `test/explain.test.ts`
// can find every one; `e2e/explain-coverage.spec.ts` walks every page and fails on a control with no
// key, or a key with no entry here.

import type { Catalogue } from "./explain-rules.ts";

/** Keys with an entry that nothing in the elements uses yet, for a section being built elsewhere. */
export const RESERVED_KEYS: readonly string[] = [];

const STEP_KEYS = "Arrow keys and the wheel move it a step at a time, Page Up and Page Down in bigger steps, and a double-click puts it back to its starting value.";
const LEVEL_KEYS = "Arrow keys and the wheel move it a step at a time, Page Up and Page Down in bigger steps. A double-click sets -20 dB, or 0 dB when the header's Double-click menu says unity; Ctrl or Cmd and a click always sets 0 dB.";
const NOTHING_SENT = "Nothing is sent to a device.";
const WORKSPACE = "It is kept in the workspace on the server, so everyone using this server sees the same.";

export const CATALOGUE: Catalogue = {
  // The header.
  "header.explain": {
    title: "Explain mode",
    what: "Turns these explanations on and off. While it is on, pointing at or focusing anything shows what it is, what changing it does and what to watch for.",
    effect: "Remembered in this browser only. It changes nothing on a device.",
  },
  "header.explain-tap": {
    title: "Explain by tapping",
    what: "For a screen with no hover. While it is lit, a tap or click on a control explains it instead of using it.",
    effect: "Tap it again, or press Escape, to use the controls again.",
    watch: "While it is lit, taps do nothing but explain, so a button will not press and a slider will not move.",
  },
  "header.page.devices": { title: "Devices page", what: "One device's identity, name, clock, settings, test oscillator, presets and power." },
  "header.page.workspace": { title: "Workspace page", what: "What this server keeps for everyone: device names and colours, surfaces, digital cables, groups, snapshots, and backup." },
  "header.page.inputs": { title: "Inputs page", what: "Every hardware input on the device: preamp type, gain, 48V and phase, digital input gains, and on the Quadro, mic emulation." },
  "header.page.outputs": { title: "Outputs page", what: "Every hardware output's volume, mute and (Quadro) dim, the output trims, and on the Studio+, talkback." },
  "header.page.mixer": { title: "Mixer page", what: "The device's four hardware mixes, as channels you set up, each with its fader, pan, mute and solo in the mix you pick." },
  "header.page.routing": { title: "Routing page", what: "Which source feeds every destination on the device, as the vendor panel's routing tab lays it out." },
  "header.page.effects": { title: "Effects page", what: "The device's effect chains and its reverb: what is loaded, bypass, each effect's settings, and the reverb's sends and returns." },
  "header.backend": {
    title: "Backend",
    what: "What this server is driving: usb is the real interfaces over USB; loopback is the built in emulator, which touches no hardware.",
    watch: "On usb every control acts on the device you are listening through.",
  },
  "header.dry-run": {
    title: "Dry run",
    what: "The server was started in dry run: every command reports the bytes it would send, and nothing is written to a device.",
    effect: "Controls still move, so you can see what they would do. Reads answer nothing, so values start at defaults.",
  },
  "header.connection": {
    title: "Connection",
    what: "Whether this page is connected to the server. While it is not, every control is disabled until it reconnects.",
  },
  "header.theme": { title: "Theme", what: "The colours the app is drawn in. Remembered in this browser only." },
  "header.double-click": {
    title: "Double-click on a level",
    what: "What a double-click does to a level: its safe level (-20 dB on a fader, -30 dB on a volume, off on a reverb send), or unity. Ctrl or Cmd and a click always sets unity.",
    effect: "Remembered in this browser only. The wheel does not step this menu, and a phone does not show it, having no double-click.",
  },

  // The shell.
  "app.menu": { title: "Sidebar", what: "Opens the sidebar over the page: device cards, the output meters and the Control Room. Escape or a tap outside closes it." },
  "app.disconnected": { title: "Not connected", what: "The server is not answering. Controls stay disabled until the page reconnects, which it tries on its own." },
  "app.page-retry": { title: "Try again", what: "Reloads the app, since a browser will not fetch a page's code again in the same document after it failed once." },
  "sidebar.move": { title: "Move the sidebar", what: "Docks the sidebar on the other side of the page. Remembered in this browser." },
  "sidebar.fold": { title: "Fold the sidebar", what: "Folds the sidebar to a narrow rail, or opens it again. Remembered in this browser." },
  "sidebar.close": { title: "Close the sidebar", what: "Closes the sidebar drawer and hands focus back to the menu button." },
  "sidebar.devices": { title: "Devices", what: "A card per connected device. Picking one switches the page you are on to that device." },
  "sidebar.meter": { title: "Meter", what: "The output levels of the device on screen, with a clip light per output and how soon clip lights clear themselves." },
  "sidebar.control-room": { title: "Control Room", what: "The outputs you chose on the Outputs page, each with volume, mute, dim (Quadro) and mono, and on the Studio+, talkback." },

  // Device cards.
  "devicelist.card": {
    title: "{name}",
    name: "Device",
    what: "A connected device, by the name you gave it. Its state at a glance comes from its status report.",
    effect: "Picking it switches the page you are on to this device. On the Workspace page it selects the device without leaving.",
  },
  "devicelist.unknown": { title: "Unknown model", what: "The server does not know this device's model, so only its Devices page can show it and nothing it reports can be decoded." },
  "devicelist.input": {
    title: "Input activity",
    what: "Lit when any hardware input has signal above -60 dBFS, and red when one clips, held for 1.5 s so a short clip is still seen.",
    effect: "Covers the preamps, S/PDIF and ADAT, and on the Studio+ the line inputs.",
  },
  "devicelist.clock": { title: "Clock", what: "The sample rate the device measures (or the rate it is set to, before one is measured), and whether it is locked to its clock source." },
  "devicelist.lock": { title: "Lock", what: "LOCKED when the device reports itself locked to its clock source, NO LOCK when it does not." },
  "devicelist.state": { title: "Power and preset", what: "Whether the device is on or in standby, and which of its five presets it is on." },

  // The Meter section.
  "meter.output": {
    title: "{name} meter",
    name: "Output",
    what: "The level leaving {name}, left and right, in dB below full scale, from 0 down to -60.",
    effect: "The bar rises fast and falls back steadily. The bright marker holds the loudest recent level for 1.5 s, then falls. The figure is that held peak.",
  },
  "meter.output-clip": {
    title: "{name} clip light",
    name: "Output",
    what: "Lights when either side of {name} reaches full scale.",
    effect: "Click it to clear it. It also clears by itself after the time chosen above, counted from when the clip ends.",
  },
  "meter.auto-clear": { title: "Clip lights clear after", what: "How long after a clip ends every clip light clears itself, on the strips too. Hold keeps them lit until cleared.", effect: "Remembered in this browser." },
  "meter.clear-all": { title: "Clear all clip lights", what: "Clears every clip light on every device: outputs, strips and effects." },
  "meter.none": {
    title: "No output meters",
    what: "The Studio+ reports its output levels only through a meter bank that can be pointed at different sources. The app does not point it, since that is a write to the device a page would make on its own.",
  },

  // The Control Room.
  "cr.device": { title: "Control Room device", what: "The device the Control Room shows: the one on the page you are on, or the one last selected." },
  "cr.volume": {
    title: "{name} volume",
    name: "Output",
    what: "The hardware volume of {name}, in dB of attenuation: 0 dB at the right, down to off at the left.",
    effect: "Sends the new volume straight to the device, the same control as the Outputs page's. A double-click sets -30 dB, or 0 dB when the header's Double-click menu says unity; Ctrl or Cmd and a click always sets 0 dB.",
    watch: "This is the volume of what you are listening to: a drag to the right is as loud as the output goes.",
  },
  "cr.mute": { title: "{name} mute", name: "Output", what: "Mutes {name} on the device. Its volume is kept and comes back when unmuted." },
  "cr.dim": {
    title: "{name} dim",
    name: "Output",
    what: "Dims {name}: the Quadro lowers the output by its own dim amount while this is lit.",
    watch: "How many dB the device dims by is its own; the app has not measured it.",
  },
  "cr.mono": {
    title: "{name} mono",
    name: "Output",
    what: "Neither model can make an output mono, so this sums the mix that feeds {name} to mono: every channel in that mix is panned to centre, and panned back when it is turned off.",
    effect: "Every other output that plays the same mix goes mono with it. The button's own tooltip names them.",
    watch: "It centres the mix's pans on the device, and the panning law (Quadro) sets how loud the centred sum is. A mix that also feeds a recording input records mono meanwhile. Disabled when no one mix feeds this output.",
  },
  "cr.mono-badge": { title: "MONO", what: "The Quadro reports {name} as mono. The app can show this but has no command to change it.", name: "this output" },
  "cr.feed": { title: "What feeds {name}", name: "this output", what: "The mix routed to {name}, or the sources it plays straight, or Muted, from the device's routing." },
  "cr.talk": {
    title: "Talk",
    what: "The Studio+ talkback: on only while you hold it (pointer, Space or Enter), off as soon as you let go.",
    effect: "Sends the talkback microphone to the outputs lit under To, at the talkback level.",
  },
  "cr.talk-level": {
    title: "Talkback level",
    what: "How loud talkback is, as a fader on the outputs' scale: 0 dB at the right, off at the left.",
    effect: "A double-click sets -30 dB, or 0 dB when the header's Double-click menu says unity; Ctrl or Cmd and a click always sets 0 dB.",
  },
  "cr.talk-to": { title: "Talkback to {name}", name: "this output", what: "Whether talkback is heard on {name} while Talk is held." },

  // The mixer dock.
  "dock.section": { title: "Mixer dock", what: "The mix you picked on the Mixer page as slim strips, so levels can be ridden from any page. Hidden on the Mixer page itself. Whether it is folded is remembered in this browser." },
  "dock.source": { title: "Dock shows", what: "This device's selected mix, or one of your surfaces, so strips from any device can sit under every page. Remembered in this browser." },
  "dock.mix": { title: "Dock mix", what: "Which of the device's mixes the dock shows. It is the same choice as the Mixer page's Mix menu: changing one changes the other." },
  "dock.open-mixer": { title: "Open the Mixer page", what: "No channel is set up in this mix yet; channels are set up on the Mixer page." },
  "dock.open-surface": { title: "Open the surface", what: "This surface has no strips yet; strips are added on the surface's own page." },

  // Pages in general.
  "page.last-sent": {
    title: "Last command",
    what: "The last command this page sent to the device and its bytes, or in dry run the bytes it would have sent.",
    effect: "It appears only while this explain mode is on, or while the server is in dry run; the rest of the time it stays out of the way.",
  },

  // A mixer strip.
  "strip.fader": {
    title: "{name} fader",
    name: "Channel",
    what: "{name}'s level in the mix you picked, from 0 dB at the top down to -90 dB. The scale is drawn like the meters, so each 10 dB down takes less room.",
    effect: "Changes this channel's level in this mix only; the other mixes keep theirs. A channel linked with it follows. " + LEVEL_KEYS,
    watch: "-90 dB is the floor, not silence: to take a channel out of a mix, mute it.",
  },
  "strip.master-fader": {
    title: "{name} master fader",
    name: "Mix",
    what: "The master level of {name}: everything the mix plays goes through it, from 0 dB down to -90 dB.",
    effect: "Every output and recording input that plays this mix gets louder or quieter together. " + LEVEL_KEYS,
  },
  "strip.pan": {
    title: "{name} pan",
    name: "Channel",
    what: "Where {name} sits between left and right in this mix, from L 100% through C to R 100%.",
    effect: "Dragging snaps to centre near the middle, as the vendor panel's knob does; the wheel and arrow keys step about 3% at a time through it. Double-click centres it. Each channel keeps its own pan, even when linked.",
    watch: "While the mix is mono, the device has the channel centred and this shows the pan it goes back to. On the Quadro the panning law sets how loud a centred channel is.",
  },
  "strip.send": {
    title: "{name} reverb send",
    name: "Channel",
    what: "How much of {name} goes to the Studio+'s reverb, in dB of attenuation: 0 dB at the right, off at the left. Only on Mix 1, as the vendor panel has it.",
    effect: "A double-click turns it off, so it never adds reverb, or sets 0 dB when the header's Double-click menu says unity; Ctrl or Cmd and a click always sets 0 dB. The reverb's level on the Effects page is its return.",
  },
  "strip.mute": {
    title: "{name} mute",
    name: "Channel",
    what: "Mutes {name} in this mix only. Its fader stays where it is.",
    effect: "A channel linked with it follows.",
  },
  "strip.master-mute": {
    title: "{name} master mute",
    name: "Mix",
    what: "Mutes {name} at its master, so the mix goes quiet on every output and recording input it plays on.",
  },
  "strip.solo": {
    title: "{name} solo",
    name: "Channel",
    what: "Sets {name}'s solo in this mix, as the vendor panel's S button does.",
    watch: "How the device plays a solo, which channels it silences and on which outputs, is the device's own behaviour; the app has not measured it.",
  },
  "strip.meter": {
    title: "{name} meter",
    name: "Channel",
    what: "The signal arriving at {name}'s input, before its fader, in dB below full scale from 0 down to -60. Every strip on the same input shows the same meter.",
    effect: "The bar rises fast and falls back at 12 dB a second. The bright marker holds the loudest recent level for 1.5 s, then falls.",
    watch: "It is the input, not the mix: a muted or pulled down channel still shows its input. A strip on an effect return shows its chain's last effect, or what feeds an empty chain.",
  },
  "strip.clip": {
    title: "{name} clip light",
    name: "Channel",
    what: "Lights when {name}'s input reaches full scale. It belongs to the input, so every strip on that input shares it.",
    effect: "Click it to clear it; it also clears by itself after the time set in the Meter section.",
  },
  "strip.level": { title: "Level", what: "The fader's level in dB." },
  "strip.peak": { title: "Peak", what: "The loudest recent level on the meter, in dB below full scale: the value the peak marker holds. Below -60 it reads < -60." },
  "strip.doubled": {
    title: "Summed twice",
    what: "The same audio reaches this mix twice: two channels on one input, or a channel whose empty effect chain passes the other's input straight through.",
    effect: "Summing a signal with itself makes it about 6 dB louder. The badge's own tooltip names the source.",
    watch: "Mute one of the two, or load an effect in the chain, and the badge goes. Dry and wet through a chain with an effect in it is not warned about: that is a parallel setup, and a different signal.",
  },
  "strip.name": {
    title: "{name}",
    name: "Channel",
    what: "The channel's name and colour. The colour is its group's if the group has one, else its own, else its input's colour from the Routing page.",
    effect: "Greyed when the channel is not set up in the mix you picked.",
  },

  // A channel's head on the Mixer page.
  "channel.grip": { title: "Drag to move", what: "Drag the channel by this grip to put it somewhere else in the row. Dropped between two channels of a group, it joins the group. " + NOTHING_SENT },
  "channel.move-left": { title: "Move left", what: "Moves the channel one place left in the row. Only the layout changes. " + NOTHING_SENT },
  "channel.move-right": { title: "Move right", what: "Moves the channel one place right in the row. Only the layout changes. " + NOTHING_SENT },
  "channel.remove": {
    title: "Remove {name}",
    name: "the channel",
    what: "Takes the channel off the mixer. Click twice: the first click arms it for 3 s.",
    watch: "Its input is muted in every mix it fed, so the audio stops there.",
  },
  "channel.name": { title: "Channel name", what: "The channel's name. Left empty, it goes by its input's name. " + WORKSPACE + " " + NOTHING_SENT },
  "channel.colour": { title: "Channel colour", what: "Shows the strip's colour and opens a picker: the theme's colours, any colour, or Clear to go back to the default. A group's colour wins while the channel is in a group with one. " + NOTHING_SENT },
  "channel.input": {
    title: "Channel input",
    what: "Which source feeds this channel: a preamp, a digital input, the computer's playback, an effect return.",
    effect: "Routes that source to the channel's mixer input in its main mix and every mix it is sent to, and mutes it in the rest. No input mutes it everywhere. The wheel does not step this menu, since each step would route.",
  },
  "channel.main-mix": {
    title: "Main mix",
    what: "The mix this channel belongs to. A channel works once it has an input and a main mix.",
    effect: "Routes the channel into that mix. Other mixes can take it too, with Add to on the channel. The wheel does not step this menu.",
  },
  "channel.group": { title: "Group", what: "Puts the channel in a group, or makes a new one. Grouped channels sit together under a coloured band that can fold them away. " + NOTHING_SENT },
  "channel.in-mix": {
    title: "In this mix",
    what: "Whether the channel is in the mix you picked. Its main mix reads Main mix; any other mix can take it as a send.",
    effect: "Add routes the channel's input into this mix; taking it out mutes it there. Its level in each mix is its own.",
    watch: "Adding a channel whose input the mix already has takes two clicks: the first reads Confirm and says what would be summed twice.",
  },
  "channel.doubling-confirm": {
    title: "Confirm",
    what: "The choice beside this would put one input into a mix twice, so that mix would sum it twice, about 6 dB louder. Press Confirm to do it anyway, or pick something else.",
    effect: "Nothing has been sent yet. Left alone for a few seconds, the menu goes back to what it had.",
    watch: "Dry into a mix alongside the same signal through an effect chain is a parallel setup, not a doubling, and is not asked about.",
  },
  "channel.pre-type": {
    title: "{name} type",
    name: "Preamp",
    what: "What {name} expects: Mic, Line, or Hi-Z for an instrument, on the preamps that have it. The same control as the Inputs page's.",
    effect: "Changes the gain range: Mic 0 to 65 dB, Line -6 to +20 dB, Hi-Z 0 to 40 dB.",
    watch: "48V works on Mic only.",
  },
  "channel.pre-gain": {
    title: "{name} gain",
    name: "Preamp",
    what: "{name}'s gain in whole dB, shared with the Inputs page and every channel on this preamp.",
    effect: "Raises or lowers the level before anything else, so every mix and recording of this input follows. " + STEP_KEYS,
    watch: "Too much clips the converter: watch the clip light.",
  },
  "channel.48v": {
    title: "{name} 48V",
    name: "Preamp",
    what: "Phantom power for a condenser microphone on {name}. Mic type only.",
    effect: "Turning it on takes a second click (or Ctrl or Cmd and a click); turning it off is one.",
    watch: "48V can damage a ribbon microphone or gear wired unbalanced, and switching it makes a loud thump: turn the monitors down first.",
  },
  "channel.phase": { title: "{name} phase invert", name: "Preamp", what: "Flips the polarity of {name}'s signal.", effect: "Useful when two microphones on one source cancel each other; on its own it sounds the same." },
  "colour.swatch": { title: "Palette colour", what: "Gives the channel this colour from the theme. " + NOTHING_SENT },
  "colour.custom": { title: "Custom colour", what: "Gives the channel any colour you pick. " + NOTHING_SENT },
  "colour.clear": { title: "Clear the colour", what: "Takes the channel's own colour away, so it shows its input's colour again." },

  // A group band.
  "group.toggle": { title: "Fold the group", what: "Folds the group's channels into a narrow tile, or opens them again. Their levels are untouched. " + NOTHING_SENT },
  "group.name": { title: "Group name", what: "The group's name. " + WORKSPACE },
  "group.colour": { title: "Group colour", what: "The colour every channel in the group shows, over their own." },
  "group.remove": { title: "Remove the group", what: "Removes the group; its channels stay, ungrouped. " + NOTHING_SENT },

  // A mix's master head.
  "master.name": { title: "Mix name", what: "The mix's name, shown in menus and on its master. " + WORKSPACE + " " + NOTHING_SENT },
  "master.mono": {
    title: "Mono",
    what: "Sums this mix to mono. Neither model has a mono switch, so the app pans every channel in the mix to centre and keeps their pans to put back when it is turned off.",
    effect: "Every output and recording input playing this mix goes mono.",
    watch: "No level is changed: on the Quadro the panning law sets how loud the centred sum is.",
  },
  "master.outputs": { title: "Where this mix plays", what: "Each output pair this mix is routed to, from the device's routing." },
  "master.output": { title: "{name}", name: "Output", what: "This mix plays on {name}." },
  "master.output-stop": { title: "Stop feeding {name}", name: "this output", what: "Mutes this mix's route to {name} on the device.", watch: "If that is the output you are listening on, it goes quiet." },
  "master.add-output": {
    title: "Add an output",
    what: "Routes this mix to another output pair: a hardware output, or one of the computer's recording inputs.",
    effect: "Choosing one sends the routing to the device at once. The wheel does not step this menu, since every step would add an output.",
    watch: "Whatever that output played before is replaced by this mix.",
  },

  // The link bar and badges.
  "link.mixer": {
    title: "Link",
    what: "Links this channel with others, on this device or another, so a change made here goes to every member: level, mute and solo, in each mix. Pan stays per channel.",
    effect: "Opens the link bar: pick the other channels, choose Same value or Relative, and Save. A number shows which link it is in.",
    watch: "Links are the app's, kept in the workspace. Changes made on the device or in the vendor panel are not repeated to the others.",
  },
  "link.input": {
    title: "Link",
    what: "Links this input with others of its kind, on this device or another, so a change made here goes to every member. A type change goes to every member or to none.",
    effect: "Opens the link bar: pick the other inputs, choose Same value or Relative, and Save. A number shows which link it is in.",
    watch: "48V turned on through a link reaches every member on Mic, and its confirmation says how many.",
  },
  "link.bar": { title: "Link bar", what: "The link being made or changed: its members, and whether they share one value or move by the same step. Pick badges to add or remove members." },
  "link.mode-absolute": { title: "Same value", what: "Every member of the link takes the same value." },
  "link.mode-relative": { title: "Relative", what: "Every member moves by the same step, keeping the offsets between them, each held inside its own range." },
  "link.save": { title: "Save the link", what: "Makes the link, kept in the workspace. Where a link joins exactly one pair on a device, the device's own link flag is set to match, which changes only the vendor panel's controls, not the audio." },
  "link.unlink": { title: "Unlink", what: "Removes the link. Every member keeps its current value." },
  "link.cancel": { title: "Cancel", what: "Closes the link bar without changing anything." },

  // The Mixer page.
  "mixer.mix": {
    title: "Mix",
    what: "Which of the device's four hardware mixes the page shows. One mix is always chosen. Every strip's fader, pan, mute and solo act on that mix, and the meters follow it; each channel keeps its own settings in each mix.",
    effect: "Remembered per device and put in the address, and shared with the mixer dock. Choosing a mix sends nothing.",
  },
  "mixer.show-all": {
    title: "Show all channels",
    what: "Off, the page shows only the channels routed to the chosen mix. On, it shows every channel you have made, with the ones outside this mix dimmed and unmetered, so you can move one in from its head.",
    effect: "Remembered per device in this browser. It changes nothing on the device.",
  },
  "mixer.notes": { title: "How this mixer works", what: "A short note on channels, the mix the faders act on, which channels are shown, and whether the device's levels have been read." },
  "mixer.layout-name": { title: "Layout name", what: "A name for saving these channels, groups and mix names as a layout." },
  "mixer.layout-save": { title: "Save layout", what: "Saves these channels, groups and mix names as a layout any device of this model can start from. " + NOTHING_SENT },
  "mixer.profile": { title: "Start from", what: "A starting layout for this model, or one you saved. Shown while no channel is set up." },
  "mixer.profile-apply": {
    title: "Apply the layout",
    what: "Replaces these channels with the chosen layout and routes it.",
    watch: "It sends routing to the device for every channel in the layout.",
  },
  "mixer.layout-remove": { title: "Delete the saved layout", what: "Deletes the chosen saved layout from the workspace. " + NOTHING_SENT },
  "mixer.add-channel": { title: "Add a channel", what: "Adds a channel on the next free mixer input. It does nothing until it has an input and a main mix." },
  "mixer.width-auto": { title: "Fit channels", what: "Fits the channels to the window's width, between the narrowest and widest a strip can be. Remembered in this browser." },
  "mixer.width-fixed": { title: "Fixed width", what: "Gives every channel the width set beside it; channels scroll sideways when they do not fit. Remembered in this browser." },
  "mixer.width-px": { title: "Channel width", what: "The width of every channel, in pixels, while Fixed is chosen." },

  // The Inputs page.
  "inputs.preamps": { title: "Preamps", what: "The device's microphone preamps, whether or not a mixer channel uses them." },
  "inputs.digital": { title: "{name}", name: "Digital inputs", what: "The device's {name}. Their gain trims the digital signal as it arrives." },
  "inputs.mic-emulation": { title: "Mic emulation", what: "On the Quadro, which of Antelope's own microphones is on a preamp and which classic microphone it is made to sound like." },
  "inputs.type-mic": {
    title: "{name} Mic",
    name: "Preamp",
    what: "Sets {name} for a microphone: gain 0 to 65 dB, and 48V available.",
  },
  "inputs.type-line": {
    title: "{name} Line",
    name: "Preamp",
    what: "Sets {name} for a line level source such as a synth or another preamp: gain -6 to +20 dB.",
    watch: "48V is off on Line.",
  },
  "inputs.type-hiz": {
    title: "{name} Hi-Z",
    name: "Preamp",
    what: "Sets {name} for an instrument plugged straight in, such as a guitar or bass: gain 0 to 40 dB. Only the first preamps have it (two on the Quadro, four on the Studio+).",
    watch: "48V is off on Hi-Z.",
  },
  "inputs.gain": {
    title: "{name} gain",
    name: "Preamp",
    what: "{name}'s gain in whole dB. Its range follows the type: Mic 0 to 65, Line -6 to +20, Hi-Z 0 to 40.",
    effect: "Raises or lowers the input before anything else, so every mix, recording and effect fed by it follows. " + STEP_KEYS,
    watch: "Too much clips the converter, and that cannot be undone later: watch the clip light on its strips.",
  },
  "inputs.48v": {
    title: "{name} 48V",
    name: "Preamp",
    what: "Phantom power for a condenser microphone on {name}. It exists on Mic only.",
    effect: "Turning it on takes a second click (or Ctrl or Cmd and a click), as the Quadro's panel guards it; turning it off is one click.",
    watch: "48V can damage a ribbon microphone or gear wired unbalanced, and switching it makes a loud thump: turn the monitors down first.",
  },
  "inputs.phase": { title: "{name} phase invert", name: "Preamp", what: "Flips the polarity of {name}'s signal.", effect: "Useful when two microphones on one source cancel each other; on its own it sounds the same." },
  "inputs.hpf": {
    title: "High-pass filter",
    what: "Lit when the device reports its high-pass filter on for this preamp.",
    watch: "Neither model's panel has a command to change it, so the app can only show it.",
  },
  "inputs.digital-gain": {
    title: "{name} gain",
    name: "Input",
    what: "The gain of {name}, -6 to +12 dB in whole dB, applied to the digital signal as it arrives.",
    effect: STEP_KEYS,
  },
  "inputs.digital-gain-readonly": {
    title: "{name} gain",
    name: "Input",
    what: "The gain the device reports for {name}.",
    watch: "The Quadro's own panel never sets this gain, so the app shows it and does not change it.",
  },
  "mic.target": {
    title: "{name} microphone",
    name: "Preamp",
    what: "Which of Antelope's microphones is on {name}, or none. An Edge Duo covers two preamps and an Edge Quadro four.",
    effect: "Picking a microphone that covers more than one preamp links them, so gain and 48V move together.",
    watch: "Every preamp the microphone covers must be on Mic. One the device's licence does not cover is greyed.",
  },
  "mic.model": {
    title: "{name} emulation",
    name: "Preamp",
    what: "Which classic microphone the one on {name} is made to sound like, or the microphone itself. The Edge Quadro has one per head.",
    watch: "One the device's licence does not cover is greyed and cannot be picked.",
  },
  "mic.pattern": {
    title: "{name} polar pattern",
    name: "Preamp",
    what: "Which directions the microphone picks up, from omni through cardioid to figure-8, as far as the emulated microphone allows. Some emulations are fixed at one pattern.",
  },
  "mic.preset": {
    title: "{name} stereo technique",
    name: "Preamp",
    what: "A stereo technique for the Edge Quadro's two heads: XY (both cardioid), Blumlein (both figure-8) or M/S (one of each). Offered only where both heads' emulations can do it.",
    watch: "The technique also wants the top head turned 90 degrees, which is yours to do by hand.",
  },
  "mic.swap": { title: "Swap membranes", what: "Swaps the microphone's front and rear membranes, so its front faces the other way." },
  "mic.plot": {
    title: "Polar pattern",
    what: "How loud the microphone hears each direction, front at the top. A lobe drawn dashed hears in inverted polarity.",
    watch: "Under a stereo technique the heads are drawn turned as the technique wants them, not as they sit: the app cannot see them.",
  },

  // The Outputs page.
  "outputs.hard-mute": {
    title: "Hard mute",
    what: "The Quadro's one switch that mutes every output at once. The vendor panel throws it while it restores a session, so nothing plays through half applied settings.",
    effect: "Every output goes silent until it is switched off again; each output's own volume and mute are kept.",
    watch: "It is a real mute of all four outputs, not the Mute beside one: nothing plays anywhere while it is lit.",
  },
  "outputs.volume": {
    title: "{name} volume",
    name: "Output",
    what: "The hardware volume of {name}, in dB of attenuation: 0 dB at the right, down to off at the left.",
    effect: "Sends the new volume straight to the device. A double-click sets -30 dB, or 0 dB when the header's Double-click menu says unity; Ctrl or Cmd and a click always sets 0 dB.",
    watch: "A drag to the right is as loud as the output goes: turn up slowly on speakers and headphones.",
  },
  "outputs.mute": { title: "{name} mute", name: "Output", what: "Mutes {name} on the device. Its volume is kept and comes back when unmuted." },
  "outputs.dim": {
    title: "{name} dim",
    name: "Output",
    what: "Dims {name}: the Quadro lowers the output by its own dim amount while this is lit.",
    watch: "How many dB the device dims by is its own; the app has not measured it.",
  },
  "outputs.mono": { title: "MONO", what: "The Quadro reports this output as mono. The app can show this but has no command to change it; the Control Room's Mono sums a mix instead." },
  "outputs.in-cr": { title: "{name} in the Control Room", name: "this output", what: "Whether {name} shows in the sidebar's Control Room. " + WORKSPACE + " " + NOTHING_SENT },
  "outputs.trims": { title: "Trims", what: "The output trims, in the seven steps the vendor panel labels 20 dBu down to 14 dBu." },
  "outputs.trim": {
    title: "{name} trim",
    name: "Output",
    what: "{name}'s trim, one of seven steps the vendor panel labels 20 dBu to 14 dBu.",
    watch: "The panel gives the steps in dBu and nothing more; how they relate to full scale on the device has not been measured here.",
  },
  "outputs.talkback": { title: "Talkback", what: "The Studio+'s talkback microphone: talk while held, its level, and which outputs hear it." },
  "outputs.talk": {
    title: "Talk",
    what: "The Studio+ talkback: on only while you hold it (pointer, Space or Enter), off as soon as you let go, however it is let go.",
  },
  "outputs.talk-level": { title: "Talkback level", what: "How loud talkback is, as a fader on the outputs' scale: 0 dB at the right, off at the left.", effect: "A double-click sets -30 dB, or 0 dB when the header's Double-click menu says unity; Ctrl or Cmd and a click always sets 0 dB." },
  "outputs.talk-to": { title: "Talkback to {name}", name: "this output", what: "Whether talkback is heard on {name} while Talk is held." },

  // The Routing page.
  "routing.read": { title: "Read from device", what: "Reads every destination's routing from the device again, to see changes made elsewhere." },
  "routing.sources": { title: "Sources", what: "Everything that can feed a destination: preamps, digital inputs, the computer's playback, mixes, effect returns. Pick one, or shift-click for a run." },
  "routing.destinations": { title: "Destinations", what: "Everything a source can go to: outputs, the computer's recording inputs, effect chains, and the mixers' inputs. Each channel takes one source; a source can feed any number." },
  "routing.source": {
    title: "{name}",
    name: "Source",
    what: "A source channel. Click to select it, shift-click to select a run from the last one picked. Then click a destination cell, or drag onto one.",
    effect: "Selecting sends nothing; the destination click does.",
  },
  "routing.cell": {
    title: "{name}",
    name: "Destination",
    what: "One destination channel and the source feeding it (off when muted).",
    effect: "Click it with sources selected to route them here: a run fills this cell and the ones after it. Delete or Backspace mutes it.",
    watch: "What was routed here is replaced. On an output you are listening through, that changes what you hear at once.",
  },
  "routing.cell-mixer": { title: "{name}", name: "Mixer input", what: "A mixer input and the source feeding it. Mixer inputs are set by the Mixer page's channels, so they are shown here and not changed." },
  "routing.mute-row": { title: "Mute row", what: "Mutes every channel of this destination on the device.", watch: "On an output you are listening through, it goes quiet at once." },
  "routing.mute-row-mixer": { title: "Mute row", what: "Mixer inputs are set by the Mixer page's channels, so this row cannot be muted here." },

  // The Effects page.
  "effects.read": { title: "Read from device", what: "Reads the chains, the reverb and its sends and returns from the device again." },
  "effects.chains": { title: "Effect chains", what: "Each chain processes what routing sends to its AFX IN channel and returns it on AFX OUT. The Quadro has six, the Studio+ sixteen, each up to eight effects in order." },
  "effects.chain-source": { title: "What feeds this chain", what: "The source routed to this chain's AFX IN channel, from the device's routing. Change it on the Routing page." },
  "effects.chain-link": { title: "Linked chain", what: "The device reports this chain linked with its partner. Links are shown here, not changed; a change to one effect goes to the same effect in the partner." },
  "effects.effect": {
    title: "{name}",
    name: "Effect",
    what: "An effect in the chain, with its instance number. Click to open its settings below the chains.",
  },
  "effects.move-up": {
    title: "Move earlier",
    what: "Moves the effect one place earlier in the chain. The whole chain is written and read back.",
    watch: "Reordering a chain has never been tried on a device.",
  },
  "effects.move-down": {
    title: "Move later",
    what: "Moves the effect one place later in the chain. The whole chain is written and read back.",
    watch: "Reordering a chain has never been tried on a device.",
  },
  "effects.remove": {
    title: "Remove the effect",
    what: "Takes the effect out of the chain. The whole chain is written and read back, and the device switches the effect off as it goes.",
  },
  "effects.on": {
    title: "Process",
    what: "The effect processes the signal. Neither On nor Bypass shows lit until the effect's settings are read or you set one.",
  },
  "effects.bypass": {
    title: "Bypass",
    what: "The signal passes the effect untouched. The effect keeps its settings.",
    effect: "A linked chain's same effect follows.",
  },
  "effects.meter": {
    title: "Effect meter",
    what: "The effect's output level in dB below full scale, with a clip light, and GR, the gain reduction the device reports.",
    watch: "GR is shown in the device's own steps: the vendor panel reads it as dB for some effects and quarter dB for others.",
  },
  "effects.no-meter": { title: "No meter", what: "The device reports no meter for this effect." },
  "effects.clip": { title: "Effect clip light", what: "Lights when the effect's output reaches full scale. Click it to clear it." },
  "effects.add": {
    title: "Add an effect",
    what: "Every effect type this model has. One with no free instance left is greyed rather than left out.",
    effect: "Choosing one puts it after what is in the chain, on the lowest free instance, then writes the chain and reads it back. The wheel does not step this menu.",
    watch: "Adding more than one effect to a chain, and the Studio+'s chains, have not been tried on a device.",
  },
  "effects.bypass-all": { title: "Bypass all", what: "Bypasses every effect in this chain." },
  "effects.process-all": { title: "Process all", what: "Sets every effect in this chain processing." },
  "effects.editor": { title: "{name}", name: "Effect", what: "The settings of the effect you chose, as the vendor panel's code describes them." },
  "effects.editor-close": { title: "Close", what: "Closes the effect's settings." },
  "effects.param-range": {
    title: "{name}",
    name: "Setting",
    what: "One of the effect's settings, as a range.",
    effect: "Each change sends every setting of this effect with the new value. A double-click puts it back to the vendor panel's starting value.",
    watch: "A value without a unit is the device's own step: the vendor panel draws its scale in artwork the app does not copy.",
  },
  "effects.param-switch": { title: "{name}", name: "Setting", what: "One of the effect's settings, as a switch.", effect: "Each change sends every setting of this effect. A double-click puts it back to the starting value." },
  "effects.param-menu": { title: "{name}", name: "Setting", what: "One of the effect's settings, chosen from a list.", effect: "Each change sends every setting of this effect. A double-click puts it back to the starting value." },
  "effects.param-bits": { title: "{name}", name: "Setting", what: "One of the effect's settings, as buttons that each switch one part of it on or off." },
  "effects.band": { title: "{name}", name: "Band", what: "One band of the equaliser. A change sends only this band.", watch: "While the band is a high-pass or low-pass filter, its gain is off." },

  // The reverb.
  "reverb.heading": { title: "Reverb", what: "The device's one reverb: on or off and its level here; its other settings as the device reports them." },
  "reverb.on": { title: "Reverb on", what: "Switches the device's reverb on or off. Sent only once its settings have been read, so the other settings are never overwritten with defaults." },
  "reverb.level": { title: "Reverb level", what: "The level of the reverb's output. On the Studio+ this is its return.", effect: "A double-click sets -18 dB, the nearest its steps have to -20, or 0 dB when the header's Double-click menu says unity; Ctrl or Cmd and a click sets 0 dB." },
  "reverb.param": { title: "{name}", name: "Reverb setting", what: "A reverb setting as the device reports it, in the vendor panel's units.", watch: "Changing it is not in this version." },
  "reverb.returns": { title: "Reverb returns", what: "How much reverb each of the Quadro's first two mixes gets back." },
  "reverb.return-level": {
    title: "{name}",
    name: "Reverb return",
    what: "How much reverb goes back into this mix, in the vendor panel's own steps: 0 is full, 90 the least.",
    effect: "A double-click sets 20 steps below full; Ctrl or Cmd and a click sets full.",
    watch: "The panel shows no scale for these, and they have not been checked on the device.",
  },
  "reverb.return-mute": { title: "Mute the return", what: "Mutes the reverb coming back into this mix." },
  "reverb.sends": { title: "Reverb sends", what: "How much of each of mix 1's channels 1 to 16 goes to the Quadro's reverb, and where it sits in it." },
  "reverb.send-level": { title: "{name}", name: "Reverb send", what: "How much of this channel goes to the reverb, in dB of attenuation: 0 dB at the right, off at the left.", effect: "A double-click turns it off, so it never adds reverb, or sets 0 dB when the header's Double-click menu says unity; Ctrl or Cmd and a click always sets 0 dB." },
  "reverb.send-pan": { title: "{name}", name: "Reverb send pan", what: "Where this channel sits in the reverb, from L 100% through C to R 100%.", effect: "Dragging snaps to centre near the middle; a double-click centres it." },

  // The Devices page.
  "devices.device": { title: "Device", what: "The device's identity as the server sees it, and the name you give it." },
  "devices.status": { title: "Status report", what: "A few values live from the report the device sends many times a second." },
  "devices.clock": { title: "Clock", what: "Where the device takes its clock from, the sample rate, and what it measures." },
  "devices.panning": { title: "Panning law", what: "How much the Quadro turns down a centred signal in every mix." },
  "devices.dc": { title: "DC coupling", what: "Whether the Quadro's converters pass DC, for control voltages." },
  "devices.oscillator": { title: "Test oscillator", what: "A sine tone per side straight to the outputs, for lining up a signal path." },
  "devices.presets": { title: "Presets", what: "The device's own five preset slots, kept in the device's memory, not the workspace." },
  "devices.name": { title: "Device name", what: "The name shown on the device's card and in menus. Left empty, it goes by the model. " + WORKSPACE + " " + NOTHING_SENT },
  "devices.model": { title: "Model", what: "The device's model, from its USB identity." },
  "devices.family": { title: "Family", what: "The command set the server speaks to this device: quadro or studio, or unknown." },
  "devices.id": { title: "Id", what: "The server's id for this device. A real device's comes from its serial number." },
  "devices.usb-id": { title: "USB id", what: "The device's USB vendor and product ids." },
  "devices.backend": { title: "Backend", what: "How the server reaches this device: usb for the real interface, loopback for the emulator." },
  "devices.identity": { title: "Identity", what: "Whether the device keeps the same id when it is plugged in again." },
  "devices.live-power": { title: "Power", what: "Whether the device reports itself on or in standby." },
  "devices.live-preset": { title: "Preset", what: "The preset slot the device reports it is on." },
  "devices.live-sync": { title: "Sync source", what: "The clock source the device reports, as its index in the device's list." },
  "devices.power-on": { title: "Power on", what: "Wakes the device from standby." },
  "devices.standby": {
    title: "Standby",
    what: "Puts the device in standby. Click twice: the first click arms it for 3 s.",
    watch: "Standby stops the device's audio: everything playing through it stops.",
  },
  "devices.brightness": { title: "Front-panel brightness", what: "How bright the device's front panel is, 0 to 100%.", effect: "It has no effect on the audio. The bar shows what the device reports." },
  "devices.preset-recall": {
    title: "Recall preset {name}",
    name: "",
    what: "Loads the device's preset from this slot. The slot the device is on is lit.",
    effect: "Click twice: the first click arms it for 3 s and the button reads Confirm.",
    watch: "A preset may hold anything, 48V and the clock among it, and recalling it changes all of it at once, which can change what you hear.",
  },
  "devices.preset-slot": { title: "Save into", what: "The slot Save writes into. Choosing one writes nothing." },
  "devices.preset-save": {
    title: "Save preset",
    what: "Saves the device's current state into the chosen slot. Click twice: the first click arms it for 3 s.",
    watch: "It overwrites what is in that slot.",
  },
  "devices.clock-source": {
    title: "Clock source",
    what: "Where the device takes its clock from: its own (Internal on the Quadro, the oven clock on the Studio+), or a digital input, word clock (Studio+) or USB.",
    effect: "Choosing another shows Confirm beside the menu; nothing is sent until it is pressed, and a wait of 3 s puts the menu back.",
    watch: "Changing it reclocks the device, which interrupts whatever is playing through it. A source with no signal behind it loses lock. The wheel does not step this menu for that reason.",
  },
  "devices.sample-rate": {
    title: "Sample rate",
    what: "The rate the device runs at, 32 to 192 kHz.",
    effect: "Choosing another shows Confirm beside the menu; nothing is sent until it is pressed, and a wait of 3 s puts the menu back.",
    watch: "Changing it interrupts everything playing through the device, including a DAW's stream. While the device follows an external clock it takes the rate from there and ignores this. The wheel does not step this menu.",
  },
  "devices.clock-confirm": {
    title: "Confirm",
    what: "Sends the clock source or sample rate chosen in the menu beside it. Its own tooltip names the choice.",
    watch: "Audio through the device stops for a moment while it reclocks.",
  },
  "devices.measured": { title: "Measured", what: "The sample rate the device measures, as it reports it; none when it measures nothing." },
  "devices.lock": { title: "Lock", what: "Whether the device reports itself locked to its clock source." },
  "devices.spdif-src": {
    title: "S/PDIF sample-rate converter",
    what: "The Studio+'s converter on its S/PDIF input: on, a source at another rate or on another clock is converted, so it need not be the clock.",
    watch: "What the panel's label says is in a bitmap the app cannot read; the meaning is from the manual and users, and still to check on the device.",
  },
  "devices.panning-law": {
    title: "Panning law",
    what: "How much a centred signal is turned down in every mix: 0, -3, -4.5 or -6 dB.",
    effect: "Every pan is heard through it, and so is Mono, which works by centring pans.",
  },
  "devices.dc-inputs": {
    title: "DC coupled inputs",
    what: "Lets the Quadro's inputs pass control voltages as well as audio, for modular gear.",
    effect: "Turning it on takes a second click within 3 s; turning it off is one.",
    watch: "Leave it off for audio: with it on, any DC offset a source carries comes in too.",
  },
  "devices.dc-outputs": {
    title: "DC coupled outputs",
    what: "Lets the Quadro's outputs pass control voltages as well as audio, for modular gear.",
    effect: "Turning it on takes a second click within 3 s; turning it off is one.",
    watch: "Leave it off for audio: with it on, any DC in what is played reaches the outputs, which speakers and headphones should never get.",
  },
  "devices.osc-frequency": { title: "{name} frequency", name: "Oscillator", what: "The tone's frequency: 1 kHz or 440 Hz." },
  "devices.osc-tone": {
    title: "{name} tone",
    name: "Oscillator",
    what: "Switches this side's test tone on or off.",
    effect: "Turning it on takes a second click within 3 s; turning it off is one.",
    watch: "At 0 dBFS the tone is as loud as the device goes: turn the monitors down first.",
  },
  "devices.osc-level": { title: "Oscillator level", what: "The tones' level, shared by both sides: 0, -6, -12 or -18 dBFS.", watch: "0 dBFS is full scale." },
  "devices.driver": {
    title: "Driver",
    what: "The USB audio driver's settings on this computer for this device, not the device's own. Read from the driver itself; the buffer size and Safe Mode can be changed here, each with a confirming click.",
    watch: "A program using the driver (a DAW) restarts its audio when either changes.",
  },
  "devices.driver-version": { title: "Driver version", what: "The driver's version and the version of its programming interface. One Gazelle was not checked against says so." },
  "devices.driver-rate": { title: "Sample rate", what: "The sample rate the driver reports it is running at for this device." },
  "devices.driver-buffer": {
    title: "Buffer size",
    what: "The driver's ASIO buffer, in samples: smaller means less delay through the computer, larger means fewer dropouts.",
    watch: "A DAW using the driver has to restart its audio when the buffer changes, so expect a gap or a prompt from it.",
  },
  "devices.driver-buffer-menu": {
    title: "Buffer size",
    what: "The ASIO buffer the driver uses, from the sizes it offers. Choosing one shows a Confirm beside the menu; nothing is sent until it is pressed.",
    effect: "Smaller means less delay through the computer, larger means fewer dropouts. The latencies below show what the driver reports after the change.",
    watch: "A DAW using the driver restarts its audio. The wheel does not change this menu.",
  },
  "devices.driver-buffer-confirm": {
    title: "Confirm",
    what: "Sends the buffer size chosen in the menu beside it to the driver, keeping Safe Mode as it is, then shows what the driver reports. A wait puts the menu back.",
    watch: "A DAW using the driver restarts its audio.",
  },
  "devices.driver-safe-mode-switch": {
    title: "Safe Mode",
    what: "Turns the driver's ASIO Safe Mode on or off, keeping the buffer size as it is. The first click reads Confirm; a second within three seconds sends.",
    effect: "Measured on a Quadro: it adds 176 samples of output latency at 256 samples and 265 at 512, and nothing to the input.",
    watch: "A DAW using the driver restarts its audio.",
  },
  "devices.driver-force": {
    title: "Change anyway",
    what: "Sends the change the driver section refused because a program is using the driver's ASIO interface, this time anyway. It takes a confirming click.",
    watch: "That program (a DAW, most likely) restarts its audio, and may stop or complain.",
  },
  "devices.driver-result": {
    title: "Result",
    what: "What happened to the last change: what the driver reports now, with its latencies, or why nothing was sent. A result that differs from what was sent says so.",
  },
  "devices.driver-latency": { title: "Latency", what: "The delay the driver reports, in samples and in milliseconds at its sample rate, rounded as the vendor's panel rounds it." },
  "devices.driver-safe-mode": {
    title: "Safe Mode",
    what: "The driver's ASIO Safe Mode, which adds buffering for fewer dropouts at the cost of more delay.",
    effect: "Measured on a Quadro at 256 samples: it adds 176 samples, about 4 ms, to the output latency and nothing to the input.",
  },
  "devices.driver-refresh": { title: "Read again", what: "Asks the driver again now, rather than using what was read in the last few seconds. It changes nothing." },

  // The Workspace page.
  "workspace.saving": { title: "Saving", what: "The workspace is being saved to the server. Edits save by themselves a moment after you make them." },
  "workspace.names": { title: "Device names", what: "The name and badge colour of every connected device." },
  "workspace.surfaces": { title: "Surfaces", what: "Rows of strips from any devices side by side, each opened on a page of its own." },
  "workspace.cables": { title: "Digital cables", what: "Which digital output is plugged into which input, so surfaces can say where a signal comes from and warn when the clocks disagree." },
  "workspace.groups": { title: "Groups", what: "The workspace's groups, with their colours." },
  "workspace.snapshots": { title: "Snapshots", what: "Named records of the workspace and every attached device's state, to compare with now." },
  "workspace.backup": { title: "Backup", what: "Export the workspace to a file, or replace it with one." },
  "workspace.device-id": { title: "Device id", what: "The server's id for the device." },
  "workspace.device-name": { title: "Device name", what: "The name shown on the device's card and in menus. Left empty, it goes by the model. " + NOTHING_SENT },
  "workspace.device-colour": { title: "Badge colour", what: "The colour of this device's badge on surface strips, so two devices' strips never read as one mixer. " + NOTHING_SENT },
  "workspace.device-colour-clear": { title: "Clear the colour", what: "Goes back to the theme's colour for this device's badge." },
  "workspace.group-toggle": { title: "Fold the group", what: "Folds or opens the group in this list and on the Mixer page." },
  "workspace.group-no-colour": { title: "No colour", what: "The group has no colour of its own, so its channels show theirs." },
  "workspace.group-colour": { title: "Group colour", what: "Gives the group this colour, which its channels show over their own." },
  "workspace.surface-name": { title: "Surface name", what: "The surface's name; edit it in place." },
  "workspace.surface-open": { title: "Open", what: "Opens the surface on a page of its own." },
  "workspace.surface-delete": { title: "Delete the surface", what: "Deletes the surface. Click twice. Nothing changes on the devices: a surface is only what to show." },
  "workspace.surface-new-name": { title: "New surface name", what: "A name for a new surface." },
  "workspace.surface-create": { title: "New surface", what: "Makes an empty surface with the name beside it." },
  "workspace.cable-from": { title: "Cable from", what: "The digital output the cable leaves, on any attached device." },
  "workspace.cable-to": { title: "Cable to", what: "The digital input of the same kind the cable goes into, on another device." },
  "workspace.cable-channels": { title: "Channels", what: "How many channels the cable carries: up to 2 for S/PDIF, 8 for ADAT." },
  "workspace.cable-declare": {
    title: "Declare cable",
    what: "Records that this cable is plugged in.",
    watch: "A cable only says what is plugged in. It routes nothing and changes no clock; set those on each device.",
  },
  "workspace.cable-health": { title: "What is wrong along it", what: "Warnings from both devices' reports: sample rates that differ, a receiver not locked, or signal leaving one end and none arriving at the other." },
  "workspace.cable-remove": { title: "Remove the cable", what: "Removes the cable from the workspace. Click twice. Nothing changes on the devices." },
  "workspace.snapshot-name": { title: "Snapshot name", what: "The snapshot's name; edit it in place." },
  "workspace.snapshot-compare": { title: "Compare with now", what: "Reads every device again and shows what differs from the snapshot. Nothing is sent." },
  "workspace.snapshot-delete": { title: "Delete the snapshot", what: "Deletes the snapshot from the server. Click twice." },
  "workspace.snapshot-new-name": { title: "New snapshot name", what: "A name for the snapshot to take." },
  "workspace.snapshot-take": { title: "Take snapshot", what: "Reads every attached device's mixer, routing, inputs, outputs, clock and settings, and keeps them with the workspace under this name.", effect: "Only reads: nothing is sent to a device. Not possible in dry run, where reads answer nothing." },
  "workspace.snapshot-prepare": {
    title: "Prepare recall",
    what: "Reads the devices again and lists, in order, every command putting this snapshot back would send, with its guards and bytes.",
    watch: "It is a preview only: putting a snapshot back is not built yet, and nothing is sent.",
  },
  "workspace.snapshot-close": { title: "Close", what: "Closes the comparison." },
  "workspace.export": { title: "Export", what: "Downloads the workspace as a dated JSON file." },
  "workspace.export-all": { title: "Export with snapshots", what: "Downloads the workspace and every snapshot in one file." },
  "workspace.import-file": { title: "Workspace file", what: "A workspace file, or a backup with snapshots, to import." },
  "workspace.import": { title: "Import", what: "Reads the chosen file, says what it holds and asks before replacing anything." },
  "workspace.import-replace": {
    title: "Replace workspace",
    what: "Replaces the whole workspace with the file's, for everyone using this server. Snapshots it holds are added.",
    watch: "Export first if you may want the current one back. A workspace is layout only, so nothing is sent to a device.",
  },
  "workspace.import-cancel": { title: "Cancel", what: "Leaves the workspace as it is." },

  // A surface.
  "surface.name": { title: "Surface", what: "A row of strips from any devices side by side. Every control on a strip sends what the device's own page would, to that device." },
  "surface.to-workspace": { title: "Workspace page", what: "Surfaces are made and deleted on the Workspace page." },
  "surface.mix": { title: "{name} mix", name: "Device", what: "Which mix the surface shows for {name}'s channels and master, unless a strip is pinned to its own. Kept with the surface." },
  "surface.clock": { title: "Clock", what: "The device's sample rate, clock source and lock, followed while the surface is open: two devices joined by a cable must agree." },
  "surface.cable-health": { title: "Cable", what: "A cable between two devices on this surface, and anything wrong along it: rates that differ, no lock, or signal that leaves and never arrives." },
  "surface.grip": { title: "Drag to move", what: "Drag the strip by this grip to move it along the surface." },
  "surface.move-left": { title: "Move left", what: "Moves the strip one place left on the surface. " + NOTHING_SENT },
  "surface.move-right": { title: "Move right", what: "Moves the strip one place right on the surface. " + NOTHING_SENT },
  "surface.pin": { title: "Mix this strip shows", what: "Follow the surface's mix for this device, or keep this strip to one mix. The wheel does not step this menu." },
  "surface.remove": { title: "Take off the surface", what: "Takes the strip off the surface. Click twice. Nothing changes on the device." },
  "surface.add-device": { title: "Strip device", what: "The device the new strip comes from." },
  "surface.add-kind": { title: "Strip kind", what: "What to add: a mixer channel, a mix master, an input, an output, a digital output, both ends of a cable, or a label." },
  "surface.add-item": { title: "Strip item", what: "Which one of that kind on that device." },
  "surface.add-text": { title: "Label text", what: "The text of a label strip." },
  "surface.add": { title: "Add strip", what: "Puts the chosen strip at the end of the surface. " + NOTHING_SENT },
  "surface.badge": { title: "{name}", name: "Device", what: "The device this strip belongs to, in its badge colour, so strips from two devices never read as one mixer." },
  "surface.caption": { title: "Which mix", what: "The mix the strip shows, or which part of a digital output; pinned when it keeps to one mix." },
  "surface.provenance": { title: "Where it comes from", what: "A declared cable brings this input's signal from another device, and what that device routes to its end." },
  "surface.input-level": { title: "Input level", what: "The input's signal as it arrives, on the meters' scale." },
  "surface.src": {
    title: "S/PDIF SRC",
    what: "The Studio+'s sample-rate converter on its S/PDIF input: on, this device need not follow the sender's clock.",
    watch: "Its meaning is from the manual and users, and still to check on the device.",
  },
  "surface.port-feed": { title: "What feeds this pair", what: "The mix or source routed to this pair of the digital output, or Muted, from the device's routing." },
  "surface.port-route": {
    title: "Route to this pair",
    what: "Routes a mix, a source bit for bit, or nothing to this pair of the digital output, on the device that owns it.",
    effect: "One routing write to the device; the rest of the port is kept. The wheel does not step this menu.",
    watch: "A source routed bit for bit has no level: neither model has a volume for its digital outputs.",
  },
  "surface.port-level": { title: "Output level", what: "The level the Quadro reports leaving its S/PDIF output." },
  "surface.port-master": { title: "Master feeding the port", what: "This mix feeds the digital output, so its master fader is that output's level." },

  // Notices.
  "notices.error": { title: "Error", what: "Something the app tried did not work. It stays until you dismiss it." },
  "notices.warning": { title: "Warning", what: "Something worth knowing, such as the server not finding the devices. It stays until you dismiss it." },
  "notices.info": { title: "Notice", what: "Something the app wants you to know. It stays until you dismiss it." },
  "notices.dismiss": { title: "Dismiss", what: "Closes this notice." },
};
