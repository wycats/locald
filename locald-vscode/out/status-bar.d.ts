import * as vscode from "vscode";
export declare class StatusBar implements vscode.Disposable {
    private item;
    private timer;
    private projectPath;
    constructor(projectPath: string);
    start(): void;
    private refresh;
    private update;
    dispose(): void;
}
