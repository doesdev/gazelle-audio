// The recorder as both pages show it, below their DOM: the store's calls, the live state from the
// socket, one model per store, and the words and numbers the transport shows.

import { test } from "node:test";
import assert from "node:assert/strict";

import { GazelleError, type RecordingStatus } from "gazelle-audio-client";

import { clockText, diskText, fileOf, folderOf, lossText, meterFill, newPreset, prerollFill, prerollText, presetProblem, presetToOffer, recordingModel, RecordingModel, secondsText, stateLabel, takeLine } from "../src/store/recording.ts";
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
