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
exports.registerTools = registerTools;
const vscode = __importStar(require("vscode"));
const plumbing_js_1 = require("./plumbing.js");
const extension_js_1 = require("./extension.js");
function registerTools(context, controller, resolveProject) {
    if (!vscode.lm || typeof vscode.lm.registerTool !== "function") {
        extension_js_1.log.warn("vscode.lm.registerTool not available — tools disabled");
        return;
    }
    extension_js_1.log.info("Registering 5 locald language-model tools");
    context.subscriptions.push(vscode.lm.registerTool("locald_ensure", {
        async invoke(_options, _token) {
            const result = await controller.ensureCurrent("language-model readiness request");
            if (!result) {
                throw new Error("no locald project is selected");
            }
            return textResult(JSON.stringify({
                state: result.state,
                services: result.services,
                urls: result.urls,
            }, null, 2));
        },
    }), vscode.lm.registerTool("locald_services", {
        async invoke(_options, _token) {
            const projectPath = await resolveRequiredProject(resolveProject);
            const info = await (0, plumbing_js_1.status)(projectPath);
            const services = info.service_details.map((service) => ({
                name: service.name,
                status: service.status,
                health_status: service.health_status,
                url: service.url,
                domain: service.domain,
                service_type: service.service_type,
            }));
            return textResult(JSON.stringify(services, null, 2));
        },
    }), vscode.lm.registerTool("locald_restart", {
        async invoke(options, _token) {
            const result = await controller.withCurrentProject("language-model restart request", async (initial, ensureTarget) => {
                if (options.input?.service) {
                    const service = initial.services.find((candidate) => candidate.name === options.input?.service ||
                        candidate.name.endsWith(`:${options.input?.service}`));
                    if (!service) {
                        throw new Error(`service ${options.input.service} was not found`);
                    }
                    await (0, plumbing_js_1.restartService)(initial.project_path, service.name);
                }
                else {
                    await (0, plumbing_js_1.stopProject)(initial.project_path);
                }
                return ensureTarget("wait for language-model service restart");
            });
            if (!result) {
                throw new Error("no locald project is selected");
            }
            return textResult(`Services restarted and ready.${result.urls.length > 0 ? ` ${result.urls.join(" ")}` : ""}`);
        },
    }), vscode.lm.registerTool("locald_logs", {
        async invoke(options, _token) {
            const projectPath = await resolveRequiredProject(resolveProject);
            const output = await (0, plumbing_js_1.getLogs)(projectPath, options.input?.lines ?? 200, options.input?.service);
            return textResult(output);
        },
    }), vscode.lm.registerTool("locald_open", {
        async invoke(options, _token) {
            const result = await controller.ensureCurrent("language-model browser request");
            if (!result) {
                throw new Error("no locald project is selected");
            }
            const service = options.input?.service
                ? result.services.find((candidate) => candidate.name === options.input?.service ||
                    candidate.name.endsWith(`:${options.input?.service}`))
                : result.services.find((candidate) => candidate.url);
            if (!service?.url) {
                return textResult("No service URL available.");
            }
            await vscode.commands.executeCommand("simpleBrowser.show", vscode.Uri.parse(service.url));
            return textResult(`Opened ${service.url} in Simple Browser.`);
        },
    }));
}
async function resolveRequiredProject(resolveProject) {
    const projectPath = await resolveProject();
    if (!projectPath) {
        throw new Error("no locald.toml found in the current workspace");
    }
    return projectPath;
}
function textResult(text) {
    return new vscode.LanguageModelToolResult([
        new vscode.LanguageModelTextPart(text),
    ]);
}
//# sourceMappingURL=tools.js.map