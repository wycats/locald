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
const editor_controller_js_1 = require("./editor-controller.js");
const plumbing_js_1 = require("./plumbing.js");
const status_bar_js_1 = require("./status-bar.js");
const tools_js_1 = require("./tools.js");
const workspace_project_js_1 = require("./workspace-project.js");
let statusBar;
let editorController;
exports.log = vscode.window.createOutputChannel("locald", { log: true });
async function activate(context) {
    context.subscriptions.push(exports.log);
    exports.log.info("Extension activating...");
    exports.log.info(`Using locald binary ${(0, plumbing_js_1.formatBinaryIdentity)()}`);
    editorController = new editor_controller_js_1.EditorAvailabilityController({
        windowId: vscode.env.sessionId,
        hostPid: process.pid,
        resolveProject: workspace_project_js_1.resolveCurrentProjectPath,
        client: {
            ensure: plumbing_js_1.ensureEditorProject,
            renew: plumbing_js_1.renewEditorProject,
            release: plumbing_js_1.releaseEditorProject,
        },
        log: exports.log,
    });
    statusBar = new status_bar_js_1.StatusBar(() => editorController?.projectPath, exports.log);
    context.subscriptions.push(statusBar);
    statusBar.start();
    (0, tools_js_1.registerTools)(context, editorController, workspace_project_js_1.resolveCurrentProjectPath);
    registerCommands(context, editorController);
    registerSemanticActivity(context, editorController);
    try {
        const result = await editorController.activate();
        await vscode.commands.executeCommand("setContext", "locald:projectDetected", result !== undefined);
        if (!result) {
            exports.log.info("No locald.toml found in the current workspace");
        }
    }
    catch (error) {
        await vscode.commands.executeCommand("setContext", "locald:projectDetected", editorController.projectPath !== undefined);
        vscode.window.showWarningMessage(`locald: project activation failed — ${formatError(error)}`);
    }
}
async function deactivate() {
    const controller = editorController;
    editorController = undefined;
    if (!controller) {
        return;
    }
    try {
        await controller.releaseCurrent();
    }
    catch {
        // Best effort. The daemon also expires and reaps stale window ownership.
    }
    finally {
        controller.dispose();
    }
}
function registerSemanticActivity(context, controller) {
    const ensureAfterActivity = (reason) => {
        void controller
            .ensureCurrent(reason)
            .then((result) => vscode.commands.executeCommand("setContext", "locald:projectDetected", result !== undefined))
            .catch((error) => {
            exports.log.warn(`Editor activity could not ensure locald: ${formatError(error)}`);
        });
    };
    context.subscriptions.push(vscode.window.onDidChangeWindowState((state) => {
        if (state.focused) {
            ensureAfterActivity("window refocus");
        }
    }), vscode.window.onDidChangeActiveTextEditor(() => {
        if (vscode.window.state.focused) {
            ensureAfterActivity("active editor change");
        }
    }), vscode.workspace.onDidChangeWorkspaceFolders(() => {
        if (vscode.window.state.focused) {
            ensureAfterActivity("workspace folder change");
        }
    }));
}
function registerCommands(context, controller) {
    const openInBrowser = (url, reuseFilter) => {
        void vscode.commands.executeCommand("workbench.action.browser.open", {
            url,
            reuseUrlFilter: reuseFilter,
        });
    };
    context.subscriptions.push(vscode.commands.registerCommand("locald.openDashboard", async () => {
        try {
            const projectPath = await resolveRequiredProjectPath();
            const info = await (0, plumbing_js_1.status)(projectPath);
            const url = info.project_name
                ? `https://locald.localhost/?project=${encodeURIComponent(info.project_name)}`
                : "https://locald.localhost";
            openInBrowser(url, "https://locald.localhost/**");
        }
        catch (error) {
            vscode.window.showErrorMessage(`locald: dashboard unavailable — ${formatError(error)}`);
        }
    }), vscode.commands.registerCommand("locald.openWebService", async () => {
        try {
            const result = await controller.ensureCurrent("open web service");
            const webServices = result?.services.filter((service) => service.url) ?? [];
            if (webServices.length === 0) {
                vscode.window.showInformationMessage("locald: this project has no web service URL");
                return;
            }
            if (webServices.length === 1) {
                const service = webServices[0];
                if (service.url) {
                    openInBrowser(service.url, `${service.url}/**`);
                }
                return;
            }
            const picked = await vscode.window.showQuickPick(webServices.map((service) => ({
                label: service.name.split(":").pop() ?? service.name,
                description: service.url,
            })), { placeHolder: "Open web service" });
            if (picked?.description) {
                openInBrowser(picked.description, `${picked.description}/**`);
            }
        }
        catch (error) {
            vscode.window.showErrorMessage(`locald: web service unavailable — ${formatError(error)}`);
        }
    }), vscode.commands.registerCommand("locald.restartServices", async () => {
        try {
            const restarted = await controller.withCurrentProject("prepare services for restart", async (initial, ensureTarget) => {
                await (0, plumbing_js_1.stopProject)(initial.project_path);
                return ensureTarget("wait for restarted services");
            });
            if (!restarted) {
                throw new Error("no locald project is selected");
            }
            vscode.window.showInformationMessage("locald: services restarted");
        }
        catch (error) {
            vscode.window.showErrorMessage(`locald: restart failed — ${formatError(error)}`);
        }
    }), vscode.commands.registerCommand("locald.stopServices", async () => {
        try {
            const projectPath = await resolveRequiredProjectPath();
            await (0, plumbing_js_1.stopProject)(projectPath);
            vscode.window.showInformationMessage("locald: project paused");
        }
        catch (error) {
            vscode.window.showErrorMessage(`locald: pause failed — ${formatError(error)}`);
        }
    }));
}
async function resolveRequiredProjectPath() {
    const projectPath = await (0, workspace_project_js_1.resolveCurrentProjectPath)();
    if (!projectPath) {
        throw new Error("no locald.toml found in the current workspace");
    }
    return projectPath;
}
function formatError(error) {
    return error instanceof Error ? error.message : String(error);
}
//# sourceMappingURL=extension.js.map