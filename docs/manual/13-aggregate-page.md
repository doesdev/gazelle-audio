# The Aggregate page

The Aggregate page (`#/aggregate`) is for running **two or more interfaces as one** audio device in a DAW. Windows lets a DAW open one audio driver at a time, so a Quadro and a Studio+ are ordinarily two separate devices and you cannot record across both in one project. **Gazelle Aggregate** is one driver a DAW opens that carries both underneath it: the Quadro's channels, then the Studio+'s, as one long list.

The page says whether this PC can run it, sets it up, and, while a DAW is playing, shows whether it is holding together.

**Everything on the page is named the way the rest of Gazelle names it.** An interface is called by its name in Gazelle, the one the sidebar shows, and each of its channels by what it carries. See [What things are called](#what-things-are-called).

> **Warning.** This is new and has not been used in anger. It moves audio for a real session, and a session it cannot start is a session you cannot record. Try it before you need it, not on the day.

## What it needs

Three things have to be true before an aggregate works at all, and none of them is something software can arrange for you.

- **Each interface on its own USB host controller.** Two Antelope interfaces on one controller cannot both stream: this was measured at the hardware, not guessed at. A rear port and a front port are often the same controller. Adding a PCIe USB card is the usual answer.
- **One digital cable between the interfaces, and every interface clocked from it.** Two interfaces free running are two clocks, and two clocks drift apart for ever. Run S/PDIF or ADAT from one to the other, then set the receiving interface's **clock source** to that input on the [Devices page](06-devices-page.md). Declare the cable on the [Workspace page](12-workspace-page.md) so Gazelle knows which input to expect the clock on.
- **The same sample rate and the same driver buffer size on every interface.** They will not run together otherwise. The page can put the buffer sizes right for you.

Decide all three before you set anything up here. The page will tell you which of them is missing, but it cannot plug in a cable.

## Ready or not ready

At the top: **Ready** or **Not ready**, and the line beside it says what the driver is doing right now. Underneath, in the **Ready to use** section, is every reason it is not, in the order that matters: what is missing first, then the machine, then the clock.

Each reason is marked **STOPS IT** or **WORTH KNOWING**. Only a reason that stops it makes the page say Not ready; a warning is something to know about.

| Reason | What it means |
|---|---|
| No interfaces chosen | Nothing is set up yet. Add them under **Interfaces** |
| Not registered | Windows does not list Gazelle Aggregate, so no DAW will offer it. See [Registering](#registering) |
| Registered, and the file has gone | It is registered, pointing at a copy of Gazelle that has moved or been replaced. Register it again |
| An interface is not one of this PC's audio drivers | Its own driver is not installed, or the interface is unplugged |
| An interface is not connected to Gazelle | The interface its card names is not plugged in, so Gazelle cannot read its clock or buffer |
| Gazelle cannot tell which interface it is | Two of the same model are connected, or its audio driver does not say what model it is. Choose it on its card. A warning |
| Its driver could not be read | Antelope's driver would not answer. See [Troubleshooting](17-troubleshooting.md) |
| Two interfaces on one USB host controller | The one that cannot be worked around in software. Move one to another controller |
| A host controller could not be found | Gazelle could not tell which controller an interface is on, so it cannot check this. A warning |
| The rates differ | Put them all on one rate |
| The buffer sizes differ | Press the fix, or **Match buffer sizes** |
| No cable declared | Nothing says the interfaces share a clock. Declare it on the Workspace page |
| Clocked from the wrong place | An interface is not clocked from the input its cable arrives on. **This is the trap**: an interface left on Internal quietly becomes USB clocked the moment a DAW opens it, and then it drifts |
| Not locked | An interface says it is not locked to its clock. The cable, or the wrong input |
| Phase not measured | An interface has a cable from the callback master and no phase setup, so every session lines it up by its driver's figures and it lands a different distance away each time. **Set up the phase** opens its card's [phase setup](#the-phase). A warning |

Where Gazelle can put a reason right, there is a button beside it that does exactly that and nothing else: put an interface at the right rate, put it on the right clock input, match the buffer sizes, or register the driver. After it has run, the whole answer is read again, so what you then see is what the server makes of it.

**Matching buffer sizes asks twice**, the way 48V does: one click arms it, a second within three seconds does it. Every program using those drivers, a DAW included, restarts its audio when it happens, so a take being recorded is lost. Stop playback first.

## Registering

A DAW will not offer Gazelle Aggregate until Windows lists it as an audio driver. That list is shared by every program on the PC, so writing to it needs administrator rights.

**This is the one thing in Gazelle that asks for administrator rights.** Nothing else in the app does, and if you are ever asked at any other time, something is wrong. Press **Register the driver** and Windows puts up its own prompt; declining it changes nothing. The exact command is shown underneath for anyone who would rather run it themselves in a prompt started as an administrator.

The section also shows **which copy** the registration points at. Windows remembers the path, not the program, so moving or replacing Gazelle leaves a registration pointing at a file that has gone: a DAW opening the aggregate then simply fails. When that happens the page says so and **Register it again** points it at the copy that is here now.

**Unregister** takes it back out of the list, and asks twice. A DAW that had it selected will need pointing at another driver.

## Interfaces

One card per interface, in the order their channels appear to a DAW. The order is the order of the list, so the first interface's channels come first.

| On the card | What it is |
|---|---|
| The name at the top | The interface's name in Gazelle. There is nothing to type here: rename the device on the [Devices page](06-devices-page.md) or the [Workspace page](12-workspace-page.md) and it is renamed here and in the DAW |
| **Gazelle device** | Which of the interfaces Gazelle is connected to this one is. Left on **Work it out**, Gazelle settles it itself whenever exactly one interface of that model is connected, and says "Worked out" beside the menu. Choosing one pins it, which is what two of the same model need |
| **Audio driver** | The interface's own audio driver, as Windows lists it, which is what the aggregate opens. It is shown once, here, and is not a name the interface goes by |
| **Channels** | Every channel the interface has in the aggregate, and what each one is called. See [What things are called](#what-things-are-called) |
| **Clock** and **LOCK** | What it says it is clocked from now, and whether it is locked |
| **Rate** | The sample rate it reports |
| **Buffer** and **Safe Mode** | Antelope's driver settings for this interface, the same ones the [Devices page](06-devices-page.md) shows. Each takes a confirming click, and changing either restarts the audio of every program using that driver |
| **Input trim** and **Output trim** | Samples to add to what its driver claims its latency is. See [Trims](#trims) |
| **Gap** | How far it is from the interface driving the callback, while a DAW is playing. See [The gap](#the-gap) |
| **Phase now** | What this session's phase measurement came to, while a DAW has the aggregate open. See [The phase](#the-phase) |
| **Phase setup** | Where the driver measures this interface's phase, on every card but the callback master's. See [The phase](#the-phase) |

**Up** and **Down** move an interface along the list; **Remove** takes it out and asks twice. Choose one from the menu at the bottom and press **Add** to put a new one at the end: the menu names each driver for the device it is where Gazelle can tell, with the driver's own name after it. **Match buffer sizes** puts every interface on the buffer size the callback master is on.

### What things are called

**An interface is called by its name in Gazelle**: the name you gave the device, or its model where you gave it none, exactly as the sidebar shows it. That is the name on its card, in every picker and sentence on the page, in the driver's own record and log, and in the DAW. There is no second name to keep in step: to rename an interface, rename the device.

**An aggregate channel is one of the interface's USB channels**, not a preamp and not an output socket. An interface sends the computer its recordings on its USB record channels and takes what the computer plays on its USB playback channels, and those are exactly the channels the aggregate carries: aggregate input 1 is what the interface's first USB record channel records, and aggregate output 1 is what its first USB playback channel plays. What reaches a USB record channel, and where a USB playback channel goes, is up to the interface's routing on the [Routing page](10-routing-page.md).

| Interface | Aggregate inputs are its | Aggregate outputs are its |
|---|---|---|
| Zen Quadro Synergy Core | 16 channels of **USB A REC** | 16 channels of **USB 1 PLAY** |
| Zen Studio+ | 24 channels of **USB REC** | 24 channels of **USB PLAY** |

So the count is known as soon as Gazelle knows which device an interface is, without a DAW. If the driver of a running DAW reports another number, the card says so.

**A channel is named for what it carries, then by its USB channel.** An input takes the name of whatever the routing sends to its USB record channel, in Gazelle's words: your name for a Mixer channel or a mix that takes that source, where you gave one, or else the source as the Routing page shows it. So with Preamp 1 routed to USB A REC 1 and a Mixer channel for Preamp 1 that you called Vocal mic, the first input is **Vocal mic, USB A REC 1**. Change the routing and the name changes with it, because it says what would be recorded. A channel nothing is routed to is just its USB channel, **USB A REC 5**.

**An output is named for where the routing sends it**, the other way round from an input: its USB playback channel is a source on the Routing page, and the name says where that source ends up.

- It reaches an output socket, directly or through a mix: that output and side, **Monitor L, USB 1 PLAY 1**. Through a mix, the first channel of a pair is the left side and the second the right, which is how a DAW's stereo pairs land in a stereo mix.
- It lands in a mix whose output goes nowhere: the mix channel, with your names for the channel and the mix where you gave them, **Click in Cue, USB 1 PLAY 3**, else **Ch 11 in Mix 3, USB 1 PLAY 3**.
- It reaches several places: the most important socket it reaches and how many more, **Monitor L +1, USB 1 PLAY 1**. Sockets rank in this order, whatever order the interface lists them in: Monitor, Line out, the headphones (HP1 before HP2), S/PDIF out, ADAT out, and Reamp last. So a channel feeding a mix that goes to both the monitors and HP1 is named for the monitors.
- It goes nowhere: **USB 1 PLAY 5, not routed**.

Until Gazelle has read the routing the name depends on, a channel is just its USB channel. Gazelle reads each routing group it needs once, when it first needs it.

### Where the DAW can play

Each card lists the interface's output sockets, in the same order, and which of its USB playback channels reach each one: **Monitor: USB 1 PLAY 1 to 2, through Mix 1**, **S/PDIF out: USB 1 PLAY 5, directly**, or both, one after the other. A pair of sockets is one line; the Studio+'s eight line outs are a line per pair, **Line out 1 to 2**. It is the quickest way to see that something the DAW plays will actually be heard somewhere, and where.

An output that nothing from the DAW reaches says so: **Line out: nothing from the DAW reaches it**, which is what you see when the line outs are fed by a mix no USB playback channel goes into. Until Gazelle has read the routing behind an output, it says **not read yet** instead of guessing.

Beside such an output, **Send USB 1 PLAY 3 to 4 here** routes the first free USB playback channels, ones that go nowhere and are in no mix, straight to it, so the DAW can play there. It asks twice, and its tip says what the output stops playing, **Line out stops playing Mix 3**. It is one routing change to that output alone, the same as making it on the [Routing page](10-routing-page.md), and the names on the page follow it at once. If every USB playback channel already goes somewhere, there is no button, and the line says so.

**Channels** on a card opens the list, inputs and then outputs. Each row is one channel and says its name once: whether the aggregate exposes it, its name, a field for a name of your own (empty until you type), and what a DAW adds to that name, where it adds anything.

**A name you type wins**, on this page and in the DAW, over the one from the routing, and the two are kept apart: clear your name and the one from the routing comes back. A name is at most 31 characters, which is all the audio driver carries.

**What the DAW sees.** Gazelle writes each channel's name into the driver's setup, and keeps it in step with the routing whenever the routing changes through Gazelle, whether or not this page is open; the driver takes the new names at its next reset. The DAW shows the short part, what the channel carries, with the driver's own reference after it, the interface's name and the channel's number: **Monitor L (Quadro 3)**. All of that has to fit in 31 characters, so in the DAW a device you have not named goes by its model's short form, **Quadro** or **Studio+**, rather than its full name; the page still shows the full name. A name you give the device in Gazelle replaces the short form, and if it is too long to leave room, the DAW shows the channel's name alone. The row's DAW line gives only what the DAW adds, **... (Quadro 3)**, and nothing where it adds nothing.

Turning a channel off keeps it out of the aggregate altogether, so a DAW never lists it. While every channel is exposed, nothing is recorded in the setup, which is what "all of them" means, and turning everything back on takes it out again.

Two interfaces of one model that you have not named come out with the same name, so the second is told apart by its place, **Zen Quadro Synergy Core (2)**, and in the DAW by a count, **Quadro 2**. Naming the devices is the better answer.

## Setup

| Choice | What it does |
|---|---|
| **Callback master** | Which interface drives the DAW's callback. Everything else is lined up against its clock, so it should be the one the others take their clock from over the cable. It is kept by the interface's audio driver, which renaming the device does not change, so a rename never loses it |
| **Alignment** | **Aligned** pads every interface so they all line up, at the cost of a little latency; **lowest latency** pads nothing, so interfaces of different latencies end up offset from each other. Aligned unless you are counting samples |
| **Sample rate** | The rate to put every interface at. Left alone, the aggregate takes whatever they are already on |
| **Buffer size** | The buffer size the aggregate offers a DAW as its preferred one |

The setup is kept in the workspace, so it travels with a [workspace backup](15-snapshots-and-backup.md), and Gazelle writes it out for the driver at the path shown under the section. The driver takes a change at once when nothing is streaming, and at the next buffer change when a DAW is running, which drops audio for a moment exactly as a buffer size change does.

### Trims

An interface's driver reports how many samples of latency it has, and that figure is not always right. A trim is your correction to it, in samples: an interface that records late takes a positive input trim. It moves recordings against each other and does nothing else.

You can type a trim in, but the difference is usually tens of samples, which is well under a millisecond and smaller than you can judge from two waveforms. **Line the interfaces up** measures it instead.

## The phase

**Two interfaces do not start in the same place, and where they start changes every time.** Within one session they record a fixed distance apart; the next time a DAW opens the aggregate that distance has moved, by a whole number of 32 sample steps. A trim is a constant, so on its own it can only ever be right for the session it was measured in.

So the driver measures each interface's **phase** at the start of every session, down the digital cable that already carries the clock between the interfaces, and puts the session back where it was when the trim was measured. Then the trim applies, as it always has. Nothing new has to be plugged in.

### Three numbers

They are different things, and all three apply. Mixing them up is how somebody corrects the same thing twice.

| | What it is | Where you see it |
|---|---|---|
| **Trim** | A constant, in samples, measured once with a cable and a click | **Input trim** on the card |
| **Reference** | The phase that was measured in the session the trim was measured in. Written together with the trim, never on its own, and never typed in | **Phase setup** on the card, and beside each trim a measurement offers |
| **Phase** | Where this session happened to start. Measured again every session | **Phase now** on the card, and the rows under **While a DAW has it open** |

Every session is lined up by **the reference minus its phase**, and then the trim applies.

### Setting it up

Open **Phase setup** on each follower's card (not the callback master's: the others are measured against it) and choose two channels:

- **Leaves the callback master on**: one of the callback master's own USB playback channels, the one that goes out on the digital cable.
- **Arrives on**: one of this interface's own USB record channels, the one the cable comes in on.

Both are named as channels are named everywhere on the page. Nothing is saved until both are chosen, because half a path is no use to the driver. The driver keeps those two channels for itself, so a DAW no longer lists them. **Clear** takes the setup out again, reference and all, and asks twice.

Once it is set up, the card says **Set up, no reference yet**. One measurement under **Line the interfaces up** gives it its reference, written together with its input trim, and every session after that is lined up. Choosing a different pair of channels takes the old reference out, because it was measured on the old path: measure once more afterwards.

### The routing it needs

This is the step that is easy to miss. The measurement travels on the digital cable, so **each interface's own routing has to carry it**:

- On the **callback master**, the playback channel chosen under **Leaves the callback master on** has to be routed to the socket the cable leaves from, its S/PDIF out.
- On the **follower**, the socket the cable arrives at, its S/PDIF in, has to be routed to the record channel chosen under **Arrives on**.

A fresh setup has neither, and a path that is not routed reads as **nothing heard**, not as a missing route. Set both on the [Routing page](10-routing-page.md) before measuring.

### What each session made of it

While a DAW has the aggregate open, **Phase now** on each follower's card, and the row under **While a DAW has it open**, say what this session's measurement came to, with what was measured and what was applied, in samples:

- **Lined up to its reference** is what you want.
- **Measured, not lined up: no reference yet** means the setup is there and one measurement is all it needs.
- **Refused** means the driver would not use what it heard: nothing came back on the cable, the change from the reference was not a whole number of 32 sample steps, or it was further than the driver can move it. The session then runs on the figures the drivers report, exactly as it would with no phase setup, and the log says why.

## Lining the interfaces up

This plays a click out of one interface and records it on every interface at once, then reads how far apart the copies landed. It is measured through the aggregate itself, so everything the aggregate already does to line the interfaces up has happened before anything is measured, and what is left over is exactly what a trim cancels.

**Measure or Check.** **Measure** finds the trims, and the phase reference written with each, with nothing lined up, so what it hears is the raw difference. **Check** plays the same clicks through the same cables with the session lined up exactly as a DAW's is, and says how far apart a recording would land now; it writes nothing. Measure once, then check whenever you want to know the trims still hold.

**Before you press it.** It makes a noise: a click at a modest level, out of a real output, into whatever is plugged in. Turn amplifiers down the first time. It also takes both audio drivers for itself while it runs, so close your DAW first. Measure and Check each take a confirming click, the way matching buffer sizes does.

**The cabling.** The page writes out exactly what to patch for the channels you have picked, and it is worth reading rather than guessing, because the whole measurement rests on it. For the input pass, one interface plays and every interface records: one output of that interface into its own input, and the next output of **the same** interface into the other interface's input. Both copies leave on the same sample, so any difference in where they land is the difference between the interfaces and nothing else. For the output pass it is the other way round: one output on each interface, all of them into inputs of one interface.

Each picker lists one interface's own USB channels, named as they are everywhere on the page, and the cabling says which interface each is on. The measurement is asked for by interface and channel. Where that channel sits among the aggregate's channels is worked out by the measurement itself, once it has the audio drivers open, so a cable always lands on the interface its picker named. If a channel cannot be used it says so before anything plays, and says what would work: how many channels the interface really has, which ones the setup exposes, or that the channel is the one its phase setup uses.

**What comes back.** Per interface: how far behind the reference it landed, in samples; the spread, which is how much the clicks disagreed with each other, and under a sample means a measurement to trust; and how many of the clicks were found at all. Then the trims it implies, showing what the setup says now, what was measured, and what it would become, and beside an input trim the phase reference that goes with it. **Write these trims into the setup** writes each trim and its reference together. If nothing was heard on the cable, writing the trim takes the old reference out rather than leaving it beside a trim it was not measured with. An interface whose trim came out the same but whose reference is new, which is what a first measurement usually looks like, is still written.

Under **The phase**, each interface's phase at the start of the run: in a measurement it is measured and not applied, on purpose, because that is the reference. A phase the driver refused is said before the trims. Extra channels the run listened in on, if any, are listed under **Listened in on**; they change no trim.

**What a check says.** A verdict per interface rather than an offer: **Lined up** when a recording would land within a sample of the reference, **Out** when it would not, which means measure again and then check. At the hardware a good check reads a few hundredths of a sample.

**A drift finding is the serious one.** If the lag grows steadily through the run, the interfaces are not holding a single clock, and no trim fixes that. Check the digital cable and each interface's clock source, and remember that an interface left on Internal quietly becomes USB clocked the moment a DAW opens it.

**The click has to be able to get there.** It leaves on one of the interfaces' USB playback channels and comes back on one of their USB record channels, so the interface's own routing has to carry it: from that playback channel to the socket the cable leaves, and from the socket it arrives at to the record channel. That is the same routing the [Routing page](10-routing-page.md) shows, and on a fresh interface it is often not set up, which reads as "nothing arrived" rather than as a bad cable. The phase measurement needs the same of its own path (see [The routing it needs](#the-routing-it-needs)). Check the path before blaming the lead.

**Was the audio clean?** A run says so, with how many blocks were lost and which interface lost them. Blocks that go missing during a measurement move the very thing being measured, so a run that lost any is reported as not clean. Run it again rather than believing it. The same goes for clicks that disagree with each other: the page shows the spread, and a spread near the buffer size means the eight clicks were not measuring one thing.

If nothing arrives on an input, that is a cable, not a measurement, and the page says which one.

## While a DAW has it open

The driver publishes what it is doing only while a DAW has it open, so this section saying nothing is the ordinary state, not a fault. While a DAW is playing, the page reads it every second.

It shows the **plan in force**: the master, the rate, the buffer size, how many channels the DAW actually asked for, the alignment, and the latency each way. If the driver is still running an older setup than the one Gazelle last asked for, it says so, and it catches up at the next buffer change. **Last refused** is the last thing the driver would not do, in the same words the DAW was given, and it is usually the line that explains a session that would not start.

Then a row per interface, with its phase underneath (see [The phase](#the-phase)).

### The gap

**The gap is the number that matters.** It is this interface's sample count minus the master's, and it says whether the clock link is holding.

- **In step** (zero) is what you want, and **Master** is the interface everything else is measured against.
- **Off by some samples, one way or the other**, is shown in the warning colour. A gap that keeps growing in one direction is two clocks rather than one: the cable, or an interface clocked from the wrong input. Check the clock source of every interface on the [Devices page](06-devices-page.md) **while the DAW is playing**, because that is the only time the reading means anything.
- **Stalled** means the driver has given up on that interface for the moment: its inputs are reading as silence and its outputs are muted until it comes back. It is said before anything else about that interface, whatever its counters say.

Beside the gap: **blocks** handled, **dropped** (thrown away because that interface was running ahead) and **starved** (not there when they were wanted, which is what you hear as a click). Dropped and starved climbing is the same story as a growing gap.

## What happened

The driver's own log, kept on disk, written only when something actually happens: **Session started** and **Session ended** (which says what the session lost), **Phase measured** (what that session's phase came to, or why it was not lined up), **Lost a block** (the first block a session lost), an interface stalling and recovering, a setup adopted, anything refused. Lines written during one of Gazelle's own measurements or checks are marked **GAZELLE**, so they are not mistaken for something that happened to a recording. It is there so a session that would not start last night can still be explained today. It is a plain text file, so you can open it yourself; the [command line chapter](18-command-line-and-api.md) says where Gazelle keeps its files.

## When something is wrong

| What you see | What to do |
|---|---|
| The DAW does not list Gazelle Aggregate | It is not registered. Register it from this page |
| The DAW lists it and fails to open it | The registration points at a copy that has gone: **Register it again** |
| The session will not start | Read **Last refused** and the log. Usually one interface will not run at the rate asked for |
| The gap grows and grows | Two clocks. Check the cable and every interface's clock source while the DAW is playing |
| Clicks, and starved climbing | The buffer is too small for the machine, or one interface is stalling. Raise the buffer on both |
| One interface goes quiet mid session | It stalled. The log says when, and whether it recovered |
| Nothing at all in the live section | No DAW has it open. That is the normal state |
| Phase: nothing heard on the cable | The routing does not carry the measurement. See [The routing it needs](#the-routing-it-needs) |
| Takes line up differently from one session to the next | The follower has no phase setup, or no reference yet. Set it up and measure once |

This page tells you about the aggregate; [Troubleshooting](17-troubleshooting.md) covers Gazelle not finding the interfaces in the first place.
