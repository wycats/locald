import type { EnsureProjectResult } from "./plumbing.js";
export declare const EDITOR_RENEWAL_INTERVAL_MS = 30000;
export interface EditorLifecycleClient {
    ensure(projectPath: string, windowId: string, hostPid: number): Promise<EnsureProjectResult>;
    renew(projectPath: string, windowId: string, hostPid: number): Promise<void>;
    release(projectPath: string, windowId: string, hostPid: number): Promise<void>;
}
export interface ControllerLog {
    info(message: string): void;
    warn(message: string): void;
}
export type ProjectResolver = () => Promise<string | undefined>;
export type RenewalScheduler = (renew: () => void, intervalMs: number) => () => void;
export type CurrentProjectOperation<T> = (initial: EnsureProjectResult, ensureTarget: (reason: string) => Promise<EnsureProjectResult>) => Promise<T>;
export declare class EditorAvailabilityController {
    private readonly windowId;
    private readonly hostPid;
    private readonly resolveProject;
    private readonly client;
    private readonly log;
    private readonly renewalIntervalMs;
    private readonly scheduleRenewal;
    private currentPath;
    private cancelHeartbeat;
    private pending;
    private disposed;
    constructor(options: {
        windowId: string;
        hostPid: number;
        resolveProject: ProjectResolver;
        client: EditorLifecycleClient;
        log: ControllerLog;
        renewalIntervalMs?: number;
        scheduleRenewal?: RenewalScheduler;
    });
    get projectPath(): string | undefined;
    activate(): Promise<EnsureProjectResult | undefined>;
    ensureCurrent(reason: string): Promise<EnsureProjectResult | undefined>;
    withCurrentProject<T>(reason: string, operation: CurrentProjectOperation<T>): Promise<T | undefined>;
    renewCurrent(): Promise<void>;
    releaseCurrent(): Promise<void>;
    dispose(): void;
    private startHeartbeat;
    private releaseCurrentWithinQueue;
    private ensureCurrentWithinQueue;
    private ensureProjectWithinQueue;
    private ensureNotDisposed;
    private enqueue;
}
