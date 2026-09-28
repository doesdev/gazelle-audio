# The Remote page

The Remote page (`#/remote`) is a remote for the session: what you reach for away from the desk, laid out for a phone held upright. A paired phone opens on it (see [Phones on your network](05-the-app.md#phones-on-your-network)), and on a phone it is the first tab. It works in a desktop window as well, as one column in the middle of the page, though a wide window on the computer shows no tab for it: open it at `#/remote`. Every other page is still a tab away.

Its controls are the ones the rest of Gazelle already has, at a size a thumb can use. What you change here shows on the other pages at once, and the other way round.

## The device

At the top, the device the page controls: the one last selected, as on every page. With both interfaces attached, a button for each switches between them.

## Recording

The [Recording page](14-recording-page.md)'s transport, at thumb size: the **Preset**, the state in large letters (Off, Armed with a pulsing ring, or Recording in red with the time since Record), how much pre-roll is held, and **Arm**, **Record**, **Stop** and **Disarm**. It is the same recorder as the Recording page's, so a press on the phone shows on the computer at once. Record and Stop are one tap each; disarming while a take is being recorded takes a second tap. Presets are chosen here and changed on the computer.

## Monitoring

The sidebar's [Control Room](05-the-app.md#the-sidebar), larger: the outputs chosen for it on the [Outputs page](08-outputs-page.md) (Monitor, HP1 and HP2 until you choose), each with its volume, **Mute**, **Dim** on the Quadro, and **Mono**. On the Studio+, talkback too: **Talk** talks only while it is held, and lets go when your finger does.

On the Quadro, **Hard mute** sits below them, apart, as the control to hit when something is suddenly loud. One tap mutes every output. Letting them all play again takes a second tap within a few seconds, so a stray tap cannot bring everything back at once. It is the same switch as the Outputs page's.

## Mix

What the mixer dock would show; the dock itself is hidden on this page. **Mix** chooses which of the device's mixes, and it is the same choice as the Mixer page's Mix buttons and the dock's menu: change one and the others follow. **Show**, there once you have a surface, puts one of your [surfaces](16-surfaces-and-cables.md) here instead, as the dock's Show menu does.

A device's mix is a list of faders lying on their side, one per channel in the mix and the mix's master last: 0 dB at the right, down to -90 dB at the left, on the same scale as the Mixer page's faders. Lying down, each fader gets the phone's whole width and the whole of its name, and the page scrolls up and down the way your thumb already moves. The bar along a fader's foot is the channel's input meter. Each row has **M** (mute), **S** (solo) and **-20**, which puts the fader back to -20 dB: the first tap asks, and a second tap within a few seconds resets.

## Inputs

Each preamp, as the [Inputs page](07-inputs-page.md) has it: its gain, its type (Mic, Line or Hi-Z), **48V** and phase (**Ø**). The type is shown here and changed on the Inputs page. **48V** takes two taps to turn on, as it takes two clicks at the desk; turning it off is one tap.

Each section folds with its heading, and this browser remembers which you folded.

## Under a finger

A finger lands on a fader while you scroll, so under a finger nothing jumps:

- **A tap changes nothing.** A fader, a volume or a gain moves only when you drag along it, and then from where it was, not to where your finger is.
- **Louder, slowly.** However fast the finger goes, a level or a gain gets louder no faster than a steady drag, so a flick adds a dB or two, never the whole travel. Quieter is never held back.
- **Scrolling still works.** A swipe up or down that starts on a fader scrolls the page and leaves the fader alone.
- **No double-tap reset.** Double-clicking resets a level at the desk, but a double tap is too easy to make by accident; use the **-20** button.

With a mouse the page behaves as the rest of Gazelle: a click on a bar jumps there, and a double-click resets.

> **Warning.** These rules keep a stray touch from jumping a level; they do not make a deliberate one safe. A slow drag to the right still takes Monitor to 0 dB. See [Sudden level jumps](02-safety.md#sudden-level-jumps).

## The Android app

**Gazelle Remote** is a small Android app that shows this page full screen, with nothing of a browser around it, and pairs by scanning the QR code. It shows the page your computer's Gazelle serves, so it is always the same version as your Gazelle. It needs Android 8.0 or later.

**Getting it.** On the phone, open the [releases page](https://github.com/doesdev/gazelle-audio/releases) and download `Gazelle-Remote.apk` from the latest release, the same release as `Gazelle-Setup.exe`. It does not come from the Play Store, so the first time Android asks whether the browser (or the Files app, if you open it from there) may install apps: choose **Settings**, allow it for that app, go back and choose **Install**. You can take that permission away again afterwards. To update, download a newer release's `Gazelle-Remote.apk` and install it over the old one; the phone stays paired.

**Pairing.** On the computer, turn on **Allow phones on this network** and choose **Pair a phone** (see [Phones on your network](05-the-app.md#phones-on-your-network)). In the app, choose **Scan the code on your computer** and point the phone at the QR code. Scanning uses Google Play services' own scanner, so the app never asks for the camera. On a phone without Play services, or if you would rather, type the address the Phones section shows, such as `192.168.1.20:8420`, and the code. The app then opens Gazelle's pairing page: give the phone a name and tap **Pair this phone**. From then on the app opens straight onto this page.

**Connecting.** Each time it opens Gazelle, the app first says **Connecting to** the computer's address, with **Cancel**. The page appears as soon as it has loaded, usually at once.

**When it cannot connect**, it gives up after a few seconds rather than wait on a blank page, and says **Gazelle is not answering at** the computer's address, with a line on why: no answer in time (a firewall silently dropping the connection looks like this, as does a computer that is asleep), nothing listening on that port, or no way to reach that address from the phone's network. If Gazelle answered that it is not letting phones in, the app says to turn on **Allow phones on this network**. Check that:

- the phone is on the same Wi-Fi network as the computer, not on mobile data or a guest network;
- Gazelle is running, with **Allow phones on this network** turned on;
- Windows Firewall lets Gazelle in. On a network Windows counts as **Public**, Windows blocks incoming connections until the firewall's question is answered for public networks, or the network is marked **Private** (Settings, Network and internet, the network's properties). Which to do is your choice; Gazelle never changes firewall settings.

Then choose **Retry**. If the computer's address has changed (a router can hand it a new one after a restart), choose **Pair with another computer** and pair again; a reserved address for the computer in your router's settings stops that happening.

**Pairing again, or with another computer.** Press and hold the app's icon and choose **Pair with another computer**. The pairing screen says which computer the phone is paired with now, and **Back to Gazelle** returns to it. Pairing anew forgets the old computer. A phone you revoked on the computer shows that it is no longer paired, and its **Pair again** link opens the app's pairing screen.

**What it reaches.** Only the computer it paired with: a link anywhere else, such as this manual online, opens in the phone's browser. The screen stays on while the app is in front. Its connection is the same plain HTTP as a phone's browser, so what [Phones on your network](05-the-app.md#phones-on-your-network) says about trusted networks holds for it too.
