// The metronome's arithmetic the pages share: tap tempo, where the beat light is between frames, and
// the words. Its calls go through the recording model, which the fake client answers.

import { test } from "node:test";
import assert from "node:assert/strict";

import { GazelleError, type MetronomeStatus, type RecordingStatus } from "gazelle-audio-client";

import { beatAt, clampTempo, listText, metronomeState, TAP_RESET_MS, TapTempo, tempoLine, tempoText, volumeText } from "../src/store/metronome.ts";
import { RecordingModel, stateLabel } from "../src/store/recording.ts";
import { Store } from "../src/store/store.ts";
import { builtInThemes, FakeClient, MemoryStorage } from "./fake-client.ts";

const settings = { tempo: 120, numerator: 4, denominator: 4, accent: true, subdivision: "none", sound: "click", volume_db: -18, outputs: [], count_in_bars: 0, follow_record: false } as const;

function status(change: Partial<MetronomeStatus> = {}): MetronomeStatus {
  return { running: true, open: true, beat: 1, bar: 1, beats_per_bar: 4, beat_seconds: 0.5, since_beat_seconds: 0, at_ms: 10_000, outputs: [], settings: { ...settings, outputs: [] }, ...change };
}

test("a tempo is 20 to 400 in steps of 0.1, and says itself plainly", () => {
  assert.equal(clampTempo(97.34), 97.3);
  assert.equal(clampTempo(5), 20);
  assert.equal(clampTempo(1_000), 400);
  assert.equal(clampTempo(Number.NaN), 120);
  assert.equal(tempoText(120), "120");
  assert.equal(tempoText(97.5), "97.5");
  assert.equal(tempoLine({ ...settings, outputs: [], numerator: 6, denominator: 8, tempo: 97.5 }), "97.5 BPM, 6/8");
  assert.equal(volumeText(-18.4), "-18 dBFS");
  assert.equal(volumeText(3), "-6 dBFS", "never shown louder than the ceiling");
});

test("tap tempo averages the last few taps, starts again after a pause, and counts quarter notes", () => {
  const taps = new TapTempo();
  assert.equal(taps.tap(0), undefined, "one tap is no tempo yet");
  assert.equal(taps.tap(500), 120);
  assert.equal(taps.tap(1_000), 120);
  // A little late, then on time: the average holds it steady.
  assert.equal(taps.tap(1_520), clampTempo(60_000 / (1_520 / 3)));
  for (let at = 2_000; at <= 4_000; at += 500) taps.tap(at);
  assert.equal(taps.count, 5, "the last four intervals, no more");
  assert.equal(taps.tap(4_500), 120);
  assert.equal(taps.tap(4_500 + TAP_RESET_MS + 1), undefined, "a pause starts again");
  assert.equal(taps.tap(4_500 + TAP_RESET_MS + 1 + 600), 100);
  // Eighth notes tapped at 240 a minute are 120 quarter notes a minute.
  const eighths = new TapTempo();
  eighths.tap(0, 8);
  assert.equal(eighths.tap(250, 8), 120);
});

test("the beat light moves on from the last frame by the page's clock, and wraps at the bar", () => {
  assert.deepEqual(beatAt(status(), 10_000), { beat: 1, into: 0 });
  assert.deepEqual(beatAt(status(), 10_250), { beat: 1, into: 0.5 });
  assert.equal(beatAt(status(), 11_000)?.beat, 3);
  assert.equal(beatAt(status({ beat: 4, since_beat_seconds: 0.25 }), 10_250)?.beat, 1, "on into the next bar");
  assert.equal(beatAt(status({ running: false }), 10_000), undefined);
  assert.equal(beatAt(status({ at_ms: undefined } as unknown as Partial<MetronomeStatus>), 10_000), undefined);
  assert.equal(beatAt(undefined, 0), undefined);
});

test("the state says what the click is doing, and a count-in says its bar", () => {
  assert.equal(metronomeState(undefined), "Stopped");
  assert.equal(metronomeState(status({ running: false })), "Stopped");
  assert.equal(metronomeState(status({ started_by: "hand" })), "Playing");
  assert.equal(metronomeState(status({ started_by: "follow" })), "Playing with the take");
  assert.equal(metronomeState(status({ count_in: { bars: 2, bar: 0 } })), "Count-in of 2 from the next bar");
  assert.equal(metronomeState(status({ count_in: { bars: 2, bar: 1 } })), "Count-in, bar 1 of 2");
  assert.equal(listText(["USB 1 PLAY 7", "USB 1 PLAY 8"]), "USB 1 PLAY 7 and USB 1 PLAY 8");
  assert.equal(listText(["A", "B", "C"]), "A, B and C");
  const counting: RecordingStatus = { state: "counting_in", channels: [], overruns: 0, dropouts: 0, dropouts_by_device: [], disk_low: false, reset_asked: false };
  assert.equal(stateLabel({ ...counting, metronome: status({ count_in: { bars: 2, bar: 2 } }) }), "Count-in 2 of 2");
  assert.equal(stateLabel({ ...counting }), "Count-in");
});

test("start, stop and a change go to the server, and a refusal is said in its words", async () => {
  const client = new FakeClient();
  const store = new Store(client, { themeSources: builtInThemes, storage: new MemoryStorage() });
  const model = new RecordingModel(store.recorder);
  await model.refresh();
  assert.equal(await model.metronome("start"), true);
  assert.equal(model.status.peek()?.metronome?.running, true);
  assert.equal(await model.setMetronome({ tempo: 97.5 }), true);
  assert.equal(model.status.peek()?.metronome?.settings.tempo, 97.5);
  client.recordingRefusal = new GazelleError("metronome_refused", "The metronome has no outputs to play to.");
  assert.equal(await model.metronome("start"), false);
  assert.equal(model.problem.peek(), "The metronome has no outputs to play to.");
  assert.equal(await model.metronome("stop"), true);
  assert.deepEqual(client.recordingCalls.filter((call) => call.startsWith("metronome")), ["metronome start", 'metronome settings {"tempo":97.5}', "metronome start", "metronome stop"]);
});
