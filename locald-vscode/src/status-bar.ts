import * as vscode from "vscode";
import {
  attach,
  findBinary,
  status,
  type ProjectStatusInfo,
  type ServiceStatus,
} from "./plumbing.js";

const POLL_INTERVAL = 5_000;

export class StatusBar implements vscode.Disposable {
  private dashboardItem: vscode.StatusBarItem;
  private webItem: vscode.StatusBarItem;
  private timer: ReturnType<typeof setInterval> | undefined;
  private projectPath: string;
  private windowId: string;
  private log: vscode.LogOutputChannel;
  private webServices: ServiceStatus[] = [];
  private wasUnreachable = false;
  private consecutiveFailures = 0;

  constructor(
    projectPath: string,
    windowId: string,
    log: vscode.LogOutputChannel,
  ) {
    this.projectPath = projectPath;
    this.windowId = windowId;
    this.log = log;

    // Dashboard item (left)
    this.dashboardItem = vscode.window.createStatusBarItem(
      vscode.StatusBarAlignment.Left,
      51,
    );
    this.dashboardItem.command = "locald.openDashboard";
    this.dashboardItem.text = "$(server) locald";
    this.dashboardItem.tooltip = "locald — loading…";
    this.dashboardItem.show();

    // Web service item (right of dashboard)
    this.webItem = vscode.window.createStatusBarItem(
      vscode.StatusBarAlignment.Left,
      50,
    );
    this.webItem.command = "locald.openWebService";
  }

  start(): void {
    this.refresh();
    this.timer = setInterval(() => this.refresh(), POLL_INTERVAL);
  }

  getWebServices(): ServiceStatus[] {
    return this.webServices;
  }

  private async refresh(): Promise<void> {
    try {
      const info = await status(this.projectPath);
      if (this.wasUnreachable) {
        this.log.info(
          `locald daemon reachable again after ${this.consecutiveFailures} failed status poll${this.consecutiveFailures === 1 ? "" : "s"}`,
        );
        this.wasUnreachable = false;
        attach(this.projectPath, this.windowId).catch((error: unknown) => {
          this.log.warn(
            `Failed to re-attach editor after daemon recovery: ${formatError(error)}`,
          );
        });
      }
      this.consecutiveFailures = 0;
      this.updateDashboard(info);
      this.updateWebItem(info);
    } catch (error) {
      this.consecutiveFailures += 1;
      const message = formatError(error);
      if (!this.wasUnreachable) {
        this.log.warn(
          `locald status poll failed; retrying every ${POLL_INTERVAL / 1000}s using ${findBinary()}: ${message}`,
        );
      } else if (this.consecutiveFailures % 12 === 0) {
        this.log.warn(
          `locald status poll still failing after ${this.consecutiveFailures} attempts: ${message}`,
        );
      }
      this.wasUnreachable = true;
      this.dashboardItem.text = "$(error) locald";
      this.dashboardItem.tooltip = [
        "locald — unable to reach daemon",
        `Retrying every ${POLL_INTERVAL / 1000}s.`,
        "",
        message,
      ].join("\n");
      this.webItem.hide();
    }
  }

  private updateDashboard(info: ProjectStatusInfo): void {
    const name = info.project_name ?? "locald";
    const services = info.service_details ?? [];
    const total = services.length || info.services.length;
    const healthy = services.filter(
      (s) => s.health_status === "Healthy",
    ).length;

    if (total === 0) {
      this.dashboardItem.text = `$(server) ${name}`;
      this.dashboardItem.tooltip = `${name} — no services`;
    } else if (healthy === total) {
      this.dashboardItem.text = `$(server) ${name} · ${total} service${total !== 1 ? "s" : ""}`;
      this.dashboardItem.tooltip = this.buildTooltip(name, services, info);
    } else {
      this.dashboardItem.text = `$(warning) ${name} · ${healthy}/${total}`;
      this.dashboardItem.tooltip = this.buildTooltip(name, services, info);
    }
  }

  private updateWebItem(info: ProjectStatusInfo): void {
    const services = info.service_details ?? [];
    this.webServices = services.filter((s) => s.url && s.status === "running");

    if (this.webServices.length === 0) {
      this.webItem.hide();
      return;
    }

    if (this.webServices.length === 1) {
      const svc = this.webServices[0];
      const label = svc.name.split(":").pop() ?? svc.name;
      this.webItem.text = `$(globe) ${label}`;
      this.webItem.tooltip = svc.domain ?? svc.url ?? label;
    } else {
      // Multiple web services — show count, click for picker
      this.webItem.text = `$(globe) ${this.webServices.length} sites`;
      this.webItem.tooltip = this.webServices
        .map((s) => s.name.split(":").pop() ?? s.name)
        .join(", ");
    }
    this.webItem.show();
  }

  private buildTooltip(
    name: string,
    services: ServiceStatus[],
    info: ProjectStatusInfo,
  ): string {
    const lines: string[] = [`${name} — editor attached`];
    lines.push("Services stop when this window closes.");
    lines.push("");
    for (const s of services) {
      const icon = s.status === "running" ? "●" : "○";
      const url = s.domain ? `  ${s.domain}` : "";
      lines.push(`${icon} ${s.name}${url}`);
    }
    if (services.length === 0) {
      for (const n of info.services) {
        lines.push(`● ${n}`);
      }
    }
    return lines.join("\n");
  }

  dispose(): void {
    if (this.timer !== undefined) {
      clearInterval(this.timer);
      this.timer = undefined;
    }
    this.dashboardItem.dispose();
    this.webItem.dispose();
  }
}

function formatError(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
