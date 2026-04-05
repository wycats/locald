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
exports.StatusBar = void 0;
const vscode = __importStar(require("vscode"));
const plumbing_js_1 = require("./plumbing.js");
const POLL_INTERVAL = 5_000;
class StatusBar {
    item;
    timer;
    projectPath;
    constructor(projectPath) {
        this.projectPath = projectPath;
        this.item = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 50);
        this.item.command = "locald.openDashboard";
        this.item.text = "$(server) locald";
        this.item.tooltip = "locald — loading…";
        this.item.show();
    }
    start() {
        this.refresh();
        this.timer = setInterval(() => this.refresh(), POLL_INTERVAL);
    }
    async refresh() {
        try {
            const info = await (0, plumbing_js_1.status)(this.projectPath);
            this.update(info);
        }
        catch {
            this.item.text = "$(error) locald";
            this.item.tooltip = "locald — unable to reach daemon";
        }
    }
    update(info) {
        const name = info.project_name ?? "locald";
        const services = info.service_details ?? [];
        const total = services.length || info.services.length;
        const healthy = services.filter((s) => s.health_status === "Healthy").length;
        if (total === 0) {
            this.item.text = `$(server) ${name}`;
            this.item.tooltip = `${name} — no services`;
        }
        else if (healthy === total) {
            this.item.text = `$(server) ${name} · ${total} service${total !== 1 ? "s" : ""}`;
            this.item.tooltip = this.buildTooltip(name, services, info);
        }
        else {
            this.item.text = `$(warning) ${name} · ${healthy}/${total}`;
            this.item.tooltip = this.buildTooltip(name, services, info);
        }
    }
    buildTooltip(name, services, info) {
        const lines = [`${name} — editor attached`];
        lines.push("Services stop when this window closes.");
        lines.push("");
        for (const s of services) {
            const icon = s.status === "running" ? "●" : "○";
            const url = s.domain ? `  ${s.domain}` : "";
            lines.push(`${icon} ${s.name}${url}`);
        }
        if (services.length === 0) {
            for (const n of info.services) {
                lines.push(`● ${n}`);
            }
        }
        return lines.join("\n");
    }
    dispose() {
        if (this.timer !== undefined) {
            clearInterval(this.timer);
            this.timer = undefined;
        }
        this.item.dispose();
    }
}
exports.StatusBar = StatusBar;
//# sourceMappingURL=status-bar.js.map