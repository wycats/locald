import { execFile } from "node:child_process";
import { existsSync } from "node:fs";
import { homedir } from "node:os";
import { delimiter, join } from "node:path";

const DEFAULT_COMMAND_TIMEOUT_MS = 10_000;
const ENSURE_COMMAND_TIMEOUT_MS = 45_000;

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
  availability?: {
    desired: boolean;
    state: string;
    always_on: boolean;
    paused: boolean;
    reasons: unknown[];
    demands: Array<{
      kind: string;
      safe_label: string;
      expires_at?: unknown;
    }>;
    next_transition_at?: unknown;
    last_error?: string;
  };
}

export interface EnsuredServiceStatus {
  name: string;
  service_type: string;
  status: string;
  health_status: string;
  url?: string;
}

export interface EnsureProjectResult {
  project_path: string;
  project_name?: string;
  state: "ready";
  services: EnsuredServiceStatus[];
  urls: string[];
}

export type LocaldBinarySource =
  | "LOCALD_BINARY"
  | "local install"
  | "cargo fallback"
  | "PATH";

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
  return resolveBinaryIdentityFrom({
    configuredPath: process.env.LOCALD_BINARY,
    homeDirectory: homedir(),
    path: process.env.PATH,
    exists: existsSync,
  });
}

export function resolveBinaryIdentityFrom(options: {
  configuredPath?: string;
  homeDirectory: string;
  path?: string;
  exists(path: string): boolean;
}): LocaldBinaryIdentity {
  const configuredPath = options.configuredPath?.trim();
  if (configuredPath) {
    return { path: configuredPath, source: "LOCALD_BINARY" };
  }

  const localInstallPath = join(
    options.homeDirectory,
    ".local",
    "bin",
    "locald",
  );
  if (options.exists(localInstallPath)) {
    return { path: localInstallPath, source: "local install" };
  }

  for (const directory of options.path?.split(delimiter) ?? []) {
    if (!directory) {
      continue;
    }
    const candidate = join(directory, "locald");
    if (options.exists(candidate)) {
      return { path: candidate, source: "PATH" };
    }
  }

  const cargoPath = join(
    options.homeDirectory,
    ".cargo",
    "bin",
    "locald",
  );
  if (options.exists(cargoPath)) {
    return { path: cargoPath, source: "cargo fallback" };
  }
  return { path: "locald", source: "PATH" };
}

interface RunOptions {
  cwd?: string;
  timeout?: number;
}

function run(args: string[], options: RunOptions = {}): Promise<string> {
  return new Promise((resolve, reject) => {
    const binary = getBinaryIdentity();
    execFile(
      binary.path,
      args,
      {
        cwd: options.cwd,
        timeout: options.timeout ?? DEFAULT_COMMAND_TIMEOUT_MS,
      },
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

export async function ensureEditorProject(
  projectPath: string,
  windowId: string,
  hostPid: number,
): Promise<EnsureProjectResult> {
  const output = await run(
    [
      "project",
      "editor",
      "ensure",
      projectPath,
      "--window-id",
      windowId,
      "--host-pid",
      String(hostPid),
      "--json",
    ],
    { timeout: ENSURE_COMMAND_TIMEOUT_MS },
  );
  return JSON.parse(output) as EnsureProjectResult;
}

export async function renewEditorProject(
  projectPath: string,
  windowId: string,
  hostPid: number,
): Promise<void> {
  await run([
    "project",
    "editor",
    "renew",
    projectPath,
    "--window-id",
    windowId,
    "--host-pid",
    String(hostPid),
    "--json",
  ]);
}

export async function releaseEditorProject(
  projectPath: string,
  windowId: string,
  hostPid: number,
): Promise<void> {
  await run([
    "project",
    "editor",
    "release",
    projectPath,
    "--window-id",
    windowId,
    "--host-pid",
    String(hostPid),
    "--json",
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

export async function stopProject(projectPath: string): Promise<void> {
  await run(["stop", "--json"], { cwd: projectPath });
}

export async function restartService(
  projectPath: string,
  serviceName: string,
): Promise<void> {
  await run(["restart", serviceName, "--json"], { cwd: projectPath });
}

export async function getLogs(
  projectPath: string,
  lines: number = 100,
  service?: string,
): Promise<string> {
  const args = service ? ["logs", service] : ["logs"];
  const output = await run(args, { cwd: projectPath });
  return tailLines(output, lines);
}

export function tailLines(output: string, lines: number): string {
  const limit = Number.isFinite(lines) ? Math.max(1, Math.floor(lines)) : 100;
  const hasTrailingNewline = output.endsWith("\n");
  const body = hasTrailingNewline ? output.slice(0, -1) : output;
  return `${body.split("\n").slice(-limit).join("\n")}${hasTrailingNewline ? "\n" : ""}`;
}
