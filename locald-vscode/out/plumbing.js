"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.attach = attach;
exports.detach = detach;
exports.status = status;
exports.listProjects = listProjects;
exports.startProject = startProject;
exports.getLogs = getLogs;
const node_child_process_1 = require("node:child_process");
const node_fs_1 = require("node:fs");
const node_os_1 = require("node:os");
const node_path_1 = require("node:path");
function findBinary() {
    const cargoPath = (0, node_path_1.join)((0, node_os_1.homedir)(), ".cargo", "bin", "locald");
    if ((0, node_fs_1.existsSync)(cargoPath)) {
        return cargoPath;
    }
    return "locald";
}
function run(args) {
    return new Promise((resolve, reject) => {
        (0, node_child_process_1.execFile)(findBinary(), args, { timeout: 10_000 }, (error, stdout, stderr) => {
            if (error) {
                reject(new Error(`locald ${args.join(" ")} failed: ${stderr || error.message}`));
            }
            else {
                resolve(stdout);
            }
        });
    });
}
async function attach(projectPath, windowId) {
    await run([
        "project", "attach", projectPath,
        "--source", "editor",
        "--editor-name", "vscode",
        "--editor-id", windowId,
        "--json",
    ]);
}
async function detach(projectPath, windowId) {
    await run([
        "project", "detach", projectPath,
        "--source", "editor",
        "--editor-id", windowId,
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
async function startProject(projectPath) {
    await run(["project", "start", projectPath]);
}
async function getLogs(lines = 100) {
    const output = await run(["logs", "--no-follow", "--lines", String(lines)]);
    return output;
}
//# sourceMappingURL=plumbing.js.map