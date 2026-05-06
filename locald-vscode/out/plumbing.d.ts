export interface ServiceStatus {
    name: string;
    url: string | null;
    port: number | null;
    status: string;
    health_status: string;
    domain: string | null;
    service_type: string;
}
export interface ProjectStatusInfo {
    project_path: string;
    project_name: string | null;
    services: string[];
    service_details: ServiceStatus[];
    attachments: unknown[];
    is_running: boolean;
}
export type LocaldBinarySource = "LOCALD_BINARY" | "cargo" | "PATH";
export interface LocaldBinaryIdentity {
    path: string;
    source: LocaldBinarySource;
}
export declare function getBinaryIdentity(): LocaldBinaryIdentity;
export declare function formatBinaryIdentity(identity?: LocaldBinaryIdentity): string;
export declare function findBinary(): string;
export declare function attach(projectPath: string, windowId: string): Promise<void>;
export declare function detach(projectPath: string, windowId: string): Promise<void>;
export declare function status(projectPath: string): Promise<ProjectStatusInfo>;
export declare function listProjects(): Promise<ProjectStatusInfo[]>;
export declare function startProject(projectPath: string): Promise<void>;
export declare function getLogs(lines?: number): Promise<string>;
