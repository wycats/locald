import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

interface ExtensionManifest {
  activationEvents?: string[];
  contributes?: {
    languageModelTools?: Array<{ name: string }>;
  };
}

function readManifest(): ExtensionManifest {
  return JSON.parse(
    readFileSync("package.json", "utf8"),
  ) as ExtensionManifest;
}

test("activates for root and nested locald project configurations", () => {
  const manifest = readManifest();

  assert.deepEqual(manifest.activationEvents, [
    "workspaceContains:locald.toml",
    "workspaceContains:**/locald.toml",
  ]);
});

test("instructions reference contributed readiness and browser tools", () => {
  const manifest = readManifest();
  const toolNames = new Set(
    manifest.contributes?.languageModelTools?.map((tool) => tool.name),
  );
  const instructions = readFileSync("locald-instructions.md", "utf8");

  assert.ok(toolNames.has("locald_ensure"));
  assert.ok(toolNames.has("locald_open"));
  assert.match(instructions, /`locald_ensure`/);
  assert.match(instructions, /`locald_open`/);
});
