import assert from "node:assert/strict";
import test from "node:test";
import {
  resolveBinaryIdentityFrom,
  tailLines,
} from "../src/plumbing.js";

test("tailLines keeps the requested final complete lines", () => {
  assert.equal(tailLines("one\ntwo\nthree\n", 2), "two\nthree\n");
  assert.equal(tailLines("one\ntwo\nthree", 2), "two\nthree");
  assert.equal(tailLines("one  \ntwo  \n", 2), "one  \ntwo  \n");
});

test("tailLines normalizes invalid limits to a safe snapshot", () => {
  assert.equal(tailLines("one\ntwo\n", 0), "two\n");
  assert.equal(tailLines("one\ntwo\n", Number.NaN), "one\ntwo\n");
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
