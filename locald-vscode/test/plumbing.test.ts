import assert from "node:assert/strict";
import test from "node:test";
import {
  logCommandArgs,
  parseJsonOutput,
  resolveBinaryIdentityFrom,
  restartCommandArgs,
  StreamingLineTail,
  tailLines,
} from "../src/plumbing.js";

test("log service names are separated from CLI options", () => {
  assert.deepEqual(logCommandArgs(), ["logs"]);
  assert.deepEqual(logCommandArgs("web"), ["logs", "--", "web"]);
  assert.deepEqual(logCommandArgs("--follow"), [
    "logs",
    "--",
    "--follow",
  ]);
});

test("restart service names are separated from CLI options", () => {
  assert.deepEqual(restartCommandArgs("web"), [
    "restart",
    "--json",
    "--",
    "web",
  ]);
  assert.deepEqual(restartCommandArgs("--help"), [
    "restart",
    "--json",
    "--",
    "--help",
  ]);
});

test("tailLines keeps the requested final complete lines", () => {
  assert.equal(tailLines("one\ntwo\nthree\n", 2), "two\nthree\n");
  assert.equal(tailLines("one\ntwo\nthree", 2), "two\nthree");
  assert.equal(tailLines("one  \ntwo  \n", 2), "one  \ntwo  \n");
});

test("tailLines normalizes invalid limits to a safe snapshot", () => {
  assert.equal(tailLines("one\ntwo\n", 0), "two\n");
  assert.equal(tailLines("one\ntwo\n", Number.NaN), "one\ntwo\n");
});

test("parseJsonOutput accepts machine-clean JSON and legacy startup prefixes", () => {
  assert.deepEqual(parseJsonOutput<{ ready: boolean }>('{"ready":true}\n'), {
    ready: true,
  });
  assert.deepEqual(
    parseJsonOutput<{ ready: boolean }>(
      'Starting locald server...\n{\n  "ready": true\n}\n',
    ),
    { ready: true },
  );
});

test("StreamingLineTail bounds chunked output before returning recent lines", () => {
  const tail = new StreamingLineTail(2, 100);
  tail.push("one\ntwo");
  tail.push("\nthree\n");

  assert.equal(tail.value(), "two\nthree\n");
});

test("StreamingLineTail bounds a single oversized line while streaming", () => {
  const tail = new StreamingLineTail(2, 8);
  tail.push("0123456789");

  assert.equal(tail.value(), "23456789");
});

test("binary selection prefers explicit and installed product paths", () => {
  const existing = new Set([
    "/home/user/.local/bin/locald",
    "/home/user/.cargo/bin/locald",
    "/usr/local/bin/locald",
  ]);
  const resolve = (configuredPath?: string) =>
    resolveBinaryIdentityFrom({
      configuredPath,
      homeDirectory: "/home/user",
      path: "/usr/local/bin",
      exists: (path) => existing.has(path),
    });

  assert.deepEqual(resolve("/custom/locald"), {
    path: "/custom/locald",
    source: "LOCALD_BINARY",
  });
  assert.deepEqual(resolve(), {
    path: "/home/user/.local/bin/locald",
    source: "local install",
  });
});

test("binary selection uses PATH before the Cargo development fallback", () => {
  const existing = new Set([
    "/home/user/.cargo/bin/locald",
    "/opt/homebrew/bin/locald",
  ]);

  assert.deepEqual(
    resolveBinaryIdentityFrom({
      homeDirectory: "/home/user",
      path: "/usr/bin:/opt/homebrew/bin",
      exists: (path) => existing.has(path),
    }),
    {
      path: "/opt/homebrew/bin/locald",
      source: "PATH",
    },
  );
});
