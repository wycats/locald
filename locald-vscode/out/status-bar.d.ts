import * as vscode from "vscode";
import { type ServiceStatus } from "./plumbing.js";
export declare class StatusBar implements vscode.Disposable {
    private dashboardItem;
    private webItem;
    private timer;
    private projectPath;
    private windowId;
    private webServices;
    private wasUnreachable;
    constructor(projectPath: string, windowId: string);
    start(): void;
    getWebServices(): ServiceStatus[];
    private refresh;
    private updateDashboard;
    private updateWebItem;
    private buildTooltip;
    dispose(): void;
}
