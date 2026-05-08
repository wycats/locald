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

export type LocaldBinarySource = "LOCALD_BINARY" | "cargo" | "PATH";

export interface LocaldBinaryIdentity {
  path: string;
  source: LocaldBinarySource;
}

let cachedBinaryIdentity: LocaldBinaryIdentity | undefined;

export function getBinaryIdentity(): LocaldBinaryIdentity {
  cachedBinaryIdentity ??= resolveBinaryIdentity();
  return cachedBinaryIdentity;
}

export function formatBinaryIdentity(identity = getBinaryIdentity()): string {
  return `${identity.path} (${identity.source})`;
}

export function findBinary(): string {
  return getBinaryIdentity().path;
}

function resolveBinaryIdentity(): LocaldBinaryIdentity {
  const configuredPath = process.env.LOCALD_BINARY?.trim();
  if (configuredPath) {
    return { path: configuredPath, source: "LOCALD_BINARY" };
  }

  const cargoPath = join(homedir(), ".cargo", "bin", "locald");
  if (existsSync(cargoPath)) {
    return { path: cargoPath, source: "cargo" };
  }
  return { path: "locald", source: "PATH" };
}

function run(args: string[]): Promise<string> {
  return new Promise((resolve, reject) => {
    const binary = getBinaryIdentity();
    execFile(
      binary.path,
      args,
      { timeout: 10_000 },
      (error, stdout, stderr) => {
        if (error) {
          reject(
            new Error(
              `${formatBinaryIdentity(binary)} ${args.join(" ")} failed: ${stderr || error.message}`,
            ),
          );
        } else {
          resolve(stdout);
        }
      },
    );
  });
}

export async function attach(
  projectPath: string,
  windowId: string,
): Promise<void> {
  await run([
    "project",
    "attach",
    projectPath,
    "--source",
    "editor",
    "--editor-name",
    "vscode",
    "--editor-id",
    windowId,
    "--editor-pid",
    String(process.pid),
    "--json",
  ]);
}

export async function detach(
  projectPath: string,
  windowId: string,
): Promise<void> {
  await run([
    "project",
    "detach",
    projectPath,
    "--source",
    "editor",
    "--editor-id",
    windowId,
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
