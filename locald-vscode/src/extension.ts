import * as vscode from "vscode";
import { attach, detach, startProject, status } from "./plumbing.js";
import { StatusBar } from "./status-bar.js";
import { registerTools } from "./tools.js";

let statusBar: StatusBar | undefined;
let projectPath: string | undefined;
let projectName: string | undefined;
let windowId: string | undefined;
let dashboardPanel: vscode.WebviewPanel | undefined;

export async function activate(
  context: vscode.ExtensionContext,
): Promise<void> {
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
    vscode.window.showWarningMessage(
      `locald: failed to attach — ${e instanceof Error ? e.message : e}`,
    );
  }

  // Get project name for deep linking
  try {
    const info = await status(projectPath);
    projectName = info.project_name ?? projectPath.split("/").pop();
  } catch {
    projectName = projectPath.split("/").pop();
  }

  // Set context key for chatInstructions
  await vscode.commands.executeCommand(
    "setContext",
    "locald:projectDetected",
    true,
  );

  // Status bar
  statusBar = new StatusBar(projectPath);
  context.subscriptions.push(statusBar);
  statusBar.start();

  // Register Copilot tools
  registerTools(context, projectPath);

  // Commands
  context.subscriptions.push(
    vscode.commands.registerCommand("locald.openDashboard", () => {
      const url = projectName
        ? `https://dashboard.dotlocal.localhost/?project=${encodeURIComponent(projectName)}`
        : "https://dashboard.dotlocal.localhost";

      if (dashboardPanel) {
        // Reuse existing panel — update URL and reveal it
        dashboardPanel.webview.html = getDashboardHtml(url);
        dashboardPanel.reveal(vscode.ViewColumn.Beside, false);
      } else {
        dashboardPanel = vscode.window.createWebviewPanel(
          "locald.dashboard",
          "locald Dashboard",
          { viewColumn: vscode.ViewColumn.Beside, preserveFocus: false },
          { enableScripts: true, retainContextWhenHidden: true },
        );
        dashboardPanel.webview.html = getDashboardHtml(url);
        dashboardPanel.onDidDispose(() => {
          dashboardPanel = undefined;
        });
      }
    }),
  );

  context.subscriptions.push(
    vscode.commands.registerCommand("locald.restartServices", async () => {
      try {
        await startProject(projectPath!);
        vscode.window.showInformationMessage("locald: services restarted");
      } catch (e) {
        vscode.window.showErrorMessage(
          `locald: restart failed — ${e instanceof Error ? e.message : e}`,
        );
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
        vscode.window.showErrorMessage(
          `locald: stop failed — ${e instanceof Error ? e.message : e}`,
        );
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

function getDashboardHtml(url: string): string {
  return `<!DOCTYPE html>
<html style="height:100%;width:100%;margin:0;padding:0;">
<body style="height:100%;width:100%;margin:0;padding:0;overflow:hidden;">
<iframe src="${url}" style="width:100%;height:100%;border:none;"></iframe>
</body>
</html>`;
}
