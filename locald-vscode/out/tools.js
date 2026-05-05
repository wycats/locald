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
function registerTools(context, projectPath) {
    if (!vscode.lm || typeof vscode.lm.registerTool !== "function") {
        extension_js_1.log.warn("vscode.lm.registerTool not available — tools disabled");
        return;
    }
    extension_js_1.log.info("Registering 4 Copilot tools");
    context.subscriptions.push(vscode.lm.registerTool("locald_services", {
        async invoke(_options, _token) {
            const info = await (0, plumbing_js_1.status)(projectPath);
            const text = JSON.stringify(info.service_details, null, 2);
            return new vscode.LanguageModelToolResult([
                new vscode.LanguageModelTextPart(text),
            ]);
        },
    }));
    context.subscriptions.push(vscode.lm.registerTool("locald_restart", {
        async invoke(options, _token) {
            void options.input?.service;
            await (0, plumbing_js_1.startProject)(projectPath);
            return new vscode.LanguageModelToolResult([
                new vscode.LanguageModelTextPart("Services restarted."),
            ]);
        },
    }));
    context.subscriptions.push(vscode.lm.registerTool("locald_logs", {
        async invoke(options, _token) {
            void options.input?.service;
            const output = await (0, plumbing_js_1.getLogs)(options.input?.lines ?? 200);
            return new vscode.LanguageModelToolResult([
                new vscode.LanguageModelTextPart(output),
            ]);
        },
    }));
    context.subscriptions.push(vscode.lm.registerTool("locald_open", {
        async invoke(options, _token) {
            const info = await (0, plumbing_js_1.status)(projectPath);
            const service = options.input?.service
                ? info.service_details.find((s) => s.name === options.input?.service)
                : info.service_details.find((s) => s.url);
            if (!service?.url) {
                return new vscode.LanguageModelToolResult([
                    new vscode.LanguageModelTextPart("No service URL available."),
                ]);
            }
            await vscode.commands.executeCommand("simpleBrowser.show", vscode.Uri.parse(service.url));
            return new vscode.LanguageModelToolResult([
                new vscode.LanguageModelTextPart(`Opened ${service.url} in Simple Browser.`),
            ]);
        },
    }));
}
//# sourceMappingURL=tools.js.map