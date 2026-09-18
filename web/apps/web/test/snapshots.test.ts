import { test } from "node:test";
import assert from "node:assert/strict";

import { ManualTimers } from "../../../packages/client/test/fakes.ts";
import { Store } from "../src/store/store.ts";
import { describeChange, describeDiff, describeSection, describeSnapshot, formatValue, formatWhen } from "../src/store/snapshots.ts";
import { builtInThemes, device, FakeClient, MemoryStorage } from "./fake-client.ts";
import type { Snapshot, SnapshotDiff } from "gazelle-audio-client";

function setup() {
  const client = new FakeClient(device("loopback-0", "quadro", "Zen Quadro"), device("loopback-1", "studio", "Zen Studio+"));
  const store = new Store(client, { timers: new ManualTimers(), storage: new MemoryStorage(), requestFrame: (callback) => callback(), themeSources: builtInThemes });
  return { client, store };
}

const snapshot = (id: string, name: string, created: string): Snapshot => ({
  version: 1,
  id,
  name,
  created,
  note: "",
  workspace: { version: 1, groups: [], links: [], aliases: {}, mixers: {} },
  devices: {
    "loopback-0": {
      family: "quadro",
      model: "Zen Quadro",
      read_at: created,
      current_preset: 1,
      sections: { clock: { sync_source: 0 } },
      unreadable: [{ path: "inputs.emulations", reason: "the device refused 'get_mic_emulations'" }],
    },
  },
});

test("the list is fetched once, and taking, renaming and deleting keep it in step", async () => {
  const { client, store } = setup();
  const snapshots = store.snapshots;
  const names = () => snapshots.list.value.map((s) => s.name);
  assert.deepEqual(names(), []);
  assert.equal(snapshots.known.value, false, "an empty list before it is fetched is not 'no snapshots'");

  await snapshots.loadOnce();
  assert.equal(snapshots.known.value, true);
  await snapshots.loadOnce();

  const taken = await snapshots.take("  Drum tracking  ");
  assert.equal(taken?.name, "Drum tracking", "a name is trimmed before it is sent");
  assert.deepEqual(names(), ["Drum tracking"]);
  await snapshots.take("After the take");
  assert.deepEqual(names(), ["After the take", "Drum tracking"], "newest first");

  const id = snapshots.list.peek()[1]!.id;
  assert.equal(await snapshots.rename(id, "Take 1"), true);
  assert.deepEqual(names(), ["After the take", "Take 1"]);
  assert.equal(client.storedSnapshots.find((s) => s.id === id)?.name, "Take 1");

  assert.equal(await snapshots.remove(id), true);
  assert.deepEqual(names(), ["After the take"]);
  assert.deepEqual(client.invocations, [], "taking, renaming and deleting send nothing to a device");
});

test("a server that refuses says why, and the list is left as it was", async () => {
  const { client, store } = setup();
  await store.snapshots.load();
  client.failSnapshots = new Error("a snapshot cannot be taken in dry run");

  assert.equal(await store.snapshots.take("Dry"), undefined);
  assert.equal(store.snapshots.problem.value, "a snapshot cannot be taken in dry run");
  assert.deepEqual(store.snapshots.list.value.length, 0);
  assert.equal(store.snapshots.busy.value, undefined, "a failure does not leave the section busy");

  client.failSnapshots = undefined;
  await store.snapshots.take("Wet");
  assert.equal(store.snapshots.problem.value, undefined, "the next thing asked for clears the last reason");
});

test("comparing shows the diff and deleting the snapshot it is about closes it", async () => {
  const { client, store } = setup();
  client.storedSnapshots = [snapshot("snap-1", "Take 1", "2026-09-17T20:15:00Z")];
  await store.snapshots.load();

  const diff = await store.snapshots.compare("snap-1");
  assert.equal(diff?.same, true);
  assert.equal(store.snapshots.diff.value?.snapshot.id, "snap-1");
  assert.deepEqual(client.invocations, [], "comparing reads through the server, and sends nothing");

  await store.snapshots.remove("snap-1");
  assert.equal(store.snapshots.diff.value, undefined, "a comparison with a snapshot that is gone is not left on screen");
});

