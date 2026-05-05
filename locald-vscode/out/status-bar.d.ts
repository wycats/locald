import * as vscode from "vscode";
import { type ServiceStatus } from "./plumbing.js";
export declare class StatusBar implements vscode.Disposable {
    private dashboardItem;
    private webItem;
    private timer;
    private projectPath;
    private windowId;
    private log;
    private webServices;
    private wasUnreachable;
    private consecutiveFailures;
    constructor(projectPath: string, windowId: string, log: vscode.LogOutputChannel);
    start(): void;
    getWebServices(): ServiceStatus[];
    private refresh;
    private updateDashboard;
    private updateWebItem;
    private buildTooltip;
    dispose(): void;
}
