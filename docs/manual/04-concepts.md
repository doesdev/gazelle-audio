# Concepts

Both interfaces are built the same way: sources, a routing matrix, four internal mixes, effect chains and outputs. Gazelle names things the way the devices do, so the words here are the words on screen. The [glossary](19-glossary.md) has short definitions.

## Devices

A **device** is one attached interface. Gazelle manages every attached interface at once. Each has:

- a **model**: Zen Quadro Synergy Core ("Quadro") or Zen Studio+ ("Studio+"). The two share most of their commands, but differ in size and in a few features;
- an **id**, from its USB serial number, so that names, layouts and colours you give it survive unplugging it and plugging it into another port;
- a **name** you choose, shown everywhere instead of the model (the Devices page or the Workspace page).

A device of a model Gazelle does not know appears with its USB id and nothing else to control.

## Signal flow

```
 sources ──► routing ──► destinations
 (preamps,                (outputs, recording channels,
  playback,                effect chain inputs,
  digital inputs,          mixer inputs)
  effect returns,
  mix outputs)
```

Everything that makes a sound is a **source**; everywhere a sound can go is a **destination**; **routing** says which source feeds each destination channel. The four **mixes** are both: their inputs are destinations, and each mix's stereo output is a source that can be routed on to an output.

| | Zen Quadro | Zen Studio+ |
|---|---|---|
| Preamps | 4 | 12 |
| Line inputs | none | 8 |
| ADAT inputs and outputs | 8 in, none out | 16 in, 16 out |
| S/PDIF | 2 in, 2 out | 2 in, 2 out |
| Computer playback channels (USB) | 16 + 2 | 24 (and 32 over Thunderbolt, not used by Gazelle) |
| Computer recording channels (USB) | 16 + 2 | 24 |
| Analogue outputs | Monitor, HP1, HP2, Line out | Monitor, HP1, HP2, Line out 1 to 8, Reamp |
| Mixes, each with 32 inputs | 4 | 4 |
| Effect chains, 8 effects each | 6 | 16 |

## Inputs

A **preamp** has a type (**Mic**, **Line** or **Hi-Z** for instruments, on the first two Quadro or first four Studio+ preamps), a gain whose range follows the type (Mic 0 to 75 dB on the Quadro and 0 to 65 dB on the Studio+, Line -6 to +20 dB, Hi-Z 0 to 45 dB on the Quadro and 0 to 40 dB on the Studio+), **48V** phantom power (Mic only), and a phase invert (**Ø**). The Quadro's preamps also offer **microphone emulation**, which turns a supported Antelope microphone into a model of another; you need the microphone and the licence.