test("a backup carries every snapshot whole, and importing one adds without replacing", async () => {
  const { client, store } = setup();
  client.storedSnapshots = [snapshot("snap-1", "Mine", "2026-09-17T20:15:00Z")];
  const all = await store.snapshots.all();
  assert.deepEqual(all?.map((s) => s.id), ["snap-1"]);
  assert.equal(all?.[0]?.devices["loopback-0"]?.sections["clock"] !== undefined, true, "a backup holds the values, not the summary");

  const result = await store.snapshots.importAll([snapshot("snap-1", "Theirs", "2020-01-01T00:00:00Z"), snapshot("snap-2", "Theirs too", "2026-09-16T10:00:00Z")]);
  assert.deepEqual(result, { added: ["snap-2"], skipped: ["snap-1"] });
  assert.equal(client.storedSnapshots.find((s) => s.id === "snap-1")?.name, "Mine");
});

test("what a snapshot and a diff say in words", () => {
  const now = new Date(2026, 8, 20, 12, 0);
  assert.equal(formatWhen("2026-09-17T20:15:00Z", now), formatWhen("2026-09-17T20:15:00Z", now), "a time renders the same twice");
  assert.match(formatWhen(new Date(2026, 8, 17, 21, 15).toISOString(), now), /^17 Sep, 21:15$/);
  assert.match(formatWhen(new Date(2024, 1, 29, 9, 5).toISOString(), now), /^29 Feb 2024, 09:05$/, "another year is named");
  assert.equal(formatWhen("not a date", now), "not a date");

  const summary = (devices: { model: string; unreadable: number }[]) => ({
    version: 1,
    id: "snap-1",
    name: "Take 1",
    created: "2026-09-17T20:15:00Z",
    note: "",
    devices: devices.map((d, i) => ({ device_id: `loopback-${i}`, family: "quadro", model: d.model, read_at: "2026-09-17T20:15:00Z", current_preset: 1, sections: ["clock"], unreadable: d.unreadable })),
  });
  assert.equal(describeSnapshot(summary([{ model: "Zen Quadro", unreadable: 0 }])), "Zen Quadro");
  assert.equal(describeSnapshot(summary([{ model: "Zen Quadro", unreadable: 1 }, { model: "Zen Studio+", unreadable: 11 }])), "Zen Quadro and Zen Studio+ · 12 values could not be read");
  assert.equal(describeSnapshot(summary([])), "no devices were attached");

  assert.equal(formatValue(64), "64");
  assert.equal(formatValue(undefined), "—");
  assert.equal(formatValue([1, 2]), "[1,2]");
  assert.equal(describeChange({ path: "p", label: "l", kind: "changed", from: 64, to: 50 }), "64 → 50");
  assert.equal(describeChange({ path: "p", label: "l", kind: "only_in_snapshot", from: 2 }), "was 2, not set now");
  assert.equal(describeChange({ path: "p", label: "l", kind: "only_now", to: 5 }), "not in the snapshot, now 5");
  assert.equal(describeChange({ path: "p", label: "l", kind: "unknown", reason: "the device refused it" }), "could not be read: the device refused it");

  const section = { section: "inputs", title: "Inputs", changes: [
    { path: "a", label: "a", kind: "changed" as const, from: 1, to: 2 },
    { path: "b", label: "b", kind: "changed" as const, from: 1, to: 3 },
    { path: "c", label: "c", kind: "unknown" as const, reason: "no report" },
  ] };
  assert.equal(describeSection(section), "2 changes, 1 value unread");

  const diff: SnapshotDiff = {
    snapshot: summary([{ model: "Zen Quadro", unreadable: 0 }]),
    compared_at: "2026-09-17T21:00:00Z",
    workspace: [],
    devices: [
      { device_id: "loopback-0", model: "Zen Quadro", family: "quadro", missing: false, added: false, sections: [section], changes: 3 },
      { device_id: "loopback-1", model: "Zen Studio+", family: "studio", missing: true, added: false, sections: [], changes: 0 },
    ],
    changes: 3,
    same: false,
  };
  assert.equal(describeDiff(diff), "2 differences · 1 value could not be read on one side or the other · 1 device is not attached.");
  assert.equal(describeDiff({ ...diff, devices: [], changes: 0, same: true }), "Nothing that could be read differs.");
});
