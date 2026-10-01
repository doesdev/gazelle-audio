// The alignment suite as the Aggregate page shows it: the grid's ticks, the request, the one list of
// changes to confirm, how each setup's progress reads, the trims a card lists, and the model that
// follows the suite. No server is started here and nothing reaches a device.

import { test } from "node:test";
import assert from "node:assert/strict";

import { GazelleError, type AggregateAnswer, type AggregateCalibrateRequest, type AggregateSuite, type AggregateSuiteRequest } from "gazelle-audio-client";

import { ManualTimers } from "../../../packages/client/test/fakes.ts";
import {
  NO_PICKS,
  pickedSetups,
  setupTrimViews,
  suiteBuffers,
  suiteChanges,
  SuiteModel,
  suiteProblem,
  suiteRequest,
  suiteRunning,
  suiteSetupView,
  suiteSummary,
  SUITE_BUFFERS,
  SUITE_POLL_MS,
  SUITE_RUNS_DEFAULT,
  toggled,
} from "../src/store/aggregate-suite.ts";
import { reasonHint, type SuiteCalls } from "../src/store/aggregate.ts";

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

const single: AggregateCalibrateRequest = {
  direction: "inputs",
  outputs: [{ device: 0, channel: 0 }, { device: 0, channel: 1 }],
  inputs: [{ device: 0, channel: 0 }, { device: 1, channel: 0 }],
  clicks: 8,
  level_dbfs: -20,
};

test("the ticks are setups, listed by rate and then buffer size, and untick as they tick", () => {
  let picks = toggled(NO_PICKS, { rate: 96000, buffer_size: 512 }, true);
  picks = toggled(picks, { rate: 48000, buffer_size: 256 }, true);
  picks = toggled(picks, { rate: 96000, buffer_size: 128 }, true);
  assert.deepEqual(pickedSetups(picks), [
    { rate: 48000, buffer_size: 256 },
    { rate: 96000, buffer_size: 128 },
    { rate: 96000, buffer_size: 512 },
  ]);
  assert.deepEqual(pickedSetups(toggled(picks, { rate: 96000, buffer_size: 128 }, false)).length, 2);
  assert.equal(NO_PICKS.runs, SUITE_RUNS_DEFAULT);
  assert.equal(SUITE_RUNS_DEFAULT, 3);
  assert.deepEqual(pickedSetups({ setups: ["nonsense", "96000/0"], runs: 3 }), [], "a key that is not a setup is no setup");
});

test("the buffer sizes offered are the ones every driver takes", () => {
  assert.deepEqual(suiteBuffers([]), SUITE_BUFFERS, "with no driver read, the usual list");
  assert.deepEqual(suiteBuffers([undefined, [64, 128, 256, 512, 1024]]), [64, 128, 256, 512, 1024]);
  assert.deepEqual(suiteBuffers([[32, 64, 128, 256, 512], [64, 128, 256, 512, 1024]]), [64, 128, 256, 512]);
});

test("the suite is only offered with Inputs cabling that a single run would take, and something ticked", () => {
  const ticked = toggled(NO_PICKS, { rate: 96000, buffer_size: 512 }, true);
  assert.match(String(suiteProblem(ticked, single, "outputs")), /Set Pass to Inputs/);
  assert.match(String(suiteProblem(ticked, undefined, "inputs")), /Set up the cabling under Line the interfaces up/);
  assert.equal(suiteProblem(NO_PICKS, single, "inputs"), "Tick at least one rate and buffer size.");
  assert.equal(suiteProblem(ticked, single, "inputs"), undefined);
  const request: AggregateSuiteRequest = suiteRequest({ ...ticked, runs: 5 }, single);
  assert.deepEqual(request, { setups: [{ rate: 96000, buffer_size: 512 }], runs: 5, outputs: single.outputs, inputs: single.inputs, clicks: 8, level_dbfs: -20, confirmed: true });
});

const answer = (parts: Partial<AggregateAnswer> = {}): AggregateAnswer =>
  ({
    read_at_ms: 0,
    configured: true,
    config: { rate: 96000, buffer_size: 512, devices: [{ key: "Q" }, { key: "S" }] },
    export_path: "",
    drivers: [],
    registration: {} as AggregateAnswer["registration"],
    devices: [{ index: 0, name: "Quadro", is_master: true, driver: { buffer_size: 512 } } as AggregateAnswer["devices"][number]],
    ready: true,
    reasons: [],
    status: { state: "not_running", message: "" } as unknown as AggregateAnswer["status"],
    events: [],
    ...parts,
  }) as AggregateAnswer;

