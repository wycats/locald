import * as vscode from "vscode";
import { attach, detach, startProject, status } from "./plumbing.js";
import { StatusBar } from "./status-bar.js";
import { registerTools } from "./tools.js";

let statusBar: StatusBar | undefined;
let projectPath: string | undefined;
let windowId: string | undefined;

export async function activate(context: vscode.ExtensionContext): Promise<void> {
  // Find locald.toml in the workspace
  const files = await vscode.workspace.findFiles("locald.toml", null, 1);
  if (files.length === 0) {
    return;
  }

  const tomlUri = files[0];
  projectPath = vscode.workspace.getWorkspaceFolder(tomlUri)?.uri.fsPath;
  if (!projectPath) {
    projectPath = tomlUri.fsPath.replace(/\/locald\.toml$/, "");
  }

  windowId = vscode.env.sessionId;

  // Attach to the project
  try {
    await attach(projectPath, windowId);
  } catch (e) {
    vscode.window.showWarningMessage(`locald: failed to attach — ${e instanceof Error ? e.message : e}`);
  }

  // Set context key for chatInstructions
  await vscode.commands.executeCommand("setContext", "locald:projectDetected", true);

  // Status bar
  statusBar = new StatusBar(projectPath);
  context.subscriptions.push(statusBar);
  statusBar.start();

  // Register Copilot tools
  registerTools(context, projectPath);

  // Commands
  context.subscriptions.push(
    vscode.commands.registerCommand("locald.openDashboard", async () => {
      try {
        const info = await status(projectPath!);
        // Dashboard runs on the daemon — find its URL from the first service or use default
        const dashboardUrl = `https://locald.localhost`;
        await vscode.commands.executeCommand(
          "simpleBrowser.show",
          vscode.Uri.parse(dashboardUrl),
        );
      } catch {
        await vscode.commands.executeCommand(
          "simpleBrowser.show",
          vscode.Uri.parse("https://locald.localhost"),
        );
      }
    }),
  );

  context.subscriptions.push(
    vscode.commands.registerCommand("locald.restartServices", async () => {
      try {
        await startProject(projectPath!);
        vscode.window.showInformationMessage("locald: services restarted");
      } catch (e) {
        vscode.window.showErrorMessage(`locald: restart failed — ${e instanceof Error ? e.message : e}`);
      }
    }),
  );

  context.subscriptions.push(
    vscode.commands.registerCommand("locald.stopServices", async () => {
      try {
        if (projectPath && windowId) {
          await detach(projectPath, windowId);
        }
        vscode.window.showInformationMessage("locald: services stopped");
      } catch (e) {
        vscode.window.showErrorMessage(`locald: stop failed — ${e instanceof Error ? e.message : e}`);
      }
    }),
  );
}

export async function deactivate(): Promise<void> {
  if (projectPath && windowId) {
    try {
      await detach(projectPath, windowId);
    } catch {
      // Best-effort detach on shutdown
    }
  }
}
