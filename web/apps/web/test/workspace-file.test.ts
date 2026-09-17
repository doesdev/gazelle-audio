// Workspace backup files: the name an export gets, the text it holds, and what an import checks
// before asking the server (which has the last word on everything past the document's shape).

import { test } from "node:test";
import assert from "node:assert/strict";

import { readWorkspaceFile, workspaceFileName, workspaceFileText } from "../src/store/workspace-file.ts";

test("an export is named with the local date", () => {
  assert.equal(workspaceFileName(new Date(2026, 8, 16, 23, 59)), "gazelle-workspace-2026-09-16.json");
  assert.equal(workspaceFileName(new Date(2027, 0, 5, 0, 0)), "gazelle-workspace-2027-01-05.json");
});

test("an export is the workspace as JSON, unknown fields and all, and reads back unchanged", () => {
  const workspace = {
    version: 1,
    groups: [],
    links: [],
    aliases: { "loopback-0": "Desk" },
    mixers: { "loopback-0": { mixes: [], groups: [], channels: [{ id: "c1", name: "Kick", slot: 6, sends: [], color: "#aabbcc", shade: "new" }] } },
    future: { added: "by a later version" },
  };
  const text = workspaceFileText(workspace as never);
  assert.deepEqual(JSON.parse(text), workspace);
  assert.ok(text.endsWith("\n"), "ends with a newline, as text files do");
  const read = readWorkspaceFile(text, 1);
  assert.equal(read.ok, true);
  if (read.ok) assert.deepEqual(read.workspace, workspace, "nothing is dropped, added or defaulted");
});

test("a file that is not a workspace is refused with a reason, before anything is sent", () => {
  const problem = (text: string, version = 1) => {
    const read = readWorkspaceFile(text, version);
    return read.ok ? undefined : read.problem;
  };
  assert.match(problem("{ not json") ?? "", /is not JSON/);
  assert.equal(problem("[1, 2]"), "is not a Gazelle workspace");
  assert.equal(problem("null"), "is not a Gazelle workspace");
  assert.match(problem('{"groups": []}') ?? "", /has no version/);
  assert.match(problem('{"version": "1"}') ?? "", /has no version/);
  assert.match(problem('{"version": 0}') ?? "", /has no version/);
  assert.match(problem('{"version": 1.5}') ?? "", /has no version/);
  assert.equal(problem('{"version": 2}', 1), "was written by a newer Gazelle (workspace version 2); this server reads version 1");
  assert.equal(problem('{"version": 1, "groups": {}}'), "has groups that are not a list");
  assert.equal(problem('{"version": 1, "links": "none"}'), "has links that are not a list");
  assert.equal(problem('{"version": 1, "layouts": {}}'), "has layouts that are not a list");
  assert.equal(problem('{"version": 1, "aliases": []}'), "has aliases that are not a map of devices");
  assert.equal(problem('{"version": 1, "mixers": null}'), "has mixers that are not a map of devices");
  const bare = readWorkspaceFile('{"version": 1}', 1);
  assert.deepEqual(bare.ok ? bare.workspace : undefined, { version: 1 }, "missing parts are the server's to default");
});

test("a read file is summarised for the confirmation, with every device it names", () => {
  const read = readWorkspaceFile(
    JSON.stringify({
      version: 1,
      aliases: { "loopback-0": "Desk", "serial-9": "Rack" },
      groups: [{ id: "g", name: "G", collapsed: false, hidden: false, members: [{ device_id: "loopback-1", channel: 0 }], children: [{ id: "h", name: "H", collapsed: false, hidden: false, members: [], children: [] }] }],
      links: [{ id: "l", kind: "preamp", mode: "absolute", members: [{ device_id: "loopback-0", channel: 0 }, { device_id: "serial-7", channel: 1 }] }],
      mixers: { "loopback-1": { mixes: [], groups: [], channels: [] } },
      layouts: [{ id: "a", name: "A", family: "quadro", mixer: { mixes: [], groups: [], channels: [] } }],
      device_colors: { "serial-5": "#000000" },
      surfaces: [{ id: "s", name: "S", mixes: { "loopback-0": 1 }, strips: [{ id: "x", kind: "master", device_id: "serial-3" }, { id: "y", kind: "label", text: "" }] }],
      cables: [{ id: "c", from: { device_id: "serial-1", port: "ADAT_OUT", first: 0 }, to: { device_id: "loopback-0", port: "ADAT_IN", first: 0 }, channels: 8 }],
    }),
    1,
  );
  assert.equal(read.ok, true);
  if (!read.ok) return;
  assert.deepEqual(read.summary, { names: 2, groups: 2, links: 1, mixers: 1, layouts: 1, surfaces: 1, cables: 1, devices: ["loopback-0", "loopback-1", "serial-1", "serial-3", "serial-5", "serial-7", "serial-9"] });
});
