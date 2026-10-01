// The recorder as both pages show it, below their DOM: the store's calls, the live state from the
// socket, one model per store, and the words and numbers the transport shows.

import { test } from "node:test";
import assert from "node:assert/strict";

import { GazelleError, type RecordingAutoArm, type RecordingStatus } from "gazelle-audio-client";

import { alignmentText, autoArmShort, autoArmText, clockText, diskLeftText, diskText, fileOf, folderOf, lossText, meterFill, newPreset, prerollFill, prerollText, presetProblem, presetToOffer, recordingModel, RecordingModel, secondsText, stateLabel, takeLine, timeOfDay, warnings } from "../src/store/recording.ts";
import { Store } from "../src/store/store.ts";
import { builtInThemes, FakeClient, MemoryStorage } from "./fake-client.ts";

const settle = () => new Promise((resolve) => setImmediate(resolve));

const armed: RecordingStatus = {
  state: "armed",
  preset: { id: "band", name: "Band", folder: "C:\\Takes", format: "24-bit" },
  rate: 96000,
  buffer_size: 512,
  channels: [{ name: "Vocal mic (Quadro 1)", device: "Quadro", device_index: 0, channel: 0, peak_dbfs: -18 }],
  preroll: { available_bytes: 16e9, percent_asked: 10, percent: 10, bytes: 1.6e9, capacity_frames: 1, preroll_frames: 1, preroll_seconds: 60, held_seconds: 15 },
  overruns: 0,
  dropouts: 0,
  dropouts_by_device: [],
  disk_low: false,
  reset_asked: false,
};

function withStore() {
  const client = new FakeClient();
  const store = new Store(client, { themeSources: builtInThemes, storage: new MemoryStorage() });
  return { client, store };
}

test("the model follows the socket, reads the state and takes when a page opens, and stops when none is left", async () => {
  const { client, store } = withStore();
  const model = recordingModel(store);
  assert.equal(recordingModel(store), model, "one model per store, which both pages share");
  const stopA = model.activate();
  const stopB = model.activate();
  await settle();
  assert.deepEqual(client.recordingCalls, ["status"], "read once, however many pages follow");
  assert.equal(model.status.value?.state, "off");
  client.emit("recording", armed);
  assert.equal(model.status.value?.state, "armed", "the socket's frames are the state");
  stopA();
  client.emit("recording", { ...armed, state: "recording" });
  assert.equal(model.status.value?.state, "recording", "still followed while one page shows it");
  stopB();
  stopB();
  client.emit("recording", armed);
  assert.equal(model.status.value?.state, "recording", "and not once none does");
});

test("arm, record, stop and disarm are one call each, and a refusal is said in the server's words", async () => {
  const { client, store } = withStore();
  const model = new RecordingModel(store.recorder);
  assert.equal(await model.arm("band"), true);
  assert.equal(model.status.value?.state, "armed");
  assert.equal(await model.toggle(), true, "Space while armed records");
  assert.equal(model.status.value?.state, "recording");
  assert.equal(await model.toggle(), true, "and while recording stops");
  assert.equal(model.status.value?.state, "armed");
  client.recordingRefusal = new GazelleError("measuring", "A measurement on the Aggregate page is using the interfaces.");
  assert.equal(await model.record(), false);
  assert.equal(model.problem.value, "A measurement on the Aggregate page is using the interfaces.");
  assert.equal(await model.disarm(true), true);
  assert.deepEqual(client.recordingCalls, ["arm band", "record", "stop", "record", "disarm confirmed"]);
  assert.equal(model.toggle(), undefined, "Space does nothing while off: it never arms");
});

