import { execFile, spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { homedir } from "node:os";
import { delimiter, join } from "node:path";

const DEFAULT_COMMAND_TIMEOUT_MS = 10_000;
const LIFECYCLE_COMMAND_TIMEOUT_MS = 0;
const MAX_CAPTURED_LOG_CHARACTERS = 1_000_000;

export interface ServiceStatus {
  name: string;
  url: string | null;
  port: number | null;
  status: string;
  health_status: string;
  domain: string | null;
  service_type: string;
  publication?: PublicationStatus;
}

export interface PublicationStatus {
  state:
    | "waiting_for_publisher"
    | "checking_endpoint"
    | "endpoint_unhealthy"
    | "ready"
    | "route_paused"
    | "instance_missing";
  origin: string;
  explanation: string;
  next_step?: string;
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
  publication?: PublicationStatus;
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
            new Error(formatCommandFailure(binary, args, stderr || error.message)),
          );
        } else {
          resolve(stdout);
        }
      },
    );
  });
}

export function formatCommandFailure(
  binary: LocaldBinaryIdentity,
  args: string[],
  detail: string,
): string {
  const commandFailure =
    detail.trim() || "locald command exited without diagnostic output";
  const editorProtocolMismatch =
    args[0] === "project" &&
    args[1] === "editor" &&
    /unrecognized subcommand ['"]?editor['"]?/i.test(commandFailure);
  let remediation = "";
  if (editorProtocolMismatch) {
    const updateCli =
      binary.source === "LOCALD_BINARY"
        ? `Update or remove the \`LOCALD_BINARY\` override that selects ${binary.path}, or replace that binary with the current locald CLI.`
        : "Install the current locald CLI.";
    remediation = `\n${updateCli} Then run \`sudo locald admin setup\` and reload this VS Code window.`;
  }
  return `${formatBinaryIdentity(binary)} ${args.join(" ")} failed: ${commandFailure}${remediation}`;
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
    { timeout: LIFECYCLE_COMMAND_TIMEOUT_MS },
  );
  return parseJsonOutput<EnsureProjectResult>(output);
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
  return parseJsonOutput<ProjectStatusInfo>(output);
}

export async function listProjects(): Promise<ProjectStatusInfo[]> {
  const output = await run(["project", "list", "--json"]);
  return parseJsonOutput<ProjectStatusInfo[]>(output);
}

export async function stopProject(projectPath: string): Promise<void> {
  await run(["stop", "--json"], {
    cwd: projectPath,
    timeout: LIFECYCLE_COMMAND_TIMEOUT_MS,
  });
}

export async function restartService(
  projectPath: string,
  serviceName: string,
): Promise<void> {
  await run(restartCommandArgs(serviceName), {
    cwd: projectPath,
    timeout: LIFECYCLE_COMMAND_TIMEOUT_MS,
  });
}

export function restartCommandArgs(serviceName: string): string[] {
  return ["restart", "--json", "--", serviceName];
}

export async function getLogs(
  projectPath: string,
  lines: number = 100,
  service?: string,
): Promise<string> {
  const args = logCommandArgs(service);
  return runTail(args, lines, { cwd: projectPath });
}

export function logCommandArgs(service?: string): string[] {
  return service ? ["logs", "--", service] : ["logs"];
}

export function parseJsonOutput<T>(output: string): T {
  const trimmed = output.trim();
  try {
    return JSON.parse(trimmed) as T;
  } catch (initialError) {
    const firstObject = trimmed.indexOf("{");
    const firstArray = trimmed.indexOf("[");
    const candidates = [firstObject, firstArray]
      .filter((offset) => offset >= 0)
      .sort((left, right) => left - right);
    for (const offset of candidates) {
      try {
        return JSON.parse(trimmed.slice(offset)) as T;
      } catch {
        // Try the next possible JSON payload start.
      }
    }
    throw initialError;
  }
}

export function tailLines(output: string, lines: number): string {
  const limit = Number.isFinite(lines) ? Math.max(1, Math.floor(lines)) : 100;
  const hasTrailingNewline = output.endsWith("\n");
  const body = hasTrailingNewline ? output.slice(0, -1) : output;
  return `${body.split("\n").slice(-limit).join("\n")}${hasTrailingNewline ? "\n" : ""}`;
}

export class StreamingLineTail {
  private output = "";
  private readonly lines: number;
  private readonly maxCharacters: number;

  constructor(
    lines: number,
    maxCharacters: number = MAX_CAPTURED_LOG_CHARACTERS,
  ) {
    this.lines = Number.isFinite(lines)
      ? Math.max(1, Math.floor(lines))
      : 100;
    this.maxCharacters = Math.max(1, Math.floor(maxCharacters));
  }

  push(chunk: string): void {
    this.output = tailLines(this.output + chunk, this.lines);
    if (this.output.length > this.maxCharacters) {
      this.output = this.output.slice(-this.maxCharacters);
    }
  }

  value(): string {
    return this.output;
  }
}

function runTail(
  args: string[],
  lines: number,
  options: RunOptions = {},
): Promise<string> {
  return new Promise((resolve, reject) => {
    const binary = getBinaryIdentity();
    const child = spawn(binary.path, args, {
      cwd: options.cwd,
      stdio: ["ignore", "pipe", "pipe"],
    });
    const stdout = new StreamingLineTail(lines);
    const stderr = new StreamingLineTail(100);
    const timeout = options.timeout ?? DEFAULT_COMMAND_TIMEOUT_MS;
    const timer =
      timeout > 0
        ? setTimeout(() => {
            child.kill();
          }, timeout)
        : undefined;

    child.stdout.setEncoding("utf8");
    child.stdout.on("data", (chunk: string) => stdout.push(chunk));
    child.stderr.setEncoding("utf8");
    child.stderr.on("data", (chunk: string) => stderr.push(chunk));
    child.on("error", (error) => {
      if (timer) {
        clearTimeout(timer);
      }
      reject(
        new Error(
          `${formatBinaryIdentity(binary)} ${args.join(" ")} failed: ${error.message}`,
        ),
      );
    });
    child.on("close", (code, signal) => {
      if (timer) {
        clearTimeout(timer);
      }
      if (code === 0) {
        resolve(stdout.value());
        return;
      }
      const detail =
        stderr.value() ||
        (signal
          ? `terminated by ${signal}`
          : `exited with status ${code ?? "unknown"}`);
      reject(
        new Error(
          `${formatBinaryIdentity(binary)} ${args.join(" ")} failed: ${detail}`,
        ),
      );
    });
  });
}
