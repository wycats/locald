import * as vscode from "vscode";
import { EditorAvailabilityController } from "./editor-controller.js";
import {
  ensureEditorProject,
  formatBinaryIdentity,
  releaseEditorProject,
  renewEditorProject,
  restartService,
  status,
  stopProject,
} from "./plumbing.js";
import { StatusBar } from "./status-bar.js";
import { registerTools } from "./tools.js";
import { managedLifecycleServices } from "./service-presentation.js";
import { resolveCurrentProjectPath } from "./workspace-project.js";

let statusBar: StatusBar | undefined;
let editorController: EditorAvailabilityController | undefined;

export const log = vscode.window.createOutputChannel("locald", { log: true });

export async function activate(
  context: vscode.ExtensionContext,
): Promise<void> {
  context.subscriptions.push(log);
  log.info("Extension activating...");
  log.info(`Using locald binary ${formatBinaryIdentity()}`);

  editorController = new EditorAvailabilityController({
    windowId: vscode.env.sessionId,
    hostPid: process.pid,
    resolveProject: resolveCurrentProjectPath,
    client: {
      ensure: ensureEditorProject,
      renew: renewEditorProject,
      release: releaseEditorProject,
    },
    log,
  });

  statusBar = new StatusBar(
    () => editorController?.projectPath,
    log,
    (paused) => {
      void editorController
        ?.recoverAfterDaemonReconnect(paused)
        .catch((error: unknown) => {
          log.warn(
            `Editor demand recovery after daemon reconnect failed: ${formatError(error)}`,
          );
        });
    },
  );
  context.subscriptions.push(statusBar);
  statusBar.start();

  registerTools(context, editorController, resolveCurrentProjectPath);
  registerCommands(context, editorController);
  registerSemanticActivity(context, editorController);

  try {
    const result = await editorController.activate();
    await vscode.commands.executeCommand(
      "setContext",
      "locald:projectDetected",
      result !== undefined,
    );
    if (!result) {
      log.info("No locald.toml found in the current workspace");
    }
  } catch (error) {
    await vscode.commands.executeCommand(
      "setContext",
      "locald:projectDetected",
      editorController.projectPath !== undefined,
    );
    vscode.window.showWarningMessage(
      `locald: project activation failed — ${formatError(error)}`,
    );
  }
}

export async function deactivate(): Promise<void> {
  const controller = editorController;
  editorController = undefined;
  if (!controller) {
    return;
  }

  try {
    await controller.releaseCurrent();
  } catch {
    // Best effort. The daemon also expires and reaps stale window ownership.
  } finally {
    controller.dispose();
  }
}

function registerSemanticActivity(
  context: vscode.ExtensionContext,
  controller: EditorAvailabilityController,
): void {
  const ensureAfterActivity = (reason: string): void => {
    void controller
      .ensureCurrent(reason)
      .then((result) =>
        vscode.commands.executeCommand(
          "setContext",
          "locald:projectDetected",
          result !== undefined,
        ),
      )
      .catch((error: unknown) => {
        log.warn(
          `Editor activity could not ensure locald: ${formatError(error)}`,
        );
      });
  };

  context.subscriptions.push(
    vscode.window.onDidChangeWindowState((state) => {
      if (state.focused) {
        ensureAfterActivity("window refocus");
      }
    }),
    vscode.window.onDidChangeActiveTextEditor(() => {
      if (vscode.window.state.focused) {
        ensureAfterActivity("active editor change");
      }
    }),
    vscode.workspace.onDidChangeWorkspaceFolders(() => {
      if (vscode.window.state.focused) {
        ensureAfterActivity("workspace folder change");
      }
    }),
  );
}

function registerCommands(
  context: vscode.ExtensionContext,
  controller: EditorAvailabilityController,
): void {
  const openInBrowser = (url: string, reuseFilter: string): void => {
    void vscode.commands.executeCommand("workbench.action.browser.open", {
      url,
      reuseUrlFilter: reuseFilter,
    });
  };

  context.subscriptions.push(
    vscode.commands.registerCommand("locald.openDashboard", async () => {
      try {
        const projectPath = await resolveRequiredProjectPath();
        const info = await status(projectPath);
        const url = info.project_name
          ? `https://locald.localhost/?project=${encodeURIComponent(info.project_name)}`
          : "https://locald.localhost";
        openInBrowser(url, "https://locald.localhost/**");
      } catch (error) {
        vscode.window.showErrorMessage(
          `locald: dashboard unavailable — ${formatError(error)}`,
        );
      }
    }),
    vscode.commands.registerCommand("locald.openWebService", async () => {
      try {
        const result = await controller.ensureCurrent("open web service");
        const webServices =
          result?.services.filter((service) => service.url) ?? [];

        if (webServices.length === 0) {
          vscode.window.showInformationMessage(
            "locald: this project has no web service URL",
          );
          return;
        }

        if (webServices.length === 1) {
          const service = webServices[0];
          if (service.url) {
            openInBrowser(service.url, `${service.url}/**`);
          }
          return;
        }

        const picked = await vscode.window.showQuickPick(
          webServices.map((service) => ({
            label: service.name.split(":").pop() ?? service.name,
            description: service.url,
            detail: service.publication?.explanation,
          })),
          { placeHolder: "Open web service" },
        );
        if (picked?.description) {
          openInBrowser(picked.description, `${picked.description}/**`);
        }
      } catch (error) {
        vscode.window.showErrorMessage(
          `locald: web service unavailable — ${formatError(error)}`,
        );
      }
    }),
    vscode.commands.registerCommand("locald.restartServices", async () => {
      try {
        const projectPath = await resolveRequiredProjectPath();
        const initial = await status(projectPath);
        const managed = managedLifecycleServices(initial.service_details);
        if (managed.length === 0) {
          throw new Error(
            "this project has no locald-managed services to restart; use each published service's owning workflow",
          );
        }
        for (const service of managed) {
          await restartService(projectPath, service.name);
        }
        const final = await status(projectPath);
        const published = final.service_details.filter(
          (service) => service.service_type === "published",
        ).length;
        vscode.window.showInformationMessage(
          published > 0
            ? `locald: managed services restarted; ${published} published service${published === 1 ? " remains" : "s remain"} externally managed`
            : "locald: managed services restarted",
        );
      } catch (error) {
        vscode.window.showErrorMessage(
          `locald: restart failed — ${formatError(error)}`,
        );
      }
    }),
    vscode.commands.registerCommand("locald.stopServices", async () => {
      try {
        const projectPath = await resolveRequiredProjectPath();
        await stopProject(projectPath);
        vscode.window.showInformationMessage("locald: project paused");
      } catch (error) {
        vscode.window.showErrorMessage(
          `locald: pause failed — ${formatError(error)}`,
        );
      }
    }),
  );
}

async function resolveRequiredProjectPath(): Promise<string> {
  const projectPath = await resolveCurrentProjectPath();
  if (!projectPath) {
    throw new Error("no locald.toml found in the current workspace");
  }
  return projectPath;
}

function formatError(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
