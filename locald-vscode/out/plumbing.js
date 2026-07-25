"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
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
exports.getLogs = getLogs;
exports.tailLines = tailLines;
const node_child_process_1 = require("node:child_process");
const node_fs_1 = require("node:fs");
const node_os_1 = require("node:os");
const node_path_1 = require("node:path");
const DEFAULT_COMMAND_TIMEOUT_MS = 10_000;
const ENSURE_COMMAND_TIMEOUT_MS = 45_000;
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
    ], { timeout: ENSURE_COMMAND_TIMEOUT_MS });
    return JSON.parse(output);
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
    return JSON.parse(output);
}
async function listProjects() {
    const output = await run(["project", "list", "--json"]);
    return JSON.parse(output);
}
async function stopProject(projectPath) {
    await run(["stop", "--json"], { cwd: projectPath });
}
async function restartService(projectPath, serviceName) {
    await run(["restart", serviceName, "--json"], { cwd: projectPath });
}
async function getLogs(projectPath, lines = 100, service) {
    const args = service ? ["logs", service] : ["logs"];
    const output = await run(args, { cwd: projectPath });
    return tailLines(output, lines);
}
function tailLines(output, lines) {
    const limit = Number.isFinite(lines) ? Math.max(1, Math.floor(lines)) : 100;
    const hasTrailingNewline = output.endsWith("\n");
    const body = hasTrailingNewline ? output.slice(0, -1) : output;
    return `${body.split("\n").slice(-limit).join("\n")}${hasTrailingNewline ? "\n" : ""}`;
}
//# sourceMappingURL=plumbing.js.map