test("the transport's words and numbers", () => {
  assert.equal(clockText(7.44), "0:07.4");
  assert.equal(clockText(723), "12:03.0");
  assert.equal(clockText(3723.2), "1:02:03.2");
  assert.equal(clockText(undefined), "0:00.0");
  assert.equal(secondsText(4.84), "4.8 s");
  assert.equal(secondsText(135), "2 min 15 s");
  assert.equal(stateLabel(armed), "Armed");
  assert.equal(stateLabel(undefined), "Off");
  assert.equal(stateLabel({ ...armed, state: "recording" }), "Recording");
  assert.equal(prerollText(armed), "15.0 of 60.0 s of pre-roll held (10.0 % of free memory)");
  assert.equal(prerollFill(armed), 0.25);
  assert.equal(prerollText({ ...armed, state: "recording" }), "Started with 15 s of pre-roll");
  assert.equal(meterFill(-60), 0);
  assert.equal(meterFill(-30), 0.5);
  assert.equal(meterFill(3), 1);
  assert.equal(meterFill(undefined), 0);
  assert.equal(lossText(armed), undefined, "nothing lost, nothing said");
  const lossy = { ...armed, overruns: 2, dropouts: 3, dropouts_by_device: [{ device: "Studio+", dropped: 1, starved: 2 }] };
  assert.equal(lossText(lossy), "Since Arm: 2 blocks lost because the disk fell behind (each is silence in its take, and its log says where); 3 blocks lost by the aggregate (Studio+ 3).");
  assert.equal(diskText({ ...armed, disk_free_bytes: 50 * 1024 ** 3, disk_seconds_left: 7200 }), "50.0 GB free, about 2 h 0 min of recording");
  assert.equal(folderOf("C:\\Takes\\2026-09-27 T001 Kick.wav"), "C:\\Takes");
  assert.equal(fileOf("C:\\Takes\\2026-09-27 T001 Kick.wav"), "2026-09-27 T001 Kick.wav");
  const take = { number: 3, preset: "Band", folder: "C:\\Takes", files: ["a", "b"], log: "l", date: "2026-09-27", time: "14:03:20", seconds: 72, preroll_seconds: 8, overruns: 0, dropouts: 0 };
  assert.equal(takeLine(take), "T003, 2026-09-27 14:03:20, 1 min 12 s (8.0 s pre-roll), 2 files");
});

test("presets: a new one records nothing yet, and the one offered first is the armed one, then the last, then this browser's", () => {
  const made = newPreset([{ id: "preset-1", name: "Preset 1" }]);
  assert.deepEqual(made, { id: "preset-2", name: "Preset 2", channels: [] });
  assert.equal(presetProblem(made), "Preset 2 records no channels yet. Tick at least one.");
  assert.equal(presetProblem({ ...made, channels: [{ device: 0, channel: 0 }] }), undefined);
  assert.equal(presetProblem({ ...made, channels: [{ device: 0, channel: 0 }], pattern: "{take}" }), "The file name pattern needs {channel}.");
  const presets = [{ id: "a", name: "A" }, { id: "b", name: "B" }, { id: "c", name: "C" }];
  assert.equal(presetToOffer(presets, armed, "c"), "c", "an armed preset the list has not got is passed over");
  assert.equal(presetToOffer(presets, { ...armed, preset: { id: "b", name: "B", folder: "", format: "" } }, "c"), "b");
  const { preset: _armedWith, ...off } = armed;
  assert.equal(presetToOffer(presets, { ...off, state: "off", last_preset: "c" }, "a"), "c");
  assert.equal(presetToOffer(presets, undefined, "b"), "b");
  assert.equal(presetToOffer([], undefined, "b"), undefined);
});

const auto = (over: Partial<RecordingAutoArm> = {}): RecordingAutoArm => ({ on: true, preset: "band", preset_name: "Band", phase: "armed", reason: null, failures: 0, retry_at_ms: null, lost: false, ...over });

test("auto-arm is said in a sentence for the page and the hub, and in a few words for the widget", () => {
  const now = 1_800_000_000_000;
  assert.equal(autoArmText(undefined, now), undefined, "an older server says nothing about it");
  assert.equal(autoArmText(auto({ on: false }), now), undefined, "nor does one with it off");
  assert.equal(autoArmShort(auto({ on: false }), now), undefined);
  assert.equal(autoArmText(auto(), now), "Auto-arm is on, with Band.");
  assert.equal(autoArmShort(auto(), now), "Auto: Band");
  assert.match(autoArmText(auto({ phase: "paused" }), now) ?? "", /paused because you disarmed\. Arm to resume/);
  assert.equal(autoArmShort(auto({ phase: "paused" }), now), "Auto: paused");
  assert.match(autoArmText(auto({ phase: "waiting_for_interfaces", lost: true }), now) ?? "", /went away, so auto-arm disarmed\. It arms with Band again/);
  assert.match(autoArmText(auto({ phase: "waiting_for_measurement" }), now) ?? "", /measurement/);
  const backing = auto({ phase: "backing_off", reason: "A DAW has the interfaces.", failures: 2, retry_at_ms: now + 14_200 });
  assert.equal(autoArmText(backing, now), "Auto-arm could not arm with Band: A DAW has the interfaces. It tries again in 15 s.");
  assert.equal(autoArmShort(backing, now), "Auto: retry in 15 s");
  assert.equal(autoArmShort({ ...backing, retry_at_ms: now - 5 }, now), "Auto: retry in 0 s", "never a countdown below zero");
  assert.equal(autoArmShort(auto({ preset_name: null }), now), "Auto: band", "the id, when the preset has gone from the workspace");
});

