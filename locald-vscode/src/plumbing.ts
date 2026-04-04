import { execFile } from "node:child_process";
import { existsSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

export interface ServiceStatus {
  name: string;
  url: string | null;
  port: number | null;
  status: string;
  health_status: string;
  domain: string | null;
  service_type: string;
}

export interface ProjectStatusInfo {
  project_path: string;
  project_name: string | null;
  services: string[];
  service_details: ServiceStatus[];
  attachments: unknown[];
  is_running: boolean;
}

function findBinary(): string {
  const cargoPath = join(homedir(), ".cargo", "bin", "locald");
  if (existsSync(cargoPath)) {
    return cargoPath;
  }
  return "locald";
}

function run(args: string[]): Promise<string> {
  return new Promise((resolve, reject) => {
    execFile(findBinary(), args, { timeout: 10_000 }, (error, stdout, stderr) => {
      if (error) {
        reject(new Error(`locald ${args.join(" ")} failed: ${stderr || error.message}`));
      } else {
        resolve(stdout);
      }
    });
  });
}

export async function attach(projectPath: string, windowId: string): Promise<void> {
  await run([
    "project", "attach", projectPath,
    "--source", "editor",
    "--editor-name", "vscode",
    "--editor-id", windowId,
    "--json",
  ]);
}

export async function detach(projectPath: string, windowId: string): Promise<void> {
  await run([
    "project", "detach", projectPath,
    "--source", "editor",
    "--editor-id", windowId,
  ]);
}

export async function status(projectPath: string): Promise<ProjectStatusInfo> {
  const output = await run(["project", "status", projectPath, "--json"]);
  return JSON.parse(output) as ProjectStatusInfo;
}

export async function listProjects(): Promise<ProjectStatusInfo[]> {
  const output = await run(["project", "list", "--json"]);
  return JSON.parse(output) as ProjectStatusInfo[];
}

export async function startProject(projectPath: string): Promise<void> {
  await run(["project", "start", projectPath]);
}

export async function getLogs(lines: number = 100): Promise<string> {
  const output = await run(["logs", "--no-follow", "--lines", String(lines)]);
  return output;
}
