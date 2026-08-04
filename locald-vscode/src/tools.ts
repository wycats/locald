import * as vscode from "vscode";
import type { EditorAvailabilityController } from "./editor-controller.js";
import { getLogs, restartService, status } from "./plumbing.js";
import { log } from "./extension.js";
import {
  defaultServiceWithOrigin,
  managedLifecycleServices,
  openedServiceMessage,
  restartedServicesMessage,
} from "./service-presentation.js";

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
  log.info("Registering 5 locald language-model tools");

  context.subscriptions.push(
    vscode.lm.registerTool("locald_ensure", {
      async invoke(
        _options: vscode.LanguageModelToolInvocationOptions<void>,
        _token: vscode.CancellationToken,
      ): Promise<vscode.LanguageModelToolResult> {
        const result = await controller.ensureCurrent(
          "language-model readiness request",
        );
        if (!result) {
          throw new Error("no locald project is selected");
        }
        return textResult(
          JSON.stringify(
            {
              state: result.state,
              services: result.services,
              urls: result.urls,
            },
            null,
            2,
          ),
        );
      },
    } satisfies vscode.LanguageModelTool<void>),
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
          publication: service.publication,
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
        const projectPath = await resolveRequiredProject(resolveProject);
        const initial = await status(projectPath);
        if (options.input?.service) {
          const service = initial.service_details.find(
            (candidate) =>
              candidate.name === options.input?.service ||
              candidate.name.endsWith(`:${options.input?.service}`),
          );
          if (!service) {
            throw new Error(`service ${options.input.service} was not found`);
          }
          if (service.service_type === "published") {
            throw new Error(
              `service ${options.input.service} is externally managed; use its owning workflow to restart it`,
            );
          }
          await restartService(projectPath, service.name);
        } else {
          const managed = managedLifecycleServices(initial.service_details);
          if (managed.length === 0) {
            throw new Error(
              "this project has no locald-managed services to restart; use each published service's owning workflow",
            );
          }
          for (const service of managed) {
            await restartService(projectPath, service.name);
          }
        }
        const final = await status(projectPath);
        const urls = final.service_details.flatMap((service) =>
          service.url ? [service.url] : [],
        );
        return textResult(
          restartedServicesMessage(final.service_details, urls),
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
        if (options.input?.service) {
          const info = await status(projectPath);
          const service = info.service_details.find(
            (candidate) =>
              candidate.name === options.input?.service ||
              candidate.name.endsWith(`:${options.input?.service}`),
          );
          if (service?.service_type === "published") {
            throw new Error(
              `service ${options.input.service} is externally managed; locald does not own its logs`,
            );
          }
        }
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
          : defaultServiceWithOrigin(result.services);
        if (!service?.url) {
          return textResult("No service URL available.");
        }
        await vscode.commands.executeCommand(
          "simpleBrowser.show",
          vscode.Uri.parse(service.url),
        );
        return textResult(openedServiceMessage(service));
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
