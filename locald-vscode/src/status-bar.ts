import * as vscode from "vscode";
import {
  formatBinaryIdentity,
  status,
  type ProjectStatusInfo,
  type ServiceStatus,
} from "./plumbing.js";

const POLL_INTERVAL = 5_000;

export class StatusBar implements vscode.Disposable {
  private dashboardItem: vscode.StatusBarItem;
  private webItem: vscode.StatusBarItem;
  private timer: ReturnType<typeof setInterval> | undefined;
  private readonly getProjectPath: () => string | undefined;
  private log: vscode.LogOutputChannel;
  private webServices: ServiceStatus[] = [];
  private wasUnreachable = false;
  private consecutiveFailures = 0;

  constructor(
    getProjectPath: () => string | undefined,
    log: vscode.LogOutputChannel,
  ) {
    this.getProjectPath = getProjectPath;
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
    const projectPath = this.getProjectPath();
    if (!projectPath) {
      this.dashboardItem.text = "$(server) locald";
      this.dashboardItem.tooltip =
        "locald — focus a file inside a locald project";
      this.webServices = [];
      this.webItem.hide();
      return;
    }

    try {
      const info = await status(projectPath);
      if (this.wasUnreachable) {
        this.log.info(
          `locald daemon reachable again after ${this.consecutiveFailures} failed status poll${this.consecutiveFailures === 1 ? "" : "s"}`,
        );
        this.wasUnreachable = false;
      }
      this.consecutiveFailures = 0;
      this.updateDashboard(info);
      this.updateWebItem(info);
    } catch (error) {
      this.consecutiveFailures += 1;
      const message = formatError(error);
      if (!this.wasUnreachable) {
        this.log.warn(
          `locald status poll failed; retrying every ${POLL_INTERVAL / 1000}s using ${formatBinaryIdentity()}: ${message}`,
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
        `Binary: ${formatBinaryIdentity()}`,
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
      this.dashboardItem.tooltip = this.withBinaryInfo(`${name} — no services`);
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
    const lines: string[] = [`${name} — managed for this VS Code window`];
    if (info.availability?.paused) {
      lines.push("Paused until the next explicit activity.");
    } else if (info.availability?.always_on) {
      lines.push("Always On is enabled.");
    } else {
      lines.push("Idle services stop after this window demand expires.");
    }
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
    return this.withBinaryInfo(lines.join("\n"));
  }

  private withBinaryInfo(tooltip: string): string {
    return [tooltip, "", `Binary: ${formatBinaryIdentity()}`].join("\n");
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
