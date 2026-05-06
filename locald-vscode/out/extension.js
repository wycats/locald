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
exports.log = void 0;
exports.activate = activate;
exports.deactivate = deactivate;
const vscode = __importStar(require("vscode"));
const plumbing_js_1 = require("./plumbing.js");
const status_bar_js_1 = require("./status-bar.js");
const tools_js_1 = require("./tools.js");
let statusBar;
let projectPath;
let projectName;
let windowId;
let dashboardPanel;
exports.log = vscode.window.createOutputChannel("locald", { log: true });
async function activate(context) {
    context.subscriptions.push(exports.log);
    exports.log.info("Extension activating...");
    exports.log.info(`Using locald binary ${(0, plumbing_js_1.formatBinaryIdentity)()}`);
    const files = await vscode.workspace.findFiles("locald.toml", null, 1);
    if (files.length === 0) {
        exports.log.info("No locald.toml found, deactivating");
        return;
    }
    exports.log.info("Found locald.toml, proceeding with activation");
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
    // Get project name for deep linking
    try {
        const info = await (0, plumbing_js_1.status)(projectPath);
        projectName = info.project_name ?? projectPath.split("/").pop();
    }
    catch {
        projectName = projectPath.split("/").pop();
    }
    // Set context key for chatInstructions
    await vscode.commands.executeCommand("setContext", "locald:projectDetected", true);
    // Status bar
    statusBar = new status_bar_js_1.StatusBar(projectPath, windowId, exports.log);
    context.subscriptions.push(statusBar);
    statusBar.start();
    // Register Copilot tools
    (0, tools_js_1.registerTools)(context, projectPath);
    // Commands
    function openInBrowser(url, reuseFilter) {
        vscode.commands.executeCommand("workbench.action.browser.open", {
            url,
            reuseUrlFilter: reuseFilter,
        });
    }
    context.subscriptions.push(vscode.commands.registerCommand("locald.openDashboard", () => {
        const url = projectName
            ? `https://dashboard.dotlocal.localhost/?project=${encodeURIComponent(projectName)}`
            : "https://dashboard.dotlocal.localhost";
        openInBrowser(url, "https://dashboard.dotlocal.localhost/**");
    }));
    context.subscriptions.push(vscode.commands.registerCommand("locald.openWebService", async () => {
        const webServices = statusBar.getWebServices();
        if (webServices.length === 0)
            return;
        if (webServices.length === 1) {
            const svc = webServices[0];
            if (svc.url && svc.domain) {
                openInBrowser(svc.url, `https://${svc.domain}/**`);
            }
            return;
        }
        // Multiple web services — show picker
        const items = webServices.map((svc) => ({
            label: svc.name.split(":").pop() ?? svc.name,
            description: svc.domain ?? undefined,
            detail: svc.url ?? undefined,
        }));
        const picked = await vscode.window.showQuickPick(items, {
            placeHolder: "Open web service",
        });
        if (picked?.detail && picked?.description) {
            openInBrowser(picked.detail, `https://${picked.description}/**`);
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
function getDashboardHtml(url) {
    return `<!DOCTYPE html>
<html style="height:100%;width:100%;margin:0;padding:0;">
<body style="height:100%;width:100%;margin:0;padding:0;overflow:hidden;">
<iframe src="${url}" style="width:100%;height:100%;border:none;"></iframe>
</body>
</html>`;
}
//# sourceMappingURL=extension.js.map