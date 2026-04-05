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
    vscode.commands.registerCommand("locald.openDashboard", async () => {
      // Build a quick pick with dashboard + each web service
      const items: vscode.QuickPickItem[] = [
        {
          label: "$(browser) Dashboard",
          description: "locald dashboard",
          detail: projectName ?? "locald",
        },
      ];

      try {
        const info = await status(projectPath!);
        for (const svc of info.service_details ?? []) {
          if (svc.url && svc.status === "running") {
            const serviceName = svc.name.split(":").pop() ?? svc.name;
            items.push({
              label: `$(globe) ${serviceName}`,
              description: svc.domain ?? svc.url,
              detail: svc.url,
            });
          }
        }
      } catch {
        // Fall through with just the dashboard option
      }

      if (items.length === 1) {
        // No services with URLs — just open dashboard directly
        openInBrowser(
          `https://dashboard.dotlocal.localhost/?project=${encodeURIComponent(projectName ?? "")}`,
          "https://dashboard.dotlocal.localhost/**",
        );
        return;
      }

      const picked = await vscode.window.showQuickPick(items, {
        placeHolder: "Open in browser",
      });

      if (!picked) return;

      if (picked.label.includes("Dashboard")) {
        openInBrowser(
          `https://dashboard.dotlocal.localhost/?project=${encodeURIComponent(projectName ?? "")}`,
          "https://dashboard.dotlocal.localhost/**",
        );
      } else if (picked.detail) {
        // Each service gets its own persistent tab via reuseUrlFilter scoped to its domain
        const domain = picked.description ?? new URL(picked.detail).hostname;
        openInBrowser(picked.detail, `https://${domain}/**`);
      }
    }),
  );

  function openInBrowser(url: string, reuseFilter: string) {
    vscode.commands.executeCommand("workbench.action.browser.open", {
      url,
      reuseUrlFilter: reuseFilter,
    });
  }

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
