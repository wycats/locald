import * as vscode from "vscode";
import { type ServiceStatus } from "./plumbing.js";
export declare class StatusBar implements vscode.Disposable {
    private dashboardItem;
    private webItem;
    private timer;
    private readonly getProjectPath;
    private readonly recoverEditorDemand;
    private log;
    private webServices;
    private wasUnreachable;
    private consecutiveFailures;
    constructor(getProjectPath: () => string | undefined, log: vscode.LogOutputChannel, recoverEditorDemand: (paused: boolean) => void);
    start(): void;
    getWebServices(): ServiceStatus[];
    private refresh;
    private updateDashboard;
    private updateWebItem;
    private buildTooltip;
    private withBinaryInfo;
    dispose(): void;
}
