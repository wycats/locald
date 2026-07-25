import * as vscode from "vscode";
import {
  AmbiguousProjectError,
  selectProjectPath,
} from "./project-selection.js";
import { findProjectConfigPaths } from "./project-discovery.js";

export { AmbiguousProjectError, selectProjectPath };

export async function resolveCurrentProjectPath(): Promise<
  string | undefined
> {
  const configPaths = await findProjectConfigPaths((include, exclude) =>
    vscode.workspace.findFiles(include, exclude),
  );
  const activeDocument = vscode.window.activeTextEditor?.document;
  const activeFilePath =
    activeDocument?.uri.scheme === "file"
      ? activeDocument.uri.fsPath
      : undefined;
  return selectProjectPath(
    configPaths,
    activeFilePath,
  );
}