test("every change the suite makes is listed for one confirm, the putting back included", () => {
  const lines = suiteChanges([{ rate: 44100, buffer_size: 128 }, { rate: 96000, buffer_size: 256 }], 3, answer());
  assert.equal(lines.length, 3);
  assert.equal(lines[0], "Put the aggregate at 44.1 kHz, and every interface's driver on 128 samples, which restarts the audio of every program using those drivers; then play 3 runs of clicks.");
  assert.match(String(lines[1]), /^Put the aggregate at 96 kHz, and every interface's driver on 256 samples/);
  assert.equal(lines[2], "At the end, or when stopped, put the aggregate's rate back to 96 kHz and its buffer size to 512 samples, and every interface's driver back on 512 samples.");
  // A setup that left the rate to the interfaces is put back the same way, with the interfaces at their rate.
  const open = suiteChanges([{ rate: 44100, buffer_size: 128 }], 2, answer({ config: { devices: [] }, rate_in_force: { hz: 48000, from: "interfaces" } }));
  assert.match(String(open[1]), /rate back to whatever the interfaces are on, with the interfaces back at 48 kHz and its buffer size to whatever the drivers are on/);
});

test("each setup's progress reads as waiting, measuring which run, saved with what, failed with why, or stopped", () => {
  const at = { rate: 96000, buffer_size: 512 };
  assert.deepEqual(suiteSetupView({ ...at, state: "waiting" }, 3), { setup: "96 kHz at 512 samples", text: "Waiting", tone: "waiting" });
  assert.equal(suiteSetupView({ ...at, state: "switching" }, 3).text, "Switching to this setup");
  assert.equal(suiteSetupView({ ...at, state: "measuring", run: 2, round: 1 }, 3).text, "Measuring 2 of 3");
  assert.equal(suiteSetupView({ ...at, state: "measuring", run: 1, round: 2 }, 3).text, "Measuring 1 of 3, again (2 of 3)");
  const saved = suiteSetupView({ ...at, state: "saved", trims: [{ index: 1, device: "Studio+", input_trim: 28, reference: -148, spread_samples: 0.3 }] }, 3);
  assert.deepEqual(saved, { setup: "96 kHz at 512 samples", text: "Saved: Studio+ 28 (phase reference -148), runs 0.3 apart", tone: "good" });
  const failed = suiteSetupView({ ...at, state: "failed", why: "The runs did not agree within 1 sample: Studio+ came to 20.0 and 28.0 (8.0 samples apart)." }, 3);
  assert.equal(failed.tone, "bad");
  assert.match(failed.text, /^Failed: The runs did not agree within 1 sample: Studio\+ came to 20\.0 and 28\.0/);
  assert.equal(suiteSetupView({ ...at, state: "stopped" }, 3).text, "Stopped: nothing kept for this setup");
});

test("what the suite came to is said once it is over, and not while it runs", () => {
  const setups: AggregateSuite["setups"] = [
    { rate: 96000, buffer_size: 512, state: "saved" },
    { rate: 48000, buffer_size: 256, state: "failed", why: "x" },
  ];
  assert.equal(suiteSummary({ state: "running", setups, saved: 1 }), undefined);
  assert.equal(suiteRunning({ state: "stopping", setups, saved: 1 }), true);
  assert.deepEqual(suiteSummary({ state: "done", setups, saved: 1, restored: "Put back to 96 kHz and 512 samples, as it was before the suite." }), {
    text: "Finished: 1 of 2 setups saved, 1 failed. Put back to 96 kHz and 512 samples, as it was before the suite.",
    problem: true,
  });
  assert.deepEqual(suiteSummary({ state: "stopped", setups, saved: 1, restored: "Put back." }), { text: "Stopped. 1 of 2 setups saved before it was. Put back.", problem: true });
  assert.equal(suiteSummary({ state: "idle", setups: [], saved: 0 }), undefined);
});

test("a card lists its trims per setup, the one in force marked", () => {
  const device = { key: "S", trims: [{ rate: 96000, buffer_size: 512, input_trim: 28, reference: -148 }, { rate: 48000, buffer_size: 256, input_trim: 14 }] };
  assert.deepEqual(setupTrimViews(device, { rate: 96000, buffer_size: 512 }), [
    { text: "48 kHz at 256 samples: 14", inForce: false },
    { text: "96 kHz at 512 samples: 28, phase reference -148 (in force)", inForce: true },
  ]);
  assert.deepEqual(setupTrimViews({ key: "S" }, undefined), []);
});

test("the readiness reasons about setups point at the suite", () => {
  for (const code of ["trim_setup_unknown", "no_trim_for_setup"] as const) {
    assert.match(String(reasonHint({ code, severity: "warning", message: "", device: "Studio+" })), /Measure every setup/);
  }
});

/** A server's suite made of data. */
function server(): { calls: SuiteCalls; asked: string[]; state: AggregateSuite } {
  const it = {
    asked: [] as string[],
    state: { state: "idle", setups: [], saved: 0 } as AggregateSuite,
    calls: undefined as unknown as SuiteCalls,
  };
  it.calls = {
    state: async () => {
      it.asked.push("state");
      return it.state;
    },
    start: async (request) => {
      it.asked.push(`start:${request.setups.length}:${request.runs}:${request.confirmed}`);
      it.state = { state: "running", runs: request.runs, saved: 0, setups: request.setups.map((setup) => ({ ...setup, state: "waiting" })) };
      return { started: true };
    },
    stop: async () => {
      it.asked.push("stop");
      it.state = { ...it.state, state: "stopping" };
      return { stopped: true };
    },
  };
  return it;
}

test("the model follows a running suite, reads the setup again when trims are saved and at the end, and stops asking after", async () => {
  const timers = new ManualTimers();
  const it = server();
  let reread = 0;
  const model = new SuiteModel(it.calls, timers, () => (reread += 1));
  const stop = model.activate();
  await settle();
  assert.equal(model.state.value?.state, "idle");
  assert.deepEqual(timers.pending(), [], "nothing is asked again while nothing runs");

  await model.start(suiteRequest(toggled(NO_PICKS, { rate: 96000, buffer_size: 512 }, true), single));
  await settle();
  assert.equal(model.state.value?.state, "running");
  assert.deepEqual(timers.pending(), [SUITE_POLL_MS]);
  assert.equal(reread, 0);

  it.state = { ...it.state, saved: 1, setups: [{ rate: 96000, buffer_size: 512, state: "saved" }] };
  timers.advance(SUITE_POLL_MS);
  await settle();
  assert.equal(reread, 1, "a setup's trims were saved");

  it.state = { ...it.state, state: "done", restored: "Put back." };
  timers.advance(SUITE_POLL_MS);
  await settle();
  assert.equal(reread, 2, "and the rate and buffer size were put back");
  assert.deepEqual(timers.pending(), [], "it is over, so nothing is asked again");
  stop();
});

test("a page that comes back to a suite part way finds it, and stopping asks the server to stop", async () => {
  const timers = new ManualTimers();
  const it = server();
  it.state = { state: "running", runs: 3, saved: 1, setups: [{ rate: 96000, buffer_size: 512, state: "saved" }, { rate: 48000, buffer_size: 256, state: "measuring", run: 2, round: 1 }] };
  let reread = 0;
  const model = new SuiteModel(it.calls, timers, () => (reread += 1));
  const stop = model.activate();
  await settle();
  assert.equal(model.state.value?.setups[1]?.run, 2, "reattached where it is");
  assert.equal(reread, 1, "trims saved while the page was away are read once");
  await model.stop();
  await settle();
  assert.ok(it.asked.includes("stop"));
  assert.equal(model.state.value?.state, "stopping");
  stop();
  assert.deepEqual(timers.pending(), [], "a page that goes stops asking");
});

test("a server without the suite says so rather than failing, and a refused start is said", async () => {
  const timers = new ManualTimers();
  const old = new SuiteModel(
    {
      state: async () => {
        throw new GazelleError("http_404", "GET /api/v1/aggregate/suite returned HTTP 404");
      },
      start: async () => ({ started: false }),
      stop: async () => ({ stopped: false }),
    },
    timers,
    () => {},
  );
  old.activate();
  await settle();
  assert.equal(old.offered.value, false);
  assert.equal(old.problem.value, undefined);

  const refusing = new SuiteModel(
    {
      state: async () => ({ state: "idle", setups: [], saved: 0 }),
      start: async () => {
        throw new GazelleError("not_started", "The alignment suite is measuring. Wait for it, or stop it first.");
      },
      stop: async () => ({ stopped: false }),
    },
    timers,
    () => {},
  );
  await refusing.start(suiteRequest(toggled(NO_PICKS, { rate: 96000, buffer_size: 512 }, true), single));
  assert.match(String(refusing.problem.value), /^The suite did not start: The alignment suite is measuring/);
  assert.equal(new SuiteModel(undefined, timers, () => {}).offered.value, undefined, "a context with no suite is said once it is asked");
});