**Digital inputs** (ADAT, S/PDIF, and the Studio+'s line inputs) have a gain of -6 to +12 dB. On the Studio+ you can set it; on the Quadro it is shown but not settable, because the vendor's own Quadro panel never sets it and nobody has checked that the device accepts it.

Two channels can be **linked** so that a change to one follows on the other; see [Links](09-mixer-page.md#links).

## The mixer and its mixes

Each device has **four mixes**, numbered 1 to 4 and nameable ("Monitors", "Cue"). A mix has 32 inputs and a master. Every input has a level, pan, mute and solo in each mix, so one signal can be loud in the monitors and quiet in the headphones.

Gazelle presents the mixer as **channels**: a named, coloured strip that knows its input, its **main mix**, and any other mixes it is **sent** to. A channel has its own level, pan, mute and solo in *every* mix it is in; the Mixer page shows one mix at a time, and a strip's fader is that channel's level in the mix on show, main mix or send alike. Choosing a channel's input routes that source into the mixer for you; you never have to find a free mixer input yourself. On the Quadro the first six mixer inputs carry the effect returns (AFX OUT 1 to 6) and are not free for channels.

A mix's output is a source like any other, so a mix plays wherever it is routed: the Monitor output, a headphone output, a recording channel for a talkback or a loopback recording, or a digital output.

**Mono** for a mix is not a device setting: neither model has one. Gazelle sums a mix to mono by centring every channel's pan in it, and remembers the pans to put back when mono is turned off. Centring both sides sums them into each output, which is louder, so Gazelle also lowers that mix's master by as much as that gains and gives the step back when mono ends. How much depends on the device: a centred channel is already lowered by the Quadro's centre attenuation, so what summing adds is 6 dB less that amount, while the Studio+ lowers nothing at centre and gains the full 6 dB. The master fader still works while mono is on, and ending mono never leaves the mix louder than it was before.

## Routing

The **Routing** page is the matrix itself: each destination, in groups of up to 32 channels, shows the source feeding each channel, or a dash for muted. A source routed straight to an output is **bit for bit**: no fader, no level, no mute other than the output's own.

Mixer inputs are routed from the Mixer page's channels, not from the Routing page.

## Outputs and the Control Room

Each analogue **output** has a volume in dB of attenuation (0 dB is the loudest, and the bottom of the scale is silence, shown as "-inf"), a mute, and on the Quadro a **Dim**. Monitor and Line out have a **trim**: their maximum level, from 14 to 20 dBu. The Quadro also has a **Hard mute**, which silences every output at once.

The **Control Room** is Gazelle's name for the outputs you listen on: by default Monitor, HP1 and HP2, and any others you choose on the Outputs page. They sit in the sidebar with their volume, Mute, Dim and Mono, next to the Studio+'s **talkback**.

Digital outputs (S/PDIF and ADAT) have no level of their own on either model. Their level is whatever feeds them: a mix's master fader, or nothing at all for a source routed bit for bit.

## Effects and reverb

Effects on these interfaces are not attached to an input. Each device has **effect chains** (6 on the Quadro, 16 on the Studio+), each an ordered list of up to 8 effects. A chain processes what routing sends to its **AFX IN** and returns the result on its **AFX OUT**, a source like any other. An empty chain passes its input straight through, which is how the same signal can reach a mix twice (see [Doubled signals](02-safety.md#doubled-signals)).

Each effect in a chain is an **instance** of an effect type ("Guitar Amp #1"). Your devices' licences decide which types and how many instances are available. Chains are linked in pairs for stereo.

Each device has one **reverb**, separate from the chains, with its own level and settings. On the Quadro, mix 1's channels have reverb sends and mixes 1 and 2 have reverb returns; on the Studio+, each channel's reverb send is on the Mixer page, in mix 1.

## Surfaces and digital cables

A **surface** is a row of strips you build yourself from any attached devices: a channel from one, a preamp and an output from the other, a mix master, a digital output. Each strip carries a badge in its device's colour, so nothing reads as shared. Surfaces never route anything on their own: every control on one does exactly what the same control does on its device's own page.

A **digital cable** is a fact you tell Gazelle: "this device's ADAT out is plugged into that device's ADAT in". Gazelle cannot see cables. Declaring one changes nothing on the devices; it lets Gazelle label where a signal comes from and warn when the two devices' clocks disagree.

## Workspace

The **workspace** is everything Gazelle keeps that is not on a device: device names and colours, mixer channels and their layout, groups, links, saved layouts, surfaces, cables and Control Room choices. It is saved on the computer running Gazelle, in `%APPDATA%\gazelle\workspace.json`, and shared by every browser connected to that Gazelle.

Some preferences are per browser instead, such as the theme, the sidebar's side, and which mix each device shows. They are listed in [Preferences](05-the-app.md#where-preferences-are-kept).

## Snapshots

A **snapshot** is a named, dated record of the workspace and of everything readable on every attached device: mixes, routing, inputs, outputs, clock and device settings. You can take one, rename it, delete it, and **compare it with now**, which reads the devices again and lists what differs.

Putting a snapshot back (**recall**) is not built yet, on purpose: it would change many things on a device at once, and it waits for careful tests on real hardware. Gazelle can already show the plan a recall would follow, as a preview that sends nothing.

A snapshot is not a **device preset**. The devices have five preset slots of their own, stored in the device and recalled from the Devices page; Gazelle cannot read what is in them.
