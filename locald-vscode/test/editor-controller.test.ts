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

test("restart work keeps its original project target until readiness returns", async () => {
  const fixture = setup("/work/one");
  await fixture.controller.activate();
  let queuedSwitch: Promise<EnsureProjectResult | undefined> | undefined;

  const result = await fixture.controller.withCurrentProject(
    "prepare restart",
    async (initial, ensureTarget) => {
      assert.equal(initial.project_path, "/work/one");
      fixture.projectPath = "/work/two";
      queuedSwitch = fixture.controller.ensureCurrent("active editor change");
      return ensureTarget("wait for restart");
    },
  );
  await queuedSwitch;

  assert.equal(result?.project_path, "/work/one");
  assert.equal(fixture.controller.projectPath, "/work/two");
  assert.deepEqual(fixture.calls, [
    "ensure:/work/one:window-a:42",
    "ensure:/work/one:window-a:42",
    "ensure:/work/one:window-a:42",
    "ensure:/work/two:window-a:42",
    "release:/work/one:window-a:42",
  ]);
});

test("heartbeat renews the fixed target during long restart work", async () => {
  const fixture = setup("/work/one");
  await fixture.controller.activate();
  let finishRestart: (() => void) | undefined;
  let restartStarted: (() => void) | undefined;
  const started = new Promise<void>((resolve) => {
    restartStarted = resolve;
  });
  const restartGate = new Promise<void>((resolve) => {
    finishRestart = resolve;
  });

  const restart = fixture.controller.withCurrentProject(
    "prepare restart",
    async (initial) => {
      restartStarted?.();
      await restartGate;
      return initial;
    },
  );
  await started;

  await fixture.controller.renewCurrent();

  assert.deepEqual(fixture.calls, [
    "ensure:/work/one:window-a:42",
    "ensure:/work/one:window-a:42",
    "renew:/work/one:window-a:42",
  ]);

  finishRestart?.();
  await restart;
});

test("failed project switch renews both confirmed and possibly-published demands", async () => {
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
  await fixture.controller.renewCurrent();

  assert.equal(fixture.controller.projectPath, "/work/one");
  assert.deepEqual(fixture.calls, [
    "ensure:/work/one:window-a:42",
    "ensure:/work/two:window-a:42",
    "renew:/work/two:window-a:42",
    "renew:/work/one:window-a:42",
  ]);
});

test("failed initial readiness keeps a possibly-published demand renewable", async () => {
  const fixture = setup("/work/one");
  fixture.client.ensure = async (path, windowId, hostPid) => {
    fixture.calls.push(`ensure:${path}:${windowId}:${hostPid}`);
    throw new Error("readiness failed");
  };

  await assert.rejects(fixture.controller.activate(), /readiness failed/);
  await fixture.controller.renewCurrent();
  await fixture.controller.releaseCurrent();

  assert.equal(fixture.controller.projectPath, undefined);
  assert.deepEqual(fixture.calls, [
    "ensure:/work/one:window-a:42",
    "renew:/work/one:window-a:42",
    "release:/work/one:window-a:42",
  ]);
});

test("successful selection releases earlier uncertain demands", async () => {
  const fixture = setup("/work/one");
  await fixture.controller.activate();
  fixture.projectPath = "/work/two";
  fixture.client.ensure = async (path, windowId, hostPid) => {
    fixture.calls.push(`ensure:${path}:${windowId}:${hostPid}`);
    if (path === "/work/two") {
      throw new Error("readiness failed");
    }
    return ready(path);
  };
  await assert.rejects(
    fixture.controller.ensureCurrent("active editor change"),
    /readiness failed/,
  );

  fixture.projectPath = "/work/three";
  await fixture.controller.ensureCurrent("active editor change");
  await fixture.controller.renewCurrent();

  assert.equal(fixture.controller.projectPath, "/work/three");
  assert.deepEqual(fixture.calls, [
    "ensure:/work/one:window-a:42",
    "ensure:/work/two:window-a:42",
    "ensure:/work/three:window-a:42",
    "release:/work/two:window-a:42",
    "release:/work/one:window-a:42",
    "renew:/work/three:window-a:42",
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
