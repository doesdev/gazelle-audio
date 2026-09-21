# The Aggregate page

The Aggregate page (`#/aggregate`) is for running **two or more interfaces as one** audio device in a DAW. Windows lets a DAW open one audio driver at a time, so a Quadro and a Studio+ are ordinarily two separate devices and you cannot record across both in one project. **Gazelle Aggregate** is one driver a DAW opens that carries both underneath it: the Quadro's channels, then the Studio+'s, as one long list.

The page says whether this PC can run it, sets it up, and, while a DAW is playing, shows whether it is holding together.

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
| **Name** | What to call it. A DAW names its channels after this, so keep it short and recognisable |
| **Gazelle device** | Which of the interfaces Gazelle is connected to this one is. Left on **Work it out**, Gazelle settles it itself whenever exactly one interface of that model is connected, and says "Worked out" beside the menu. Choosing one pins it, which is what two of the same model need |
| **Channels** | Every input and output the interface has, and what each one is called. See [Naming the channels](#naming-the-channels) |
| **Clock** and **LOCK** | What it says it is clocked from now, and whether it is locked |
| **Rate** | The sample rate it reports |
| **Buffer** and **Safe Mode** | Antelope's driver settings for this interface, the same ones the [Devices page](06-devices-page.md) shows. Each takes a confirming click, and changing either restarts the audio of every program using that driver |
| **Input trim** and **Output trim** | Samples to add to what its driver claims its latency is. See [Trims](#trims) |
| **Gap** | How far it is from the interface driving the callback, while a DAW is playing. See [The gap](#the-gap) |

**Up** and **Down** move an interface along the list; **Remove** takes it out and asks twice. Choose one from the menu at the bottom and press **Add** to put a new one at the end. **Match buffer sizes** puts every interface on the buffer size the callback master is on.

### Naming the channels

**Channels** on a card opens the interface's whole channel list, inputs and then outputs. Each row is one channel: whether the aggregate exposes it, what it is called automatically, a field for your own name, and, where Gazelle knows it, the name Gazelle itself uses for that channel.

A channel with no name of its own reaches a DAW as the interface's name and its number, "Quadro 1". Name it and the DAW shows your name with the automatic one in brackets after it, "Vocal mic (Quadro 1)", so the track says what it is and still says where it came from. A name is at most 31 characters, which is all the audio driver carries; clearing the field puts the channel back to its automatic name.

Turning a channel off keeps it out of the aggregate altogether, so a DAW never lists it. While every channel is exposed, nothing is recorded in the setup, which is what "all of them" means, and turning everything back on takes it out again.

Gazelle's own names are a suggestion, not the truth: they are the names on the [Inputs](07-inputs-page.md) and [Outputs](08-outputs-page.md) pages, in Gazelle's order, and an audio driver may put its channels in another order. Check one against what you actually hear before trusting the rest.

The number of channels comes from the driver itself while a DAW has the aggregate open. Before that, Gazelle uses what it knows about the matched interface, and where it knows neither, it says so rather than guessing.

## Setup

| Choice | What it does |
|---|---|
| **Callback master** | Which interface drives the DAW's callback. Everything else is lined up against its clock, so it should be the one the others take their clock from over the cable |
| **Alignment** | **Aligned** pads every interface so they all line up, at the cost of a little latency; **lowest latency** pads nothing, so interfaces of different latencies end up offset from each other. Aligned unless you are counting samples |
| **Sample rate** | The rate to put every interface at. Left alone, the aggregate takes whatever they are already on |
| **Buffer size** | The buffer size the aggregate offers a DAW as its preferred one |

The setup is kept in the workspace, so it travels with a [workspace backup](15-snapshots-and-backup.md), and Gazelle writes it out for the driver at the path shown under the section. The driver takes a change at once when nothing is streaming, and at the next buffer change when a DAW is running, which drops audio for a moment exactly as a buffer size change does.

### Trims

An interface's driver reports how many samples of latency it has, and that figure is not always right. A trim is your correction to it, in samples: an interface that records late takes a positive input trim. It moves recordings against each other and does nothing else.

You can type a trim in, but the difference is usually tens of samples, which is well under a millisecond and smaller than you can judge from two waveforms. **Line the interfaces up** measures it instead.

## Lining the interfaces up

This plays a click out of one interface and records it on every interface at once, then reads how far apart the copies landed. It is measured through the aggregate itself, so everything the aggregate already does to line the interfaces up has happened before anything is measured, and what is left over is exactly what a trim cancels.

**Before you press it.** It makes a noise: a click at a modest level, out of a real output, into whatever is plugged in. Turn amplifiers down the first time. It also takes both audio drivers for itself while it runs, so close your DAW first. Measure takes a confirming click, the way matching buffer sizes does.

**The cabling.** The page writes out exactly what to patch for the channels you have picked, and it is worth reading rather than guessing, because the whole measurement rests on it. For the input pass, one interface plays and every interface records: one output of that interface into its own input, and the next output of **the same** interface into the other interface's input. Both copies leave on the same sample, so any difference in where they land is the difference between the interfaces and nothing else. For the output pass it is the other way round: one output on each interface, all of them into inputs of one interface.

**What comes back.** Per interface: how far behind the reference it landed, in samples; the spread, which is how much the clicks disagreed with each other, and under a sample means a measurement to trust; and how many of the clicks were found at all. Then the trims it implies, showing what the setup says now, what was measured, and what it would become. **Use these trims** writes them into the setup.

**A drift finding is the serious one.** If the lag grows steadily through the run, the interfaces are not holding a single clock, and no trim fixes that. Check the digital cable and each interface's clock source, and remember that an interface left on Internal quietly becomes USB clocked the moment a DAW opens it.

If nothing arrives on an input, that is a cable, not a measurement, and the page says which one.

## While a DAW has it open

The driver publishes what it is doing only while a DAW has it open, so this section saying nothing is the ordinary state, not a fault. While a DAW is playing, the page reads it every second.

It shows the **plan in force**: the master, the rate, the buffer size, how many channels the DAW actually asked for, the alignment, and the latency each way. If the driver is still running an older setup than the one Gazelle last asked for, it says so, and it catches up at the next buffer change. **Last refused** is the last thing the driver would not do, in the same words the DAW was given, and it is usually the line that explains a session that would not start.

Then a row per interface.

### The gap

**The gap is the number that matters.** It is this interface's sample count minus the master's, and it says whether the clock link is holding.

- **In step** (zero) is what you want, and **Master** is the interface everything else is measured against.
- **Off by some samples, one way or the other**, is shown in the warning colour. A gap that keeps growing in one direction is two clocks rather than one: the cable, or an interface clocked from the wrong input. Check the clock source of every interface on the [Devices page](06-devices-page.md) **while the DAW is playing**, because that is the only time the reading means anything.
- **Stalled** means the driver has given up on that interface for the moment: its inputs are reading as silence and its outputs are muted until it comes back. It is said before anything else about that interface, whatever its counters say.

Beside the gap: **blocks** handled, **dropped** (thrown away because that interface was running ahead) and **starved** (not there when they were wanted, which is what you hear as a click). Dropped and starved climbing is the same story as a growing gap.

## What happened

The driver's own log, kept on disk, written only when something actually happens: sessions starting and ending, an interface stalling and recovering, a setup adopted, anything refused. It is there so a session that would not start last night can still be explained today. It is a plain text file, so you can open it yourself; the [command line chapter](18-command-line-and-api.md) says where Gazelle keeps its files.

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

This page tells you about the aggregate; [Troubleshooting](17-troubleshooting.md) covers Gazelle not finding the interfaces in the first place.
