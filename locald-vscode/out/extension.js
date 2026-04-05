"use strict";
var __createBinding = (this && this.__createBinding) || (Object.create ? (function(o, m, k, k2) {
    if (k2 === undefined) k2 = k;
    var desc = Object.getOwnPropertyDescriptor(m, k);
    if (!desc || ("get" in desc ? !m.__esModule : desc.writable || desc.configurable)) {
      desc = { enumerable: true, get: function() { return m[k]; } };
    }
    Object.defineProperty(o, k2, desc);
}) : (function(o, m, k, k2) {
    if (k2 === undefined) k2 = k;
    o[k2] = m[k];
}));
var __setModuleDefault = (this && this.__setModuleDefault) || (Object.create ? (function(o, v) {
    Object.defineProperty(o, "default", { enumerable: true, value: v });
}) : function(o, v) {
    o["default"] = v;
});
var __importStar = (this && this.__importStar) || (function () {
    var ownKeys = function(o) {
        ownKeys = Object.getOwnPropertyNames || function (o) {
            var ar = [];
            for (var k in o) if (Object.prototype.hasOwnProperty.call(o, k)) ar[ar.length] = k;
            return ar;
        };
        return ownKeys(o);
    };
    return function (mod) {
        if (mod && mod.__esModule) return mod;
        var result = {};
        if (mod != null) for (var k = ownKeys(mod), i = 0; i < k.length; i++) if (k[i] !== "default") __createBinding(result, mod, k[i]);
        __setModuleDefault(result, mod);
        return result;
    };
})();
Object.defineProperty(exports, "__esModule", { value: true });
exports.activate = activate;
exports.deactivate = deactivate;
const vscode = __importStar(require("vscode"));
const plumbing_js_1 = require("./plumbing.js");
const status_bar_js_1 = require("./status-bar.js");
const tools_js_1 = require("./tools.js");
let statusBar;
let projectPath;
let windowId;
async function activate(context) {
    // Find locald.toml in the workspace
    const files = await vscode.workspace.findFiles("locald.toml", null, 1);
    if (files.length === 0) {
        return;
    }
    const tomlUri = files[0];
    projectPath = vscode.workspace.getWorkspaceFolder(tomlUri)?.uri.fsPath;
    if (!projectPath) {
        projectPath = tomlUri.fsPath.replace(/\/locald\.toml$/, "");
    }
    windowId = vscode.env.sessionId;
    // Attach to the project
    try {
        await (0, plumbing_js_1.attach)(projectPath, windowId);
    }
    catch (e) {
        vscode.window.showWarningMessage(`locald: failed to attach — ${e instanceof Error ? e.message : e}`);
    }
    // Set context key for chatInstructions
    await vscode.commands.executeCommand("setContext", "locald:projectDetected", true);
    // Status bar
    statusBar = new status_bar_js_1.StatusBar(projectPath);
    context.subscriptions.push(statusBar);
    statusBar.start();
    // Register Copilot tools
    (0, tools_js_1.registerTools)(context, projectPath);
    // Commands
    context.subscriptions.push(vscode.commands.registerCommand("locald.openDashboard", async () => {
        try {
            const info = await (0, plumbing_js_1.status)(projectPath);
            const dashboardUrl = "https://locald.localhost";
            await vscode.commands.executeCommand("simpleBrowser.show", vscode.Uri.parse(dashboardUrl));
        }
        catch {
            await vscode.commands.executeCommand("simpleBrowser.show", vscode.Uri.parse("https://locald.localhost"));
        }
    }));
    context.subscriptions.push(vscode.commands.registerCommand("locald.restartServices", async () => {
        try {
            await (0, plumbing_js_1.startProject)(projectPath);
            vscode.window.showInformationMessage("locald: services restarted");
        }
        catch (e) {
            vscode.window.showErrorMessage(`locald: restart failed — ${e instanceof Error ? e.message : e}`);
        }
    }));
    context.subscriptions.push(vscode.commands.registerCommand("locald.stopServices", async () => {
        try {
            if (projectPath && windowId) {
                await (0, plumbing_js_1.detach)(projectPath, windowId);
            }
            vscode.window.showInformationMessage("locald: services stopped");
        }
        catch (e) {
            vscode.window.showErrorMessage(`locald: stop failed — ${e instanceof Error ? e.message : e}`);
        }
    }));
}
async function deactivate() {
    if (projectPath && windowId) {
        try {
            await (0, plumbing_js_1.detach)(projectPath, windowId);
        }
        catch {
            // Best-effort detach on shutdown
        }
    }
}
//# sourceMappingURL=extension.js.map