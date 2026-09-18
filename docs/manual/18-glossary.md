# Glossary

**48V (phantom power).** Power for condenser microphones, sent down the microphone cable. Can damage ribbon microphones and some gear; switching it makes a thump. See [48V phantom power](02-safety.md#48v-phantom-power).

**ADAT.** An optical digital connection carrying up to 8 channels at 44.1 or 48 kHz (fewer at higher rates). The Studio+ has two ADAT ports each way; the Quadro has ADAT in only.

**AFX IN, AFX OUT.** The input and output of an effect chain. Routing feeds AFX IN; AFX OUT is a source like any other.

**Backend.** What Gazelle talks to: `usb`, your real interfaces, or `loopback`, the emulator.

**Bit for bit.** A source routed straight to an output, with no mix, fader or level in between.

**Cable (digital).** A connection between two devices' digital ports that you declare in Gazelle, so it can label signals and warn about clocks. It changes nothing on the devices.

**Chain.** An effect chain: up to 8 effects in order, fed by an AFX IN.

**Channel.** On the Mixer page: a named strip for one input, with its main mix and sends.

**Clip light.** The light by a meter that shows the signal reached full scale. Click to clear.

**Clock source.** What the interface's sample clock follows: its own (Internal, or Oven on the Studio+), or a signal arriving over word clock, ADAT, S/PDIF or USB.

**Control Room.** The outputs you listen on, gathered in the sidebar with their volume, Mute, Dim and Mono, and the Studio+'s talkback.

**dBFS.** Decibels below digital full scale. 0 dBFS is the loudest a digital signal can be; meters show -60 to 0.

**DC coupling.** Letting DC through an input or output, for modular synthesisers. Harmful to speakers. Quadro only.

**Device.** One attached interface.

**Dim.** Lowers an output by a fixed amount, without changing its volume. Quadro only.

**Dock.** The Mixer band along the bottom of every page.

**Dry run.** A mode in which Gazelle sends nothing to the devices and reports the bytes it would have sent. Started with `--dry-run`.

**Emulator (loopback).** A built-in imitation of a Quadro and a Studio+, for trying Gazelle without hardware. Started with `--backend loopback`.

**Hard mute.** The Quadro's mute for every output at once.

**Hi-Z.** A high-impedance preamp input type for instruments such as guitars.

**Instance.** One copy of an effect type in use, such as Guitar Amp #2. Licences decide how many are available.

**Layout.** A saved set of mixer channels, groups and mix names that any device of the same model can start from.

**Link.** A Gazelle link makes a change on one channel follow on others, on the mixer or the inputs, across devices too.

**Main mix.** The mix a mixer channel belongs to. It can also be sent to other mixes.

**Mix.** One of the device's four internal mixers: 32 inputs, each with level, pan, mute and solo, summed to a stereo output.

**Mono.** Gazelle's mono for a mix: every channel's pan centred, and restored afterwards. The devices have no mono setting of their own.

**Preset (device).** One of the five settings slots stored in the interface itself. Not the same as a snapshot.

**Recall.** Putting a snapshot back on the devices. Planned and previewable, not built.

**Routing.** Which source feeds each destination channel.

**S/PDIF.** A two-channel digital connection.

**Snapshot.** A named record of the workspace and every device's readable state, which can be compared with now.

**SRC (S/PDIF).** Sample rate conversion on the Studio+'s S/PDIF input, so it need not follow the device's clock.

**Surface.** A row of strips from any devices, built on the Workspace page.

**Talkback.** The Studio+'s talkback microphone signal, sent to the headphones or monitors you choose while Talk is held.

**Trim.** An output's maximum level, from 14 to 20 dBu.

**Unity.** 0 dB on a fader: the signal passes at its own level.

**Workspace.** Everything Gazelle keeps that is not on a device: names, colours, channels, groups, links, layouts, surfaces, cables and Control Room choices.

**×2 badge.** Shown on mixer strips when the same audio reaches one mix twice, about 6 dB louder than expected.
