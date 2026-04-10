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
    dashboardItem;
    webItem;
    timer;
    projectPath;
    windowId;
    webServices = [];
    wasUnreachable = false;
    constructor(projectPath, windowId) {
        this.projectPath = projectPath;
        this.windowId = windowId;
        // Dashboard item (left)
        this.dashboardItem = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 51);
        this.dashboardItem.command = "locald.openDashboard";
        this.dashboardItem.text = "$(server) locald";
        this.dashboardItem.tooltip = "locald — loading…";
        this.dashboardItem.show();
        // Web service item (right of dashboard)
        this.webItem = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 50);
        this.webItem.command = "locald.openWebService";
    }
    start() {
        this.refresh();
        this.timer = setInterval(() => this.refresh(), POLL_INTERVAL);
    }
    getWebServices() {
        return this.webServices;
    }
    async refresh() {
        try {
            const info = await (0, plumbing_js_1.status)(this.projectPath);
            if (this.wasUnreachable) {
                this.wasUnreachable = false;
                (0, plumbing_js_1.attach)(this.projectPath, this.windowId).catch(() => { });
            }
            this.updateDashboard(info);
            this.updateWebItem(info);
        }
        catch {
            this.wasUnreachable = true;
            this.dashboardItem.text = "$(error) locald";
            this.dashboardItem.tooltip = "locald — unable to reach daemon";
            this.webItem.hide();
        }
    }
    updateDashboard(info) {
        const name = info.project_name ?? "locald";
        const services = info.service_details ?? [];
        const total = services.length || info.services.length;
        const healthy = services.filter((s) => s.health_status === "Healthy").length;
        if (total === 0) {
            this.dashboardItem.text = `$(server) ${name}`;
            this.dashboardItem.tooltip = `${name} — no services`;
        }
        else if (healthy === total) {
            this.dashboardItem.text = `$(server) ${name} · ${total} service${total !== 1 ? "s" : ""}`;
            this.dashboardItem.tooltip = this.buildTooltip(name, services, info);
        }
        else {
            this.dashboardItem.text = `$(warning) ${name} · ${healthy}/${total}`;
            this.dashboardItem.tooltip = this.buildTooltip(name, services, info);
        }
    }
    updateWebItem(info) {
        const services = info.service_details ?? [];
        this.webServices = services.filter((s) => s.url && s.status === "running");
        if (this.webServices.length === 0) {
            this.webItem.hide();
            return;
        }
        if (this.webServices.length === 1) {
            const svc = this.webServices[0];
            const label = svc.name.split(":").pop() ?? svc.name;
            this.webItem.text = `$(globe) ${label}`;
            this.webItem.tooltip = svc.domain ?? svc.url ?? label;
        }
        else {
            // Multiple web services — show count, click for picker
            this.webItem.text = `$(globe) ${this.webServices.length} sites`;
            this.webItem.tooltip = this.webServices
                .map((s) => s.name.split(":").pop() ?? s.name)
                .join(", ");
        }
        this.webItem.show();
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
        this.dashboardItem.dispose();
        this.webItem.dispose();
    }
}
exports.StatusBar = StatusBar;
//# sourceMappingURL=status-bar.js.map