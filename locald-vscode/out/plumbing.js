"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.StreamingLineTail = void 0;
exports.getBinaryIdentity = getBinaryIdentity;
exports.formatBinaryIdentity = formatBinaryIdentity;
exports.findBinary = findBinary;
exports.resolveBinaryIdentityFrom = resolveBinaryIdentityFrom;
exports.ensureEditorProject = ensureEditorProject;
exports.renewEditorProject = renewEditorProject;
exports.releaseEditorProject = releaseEditorProject;
exports.status = status;
exports.listProjects = listProjects;
exports.stopProject = stopProject;
exports.restartService = restartService;
exports.restartCommandArgs = restartCommandArgs;
exports.getLogs = getLogs;
exports.logCommandArgs = logCommandArgs;
exports.parseJsonOutput = parseJsonOutput;
exports.tailLines = tailLines;
const node_child_process_1 = require("node:child_process");
const node_fs_1 = require("node:fs");
const node_os_1 = require("node:os");
const node_path_1 = require("node:path");
const DEFAULT_COMMAND_TIMEOUT_MS = 10_000;
const LIFECYCLE_COMMAND_TIMEOUT_MS = 0;
const MAX_CAPTURED_LOG_CHARACTERS = 1_000_000;
let cachedBinaryIdentity;
function getBinaryIdentity() {
    cachedBinaryIdentity ??= resolveBinaryIdentity();
    return cachedBinaryIdentity;
}
function formatBinaryIdentity(identity = getBinaryIdentity()) {
    return `${identity.path} (${identity.source})`;
}
function findBinary() {
    return getBinaryIdentity().path;
}
function resolveBinaryIdentity() {
    return resolveBinaryIdentityFrom({
        configuredPath: process.env.LOCALD_BINARY,
        homeDirectory: (0, node_os_1.homedir)(),
        path: process.env.PATH,
        exists: node_fs_1.existsSync,
    });
}
function resolveBinaryIdentityFrom(options) {
    const configuredPath = options.configuredPath?.trim();
    if (configuredPath) {
        return { path: configuredPath, source: "LOCALD_BINARY" };
    }
    const localInstallPath = (0, node_path_1.join)(options.homeDirectory, ".local", "bin", "locald");
    if (options.exists(localInstallPath)) {
        return { path: localInstallPath, source: "local install" };
    }
    for (const directory of options.path?.split(node_path_1.delimiter) ?? []) {
        if (!directory) {
            continue;
        }
        const candidate = (0, node_path_1.join)(directory, "locald");
        if (options.exists(candidate)) {
            return { path: candidate, source: "PATH" };
        }
    }
    const cargoPath = (0, node_path_1.join)(options.homeDirectory, ".cargo", "bin", "locald");
    if (options.exists(cargoPath)) {
        return { path: cargoPath, source: "cargo fallback" };
    }
    return { path: "locald", source: "PATH" };
}
function run(args, options = {}) {
    return new Promise((resolve, reject) => {
        const binary = getBinaryIdentity();
        (0, node_child_process_1.execFile)(binary.path, args, {
            cwd: options.cwd,
            timeout: options.timeout ?? DEFAULT_COMMAND_TIMEOUT_MS,
        }, (error, stdout, stderr) => {
            if (error) {
                reject(new Error(`${formatBinaryIdentity(binary)} ${args.join(" ")} failed: ${stderr || error.message}`));
            }
            else {
                resolve(stdout);
            }
        });
    });
}
async function ensureEditorProject(projectPath, windowId, hostPid) {
    const output = await run([
        "project",
        "editor",
        "ensure",
        projectPath,
        "--window-id",
        windowId,
        "--host-pid",
        String(hostPid),
        "--json",
    ], { timeout: LIFECYCLE_COMMAND_TIMEOUT_MS });
    return parseJsonOutput(output);
}
async function renewEditorProject(projectPath, windowId, hostPid) {
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
async function releaseEditorProject(projectPath, windowId, hostPid) {
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
async function status(projectPath) {
    const output = await run(["project", "status", projectPath, "--json"]);
    return parseJsonOutput(output);
}
async function listProjects() {
    const output = await run(["project", "list", "--json"]);
    return parseJsonOutput(output);
}
async function stopProject(projectPath) {
    await run(["stop", "--json"], {
        cwd: projectPath,
        timeout: LIFECYCLE_COMMAND_TIMEOUT_MS,
    });
}
async function restartService(projectPath, serviceName) {
    await run(restartCommandArgs(serviceName), {
        cwd: projectPath,
        timeout: LIFECYCLE_COMMAND_TIMEOUT_MS,
    });
}
function restartCommandArgs(serviceName) {
    return ["restart", "--json", "--", serviceName];
}
async function getLogs(projectPath, lines = 100, service) {
    const args = logCommandArgs(service);
    return runTail(args, lines, { cwd: projectPath });
}
function logCommandArgs(service) {
    return service ? ["logs", "--", service] : ["logs"];
}
function parseJsonOutput(output) {
    const trimmed = output.trim();
    try {
        return JSON.parse(trimmed);
    }
    catch (initialError) {
        const firstObject = trimmed.indexOf("{");
        const firstArray = trimmed.indexOf("[");
        const candidates = [firstObject, firstArray]
            .filter((offset) => offset >= 0)
            .sort((left, right) => left - right);
        for (const offset of candidates) {
            try {
                return JSON.parse(trimmed.slice(offset));
            }
            catch {
                // Try the next possible JSON payload start.
            }
        }
        throw initialError;
    }
}
function tailLines(output, lines) {
    const limit = Number.isFinite(lines) ? Math.max(1, Math.floor(lines)) : 100;
    const hasTrailingNewline = output.endsWith("\n");
    const body = hasTrailingNewline ? output.slice(0, -1) : output;
    return `${body.split("\n").slice(-limit).join("\n")}${hasTrailingNewline ? "\n" : ""}`;
}
class StreamingLineTail {
    output = "";
    lines;
    maxCharacters;
    constructor(lines, maxCharacters = MAX_CAPTURED_LOG_CHARACTERS) {
        this.lines = Number.isFinite(lines)
            ? Math.max(1, Math.floor(lines))
            : 100;
        this.maxCharacters = Math.max(1, Math.floor(maxCharacters));
    }
    push(chunk) {
        this.output = tailLines(this.output + chunk, this.lines);
        if (this.output.length > this.maxCharacters) {
            this.output = this.output.slice(-this.maxCharacters);
        }
    }
    value() {
        return this.output;
    }
}
exports.StreamingLineTail = StreamingLineTail;
function runTail(args, lines, options = {}) {
    return new Promise((resolve, reject) => {
        const binary = getBinaryIdentity();
        const child = (0, node_child_process_1.spawn)(binary.path, args, {
            cwd: options.cwd,
            stdio: ["ignore", "pipe", "pipe"],
        });
        const stdout = new StreamingLineTail(lines);
        const stderr = new StreamingLineTail(100);
        const timeout = options.timeout ?? DEFAULT_COMMAND_TIMEOUT_MS;
        const timer = timeout > 0
            ? setTimeout(() => {
                child.kill();
            }, timeout)
            : undefined;
        child.stdout.setEncoding("utf8");
        child.stdout.on("data", (chunk) => stdout.push(chunk));
        child.stderr.setEncoding("utf8");
        child.stderr.on("data", (chunk) => stderr.push(chunk));
        child.on("error", (error) => {
            if (timer) {
                clearTimeout(timer);
            }
            reject(new Error(`${formatBinaryIdentity(binary)} ${args.join(" ")} failed: ${error.message}`));
        });
        child.on("close", (code, signal) => {
            if (timer) {
                clearTimeout(timer);
            }
            if (code === 0) {
                resolve(stdout.value());
                return;
            }
            const detail = stderr.value() ||
                (signal
                    ? `terminated by ${signal}`
                    : `exited with status ${code ?? "unknown"}`);
            reject(new Error(`${formatBinaryIdentity(binary)} ${args.join(" ")} failed: ${detail}`));
        });
    });
}
//# sourceMappingURL=plumbing.js.map