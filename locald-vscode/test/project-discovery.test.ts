import assert from "node:assert/strict";
import test from "node:test";
import {
  LOCALD_CONFIG_EXCLUDE,
  LOCALD_CONFIG_GLOB,
  findProjectConfigPaths,
} from "../src/project-discovery.js";

test("project discovery enumerates every matching configuration", async () => {
  const configs = Array.from({ length: 150 }, (_, index) => ({
    fsPath: `/work/project-${index}/locald.toml`,
  }));
  const calls: unknown[][] = [];

  const paths = await findProjectConfigPaths(async (...args) => {
    calls.push(args);
    return configs;
  });

  assert.deepEqual(calls, [[LOCALD_CONFIG_GLOB, LOCALD_CONFIG_EXCLUDE]]);
  assert.equal(paths.length, 150);
  assert.equal(paths.at(-1), "/work/project-149/locald.toml");
});
