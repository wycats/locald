import * as vscode from "vscode";
import type { EditorAvailabilityController } from "./editor-controller.js";
import { getLogs, restartService, status } from "./plumbing.js";
import { log } from "./extension.js";

type ProjectResolver = () => Promise<string | undefined>;

export function registerTools(
  context: vscode.ExtensionContext,
  controller: EditorAvailabilityController,
  resolveProject: ProjectResolver,
): void {
  if (!vscode.lm || typeof vscode.lm.registerTool !== "function") {
    log.warn("vscode.lm.registerTool not available — tools disabled");
    return;
  }
  log.info("Registering 4 locald language-model tools");

  context.subscriptions.push(
    vscode.lm.registerTool("locald_services", {
      async invoke(
        _options: vscode.LanguageModelToolInvocationOptions<void>,
        _token: vscode.CancellationToken,
      ): Promise<vscode.LanguageModelToolResult> {
        const projectPath = await resolveRequiredProject(resolveProject);
        const info = await status(projectPath);
        const services = info.service_details.map((service) => ({
          name: service.name,
          status: service.status,
          health_status: service.health_status,
          url: service.url,
          domain: service.domain,
          service_type: service.service_type,
        }));
        return textResult(JSON.stringify(services, null, 2));
      },
    } satisfies vscode.LanguageModelTool<void>),
    vscode.lm.registerTool("locald_restart", {
      async invoke(
        options: vscode.LanguageModelToolInvocationOptions<{
          service?: string;
        }>,
        _token: vscode.CancellationToken,
      ): Promise<vscode.LanguageModelToolResult> {
        const initial = await controller.ensureCurrent(
          "language-model restart request",
        );
        if (!initial) {
          throw new Error("no locald project is selected");
        }
        const services = options.input?.service
          ? initial.services.filter(
              (candidate) =>
                candidate.name === options.input?.service ||
                candidate.name.endsWith(`:${options.input?.service}`),
            )
          : initial.services;
        if (services.length === 0) {
          throw new Error(
            `service ${options.input?.service ?? "(unknown)"} was not found`,
          );
        }
        for (const service of services) {
          await restartService(initial.project_path, service.name);
        }
        const result = await controller.ensureCurrent(
          "wait for restarted services",
        );
        if (!result) {
          throw new Error("locald project was removed during restart");
        }
        return textResult(
          `Services restarted and ready.${result.urls.length > 0 ? ` ${result.urls.join(" ")}` : ""}`,
        );
      },
    } satisfies vscode.LanguageModelTool<{ service?: string }>),
    vscode.lm.registerTool("locald_logs", {
      async invoke(
        options: vscode.LanguageModelToolInvocationOptions<{
          service?: string;
          lines?: number;
        }>,
        _token: vscode.CancellationToken,
      ): Promise<vscode.LanguageModelToolResult> {
        const projectPath = await resolveRequiredProject(resolveProject);
        const output = await getLogs(
          projectPath,
          options.input?.lines ?? 200,
          options.input?.service,
        );
        return textResult(output);
      },
    } satisfies vscode.LanguageModelTool<{
      service?: string;
      lines?: number;
    }>),
    vscode.lm.registerTool("locald_open", {
      async invoke(
        options: vscode.LanguageModelToolInvocationOptions<{
          service?: string;
        }>,
        _token: vscode.CancellationToken,
      ): Promise<vscode.LanguageModelToolResult> {
        const result = await controller.ensureCurrent(
          "language-model browser request",
        );
        if (!result) {
          throw new Error("no locald project is selected");
        }
        const service = options.input?.service
          ? result.services.find(
              (candidate) =>
                candidate.name === options.input?.service ||
                candidate.name.endsWith(`:${options.input?.service}`),
            )
          : result.services.find((candidate) => candidate.url);
        if (!service?.url) {
          return textResult("No service URL available.");
        }
        await vscode.commands.executeCommand(
          "simpleBrowser.show",
          vscode.Uri.parse(service.url),
        );
        return textResult(`Opened ${service.url} in Simple Browser.`);
      },
    } satisfies vscode.LanguageModelTool<{ service?: string }>),
  );
}

async function resolveRequiredProject(
  resolveProject: ProjectResolver,
): Promise<string> {
  const projectPath = await resolveProject();
  if (!projectPath) {
    throw new Error("no locald.toml found in the current workspace");
  }
  return projectPath;
}

function textResult(text: string): vscode.LanguageModelToolResult {
  return new vscode.LanguageModelToolResult([
    new vscode.LanguageModelTextPart(text),
  ]);
}
