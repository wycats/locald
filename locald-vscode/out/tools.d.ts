import * as vscode from "vscode";
import type { EditorAvailabilityController } from "./editor-controller.js";
type ProjectResolver = () => Promise<string | undefined>;
export declare function registerTools(context: vscode.ExtensionContext, controller: EditorAvailabilityController, resolveProject: ProjectResolver): void;
export {};
