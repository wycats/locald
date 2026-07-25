import * as vscode from "vscode";
import {
  AmbiguousProjectError,
  selectProjectPath,
} from "./project-selection.js";

export { AmbiguousProjectError, selectProjectPath };

const LOCALD_CONFIG_GLOB = "**/locald.toml";
const LOCALD_CONFIG_EXCLUDE =
  "**/{.git,node_modules,target,dist,build,.next}/**";

export async function resolveCurrentProjectPath(): Promise<
  string | undefined
> {
  const configs = await vscode.workspace.findFiles(
    LOCALD_CONFIG_GLOB,
    LOCALD_CONFIG_EXCLUDE,
    100,
  );
  const activeDocument = vscode.window.activeTextEditor?.document;
  const activeFilePath =
    activeDocument?.uri.scheme === "file"
      ? activeDocument.uri.fsPath
      : undefined;
  return selectProjectPath(
    configs.map((config) => config.fsPath),
    activeFilePath,
  );
}
