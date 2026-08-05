export interface ServiceStatus {
    name: string;
    url: string | null;
    port: number | null;
    status: string;
    health_status: string;
    domain: string | null;
    service_type: string;
    publication?: PublicationStatus;
}
export interface PublicationStatus {
    state: "waiting_for_publisher" | "checking_endpoint" | "endpoint_unhealthy" | "ready" | "route_paused" | "instance_missing";
    origin: string;
    explanation: string;
    next_step?: string;
}
export interface ProjectStatusInfo {
    project_path: string;
    project_name: string | null;
    services: string[];
    service_details: ServiceStatus[];
    attachments: unknown[];
    is_running: boolean;
    availability?: {
        desired: boolean;
        state: string;
        always_on: boolean;
        paused: boolean;
        reasons: unknown[];
        demands: Array<{
            kind: string;
            safe_label: string;
            expires_at?: unknown;
        }>;
        next_transition_at?: unknown;
        last_error?: string;
    };
}
export interface EnsuredServiceStatus {
    name: string;
    service_type: string;
    status: string;
    health_status: string;
    url?: string;
    publication?: PublicationStatus;
}
export interface EnsureProjectResult {
    project_path: string;
    project_name?: string;
    state: "ready";
    services: EnsuredServiceStatus[];
    urls: string[];
}
export type LocaldBinarySource = "LOCALD_BINARY" | "local install" | "cargo fallback" | "PATH";
export interface LocaldBinaryIdentity {
    path: string;
    source: LocaldBinarySource;
}
export declare function getBinaryIdentity(): LocaldBinaryIdentity;
export declare function formatBinaryIdentity(identity?: LocaldBinaryIdentity): string;
export declare function findBinary(): string;
export declare function resolveBinaryIdentityFrom(options: {
    configuredPath?: string;
    homeDirectory: string;
    path?: string;
    exists(path: string): boolean;
}): LocaldBinaryIdentity;
export declare function formatCommandFailure(binary: LocaldBinaryIdentity, args: string[], detail: string): string;
export declare function ensureEditorProject(projectPath: string, windowId: string, hostPid: number): Promise<EnsureProjectResult>;
export declare function renewEditorProject(projectPath: string, windowId: string, hostPid: number): Promise<void>;
export declare function releaseEditorProject(projectPath: string, windowId: string, hostPid: number): Promise<void>;
export declare function status(projectPath: string): Promise<ProjectStatusInfo>;
export declare function listProjects(): Promise<ProjectStatusInfo[]>;
export declare function stopProject(projectPath: string): Promise<void>;
export declare function restartService(projectPath: string, serviceName: string): Promise<void>;
export declare function restartCommandArgs(serviceName: string): string[];
export declare function restartProject(projectPath: string): Promise<void>;
export declare function restartProjectCommandArgs(projectPath: string): string[];
export declare function getLogs(projectPath: string, lines?: number, service?: string): Promise<string>;
export declare function logCommandArgs(service?: string): string[];
export declare function parseJsonOutput<T>(output: string): T;
export declare function tailLines(output: string, lines: number): string;
export declare class StreamingLineTail {
    private output;
    private readonly lines;
    private readonly maxCharacters;
    constructor(lines: number, maxCharacters?: number);
    push(chunk: string): void;
    value(): string;
}