test("the warnings the transport, the widget and the hub share, the clock and the disk", () => {
  assert.deepEqual(warnings(undefined, undefined), []);
  const trouble: RecordingStatus = { ...armed, overruns: 2, disk_low: true, disk_free_bytes: 2 * 1024 ** 3, disk_seconds_left: 500, reset_asked: true, problem: "The disk was slow." };
  const said = warnings("Refused.", trouble);
  assert.equal(said[0], "Refused.");
  assert.equal(said[1], "The disk was slow.");
  assert.match(said[2] ?? "", /2 blocks lost because the disk fell behind/);
  assert.equal(said[3], "The disk is getting full: 2.0 GB free, about 8 min 20 s of recording.");
  assert.match(said[4] ?? "", /driver asked to be restarted/);
  assert.equal(timeOfDay(new Date(2026, 8, 27, 9, 5, 7)), "09:05:07");
  assert.equal(diskLeftText(armed), undefined, "not known until the writer has looked");
  assert.equal(diskLeftText({ ...armed, disk_seconds_left: 3 * 3600 + 12 * 60 }), "3 h 12 min");
});

test("the settings and the windows are read and changed through the model, and a refusal is its problem", async () => {
  const { client, store } = withStore();
  const model = new RecordingModel(store.recorder);
  await model.readComputer();
  assert.deepEqual(model.settings.value, { auto_arm: false, auto_arm_preset: null, start_in_hub: false, cubase_seed: null, cubase_seed_check: null });
  assert.equal(model.windows.value?.available, true);
  assert.equal(await model.setSettings({ auto_arm: true, auto_arm_preset: "band" }), true);
  assert.deepEqual(model.settings.value, { auto_arm: true, auto_arm_preset: "band", start_in_hub: false, cubase_seed: null, cubase_seed_check: null });
  assert.equal(await model.setWindow("widget", { open: true }), true);
  assert.equal(model.windows.value?.widget, true);
  assert.equal(await model.setWindow("hub", { open: true }), true);
  assert.equal(model.windows.value?.hub_full_screen, true);

  client.recordingRefusal = new GazelleError("no_preset", "There is no preset \"gone\".");
  assert.equal(await model.setSettings({ auto_arm_preset: "gone" }), false);
  assert.equal(model.problem.value, "There is no preset \"gone\".");
  assert.equal(model.settings.peek()?.auto_arm_preset, "band", "nothing changed");

  client.recordingWindows = { available: false, widget: false, hub: false, hub_full_screen: false, reason: "no windows here" };
  assert.equal(await model.setWindow("widget", { open: true }), false);
  assert.equal(model.problem.value, "no windows here");
});


test("the alignment check is one line while armed, and a warning when the interfaces slip or the signal goes missing", () => {
  assert.equal(alignmentText(armed), undefined, "a server that does not check says nothing");
  assert.equal(alignmentText({ ...armed, state: "off", alignment: { state: "checking", checks: 3, offset: 0 } }), undefined, "nor does an off recorder");
  const checking = { state: "checking" as const, device: "Studio+", checks: 12, since_check_seconds: 0.4, offset: 0 };
  assert.deepEqual(alignmentText({ ...armed, alignment: checking }), { text: "Alignment checked 0.4 s ago: held.", warn: false });
  assert.deepEqual(alignmentText({ ...armed, alignment: { state: "waiting", device: "Studio+", checks: 0 } }), { text: "Alignment: waiting for the first check over the phase cable.", warn: false });
  const off = alignmentText({ ...armed, alignment: { state: "off", checks: 0, reason: "no phase path is set up" } });
  assert.deepEqual(off, { text: "Alignment is not being checked: no phase path is set up.", warn: false });

  const slipped = alignmentText({ ...armed, state: "recording", alignment: { ...checking, offset: 32, slip: { samples: 32, take_seconds: 83.5 } } });
  assert.deepEqual(slipped, { text: "Alignment slipped by 32 samples at 1:23.5 into this take: Studio+ is late. Nothing is corrected; the take's log says when.", warn: true });
  const early = alignmentText({ ...armed, alignment: { ...checking, offset: -1, slip: { samples: -1 } } });
  assert.equal(early?.text, "Alignment slipped by 1 sample while armed: Studio+ is early. Nothing is corrected; the take's log says when.");
  const back = alignmentText({ ...armed, alignment: { ...checking, slip: { samples: 32, take_seconds: 5 } } });
  assert.deepEqual(back, { text: "Alignment held again, checked 0.4 s ago, after slipping by 32 samples at 0:05.0 into this take.", warn: true });
  const silent = alignmentText({ ...armed, alignment: { ...checking, silent_seconds: 4.2 } });
  assert.equal(silent?.warn, true);
  assert.match(silent?.text ?? "", /no check signal has arrived on the phase cable for 4\.2 s/);
});
