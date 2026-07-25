import assert from "node:assert/strict";
import test from "node:test";
import {
  AmbiguousProjectError,
  selectProjectPath,
} from "../src/project-selection.js";

test("selects the only locald project in a window", () => {
  assert.equal(
    selectProjectPath(["/work/project/locald.toml"]),
    "/work/project",
  );
});

test("selects the deepest project containing the active file", () => {
  assert.equal(
    selectProjectPath(
      ["/work/locald.toml", "/work/nested/locald.toml"],
      "/work/nested/src/main.ts",
    ),
    "/work/nested",
  );
});

test("uses the active file to disambiguate a multi-root window", () => {
  assert.equal(
    selectProjectPath(
      ["/work/one/locald.toml", "/work/two/locald.toml"],
      "/work/two/src/main.ts",
    ),
    "/work/two",
  );
});

test("fails clearly when several projects are ambiguous", () => {
  assert.throws(
    () =>
      selectProjectPath([
        "/work/one/locald.toml",
        "/work/two/locald.toml",
      ]),
    AmbiguousProjectError,
  );
});

test("returns no project when the window has no locald config", () => {
  assert.equal(selectProjectPath([]), undefined);
});
