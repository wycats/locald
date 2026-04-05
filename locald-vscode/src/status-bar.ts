import * as vscode from "vscode";
import { status, type ProjectStatusInfo } from "./plumbing.js";

const POLL_INTERVAL = 5_000;

export class StatusBar implements vscode.Disposable {
  private item: vscode.StatusBarItem;
  private timer: ReturnType<typeof setInterval> | undefined;
  private projectPath: string;

  constructor(projectPath: string) {
    this.projectPath = projectPath;
    this.item = vscode.window.createStatusBarItem(
      vscode.StatusBarAlignment.Left,
      50,
    );
    this.item.command = "locald.openDashboard";
    this.item.text = "$(server) locald";
    this.item.tooltip = "locald — loading…";
    this.item.show();
  }

  start(): void {
    this.refresh();
    this.timer = setInterval(() => this.refresh(), POLL_INTERVAL);
  }

  private async refresh(): Promise<void> {
    try {
      const info = await status(this.projectPath);
      this.update(info);
    } catch {
      this.item.text = "$(error) locald";
      this.item.tooltip = "locald — unable to reach daemon";
    }
  }

  private update(info: ProjectStatusInfo): void {
    const name = info.project_name ?? "locald";
    const services = info.service_details ?? [];
    const total = services.length || info.services.length;
    const healthy = services.filter(
      (s) => s.health_status === "Healthy",
    ).length;

    if (total === 0) {
      this.item.text = `$(server) ${name}`;
      this.item.tooltip = `${name} — no services`;
    } else if (healthy === total) {
      this.item.text = `$(server) ${name} · ${total} service${total !== 1 ? "s" : ""}`;
      this.item.tooltip = this.buildTooltip(name, services, info);
    } else {
      this.item.text = `$(warning) ${name} · ${healthy}/${total}`;
      this.item.tooltip = this.buildTooltip(name, services, info);
    }
  }

  private buildTooltip(
    name: string,
    services: import("./plumbing.js").ServiceStatus[],
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
    this.item.dispose();
  }
}
