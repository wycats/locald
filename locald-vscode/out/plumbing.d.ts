export interface ServiceStatus {
    name: string;
    url: string | null;
    port: number | null;
    healthy: boolean;
    status: string;
}
export interface ProjectStatusInfo {
    path: string;
    name: string;
    services: string[];
    service_details: ServiceStatus[];
    attachments: unknown[];
}
export declare function attach(projectPath: string, windowId: string): Promise<void>;
export declare function detach(projectPath: string, windowId: string): Promise<void>;
export declare function status(projectPath: string): Promise<ProjectStatusInfo>;
export declare function listProjects(): Promise<ProjectStatusInfo[]>;
export declare function startProject(projectPath: string): Promise<void>;
export declare function getLogs(lines?: number): Promise<string>;
