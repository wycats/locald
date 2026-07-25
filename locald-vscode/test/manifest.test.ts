import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

interface ExtensionManifest {
  activationEvents?: string[];
}

test("activates for root and nested locald project configurations", () => {
  const manifest = JSON.parse(
    readFileSync("package.json", "utf8"),
  ) as ExtensionManifest;

  assert.deepEqual(manifest.activationEvents, [
    "workspaceContains:locald.toml",
    "workspaceContains:**/locald.toml",
  ]);
});
