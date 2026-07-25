import assert from "node:assert/strict";
import test from "node:test";
import {
  EDITOR_RENEWAL_INTERVAL_MS,
  EditorAvailabilityController,
  type EditorLifecycleClient,
} from "../src/editor-controller.js";
import type { EnsureProjectResult } from "../src/plumbing.js";

function ready(projectPath: string): EnsureProjectResult {
  return {
    project_path: projectPath,
    project_name: "project",
    state: "ready",
    services: [],
    urls: [],
  };
}

function setup(initialPath = "/work/project") {
  let projectPath: string | undefined = initialPath;
  let scheduledRenewal: (() => void) | undefined;
  let cancelled = false;
  const calls: string[] = [];
  const client: EditorLifecycleClient = {
    async ensure(path, windowId, hostPid) {
      calls.push(`ensure:${path}:${windowId}:${hostPid}`);
      return ready(path);
    },
    async renew(path, windowId, hostPid) {
      calls.push(`renew:${path}:${windowId}:${hostPid}`);
    },
    async release(path, windowId, hostPid) {
      calls.push(`release:${path}:${windowId}:${hostPid}`);
    },
  };
  const controller = new EditorAvailabilityController({
    windowId: "window-a",
    hostPid: 42,
    resolveProject: async () => projectPath,
    client,
    log: { info() {}, warn() {} },
    scheduleRenewal(renew, intervalMs) {
      assert.equal(intervalMs, EDITOR_RENEWAL_INTERVAL_MS);
      scheduledRenewal = renew;
      return () => {
        cancelled = true;
      };
    },
  });
  return {
    calls,
    client,
    controller,
    get scheduledRenewal() {
      return scheduledRenewal;
    },
    set projectPath(path: string | undefined) {
      projectPath = path;
    },
    get cancelled() {
      return cancelled;
    },
  };
}

test("activation ensures readiness with authenticated window provenance", async () => {
  const fixture = setup();

  const result = await fixture.controller.activate();

  assert.equal(result?.state, "ready");
  assert.equal(fixture.controller.projectPath, "/work/project");
  assert.deepEqual(fixture.calls, [
    "ensure:/work/project:window-a:42",
  ]);
  assert.ok(fixture.scheduledRenewal);
});

test("heartbeat passively renews without performing a semantic ensure", async () => {
  const fixture = setup();
  await fixture.controller.activate();

  fixture.scheduledRenewal?.();
  await fixture.controller.releaseCurrent();

  assert.deepEqual(fixture.calls, [
    "ensure:/work/project:window-a:42",
    "renew:/work/project:window-a:42",
    "release:/work/project:window-a:42",
  ]);
});

test("refocus performs a semantic ensure that can cross a pause barrier", async () => {
  const fixture = setup();
  await fixture.controller.activate();

  await fixture.controller.ensureCurrent("window refocus");

  assert.deepEqual(fixture.calls, [
    "ensure:/work/project:window-a:42",
    "ensure:/work/project:window-a:42",
  ]);
});

test("changing projects ensures the new project before releasing the old one", async () => {
  const fixture = setup("/work/one");
  await fixture.controller.activate();
  fixture.projectPath = "/work/two";

  await fixture.controller.ensureCurrent("active editor change");

  assert.equal(fixture.controller.projectPath, "/work/two");
  assert.deepEqual(fixture.calls, [
    "ensure:/work/one:window-a:42",
    "ensure:/work/two:window-a:42",
    "release:/work/one:window-a:42",
  ]);
});

test("failed readiness remains the current demand and releases the old project", async () => {
  const fixture = setup("/work/one");
  await fixture.controller.activate();
  fixture.projectPath = "/work/two";
  fixture.client.ensure = async (path, windowId, hostPid) => {
    fixture.calls.push(`ensure:${path}:${windowId}:${hostPid}`);
    throw new Error("readiness failed");
  };

  await assert.rejects(
    fixture.controller.ensureCurrent("active editor change"),
    /readiness failed/,
  );

  assert.equal(fixture.controller.projectPath, "/work/two");
  assert.deepEqual(fixture.calls, [
    "ensure:/work/one:window-a:42",
    "ensure:/work/two:window-a:42",
    "release:/work/one:window-a:42",
  ]);
});

test("leaving the last locald workspace releases the window demand", async () => {
  const fixture = setup();
  await fixture.controller.activate();
  fixture.projectPath = undefined;

  const result = await fixture.controller.ensureCurrent(
    "workspace folder change",
  );

  assert.equal(result, undefined);
  assert.equal(fixture.controller.projectPath, undefined);
  assert.deepEqual(fixture.calls, [
    "ensure:/work/project:window-a:42",
    "release:/work/project:window-a:42",
  ]);
});

test("dispose cancels heartbeat scheduling", async () => {
  const fixture = setup();
  await fixture.controller.activate();

  fixture.controller.dispose();

  assert.equal(fixture.cancelled, true);
});
