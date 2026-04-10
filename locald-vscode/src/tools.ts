import * as vscode from "vscode";
import { status, startProject, getLogs } from "./plumbing.js";
import { log } from "./extension.js";

export function registerTools(
  context: vscode.ExtensionContext,
  projectPath: string,
): void {
  if (!vscode.lm || typeof vscode.lm.registerTool !== "function") {
    log.warn("vscode.lm.registerTool not available — tools disabled");
    return;
  }
  log.info("Registering 4 Copilot tools");

  context.subscriptions.push(
    vscode.lm.registerTool("locald_services", {
      async invoke(
        _options: vscode.LanguageModelToolInvocationOptions<void>,
        _token: vscode.CancellationToken,
      ): Promise<vscode.LanguageModelToolResult> {
        const info = await status(projectPath);
        const text = JSON.stringify(info.service_details, null, 2);
        return new vscode.LanguageModelToolResult([
          new vscode.LanguageModelTextPart(text),
        ]);
      },
    } satisfies vscode.LanguageModelTool<void>),
  );

  context.subscriptions.push(
    vscode.lm.registerTool("locald_restart", {
      async invoke(
        options: vscode.LanguageModelToolInvocationOptions<{
          service?: string;
        }>,
        _token: vscode.CancellationToken,
      ): Promise<vscode.LanguageModelToolResult> {
        void options.input?.service;
        await startProject(projectPath);
        return new vscode.LanguageModelToolResult([
          new vscode.LanguageModelTextPart("Services restarted."),
        ]);
      },
    } satisfies vscode.LanguageModelTool<{ service?: string }>),
  );

  context.subscriptions.push(
    vscode.lm.registerTool("locald_logs", {
      async invoke(
        options: vscode.LanguageModelToolInvocationOptions<{
          service?: string;
          lines?: number;
        }>,
        _token: vscode.CancellationToken,
      ): Promise<vscode.LanguageModelToolResult> {
        void options.input?.service;
        const output = await getLogs(options.input?.lines ?? 200);
        return new vscode.LanguageModelToolResult([
          new vscode.LanguageModelTextPart(output),
        ]);
      },
    } satisfies vscode.LanguageModelTool<{
      service?: string;
      lines?: number;
    }>),
  );

  context.subscriptions.push(
    vscode.lm.registerTool("locald_open", {
      async invoke(
        options: vscode.LanguageModelToolInvocationOptions<{
          service?: string;
        }>,
        _token: vscode.CancellationToken,
      ): Promise<vscode.LanguageModelToolResult> {
        const info = await status(projectPath);
        const service = options.input?.service
          ? info.service_details.find((s) => s.name === options.input?.service)
          : info.service_details.find((s) => s.url);
        if (!service?.url) {
          return new vscode.LanguageModelToolResult([
            new vscode.LanguageModelTextPart("No service URL available."),
          ]);
        }
        await vscode.commands.executeCommand(
          "simpleBrowser.show",
          vscode.Uri.parse(service.url),
        );
        return new vscode.LanguageModelToolResult([
          new vscode.LanguageModelTextPart(
            `Opened ${service.url} in Simple Browser.`,
          ),
        ]);
      },
    } satisfies vscode.LanguageModelTool<{ service?: string }>),
  );
}
