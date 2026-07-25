export declare const LOCALD_CONFIG_GLOB = "**/locald.toml";
export declare const LOCALD_CONFIG_EXCLUDE = "**/{.git,node_modules,target,dist,build,.next}/**";
export interface ProjectConfigUri {
    fsPath: string;
}
export type ProjectConfigFinder = (include: string, exclude: string) => PromiseLike<readonly ProjectConfigUri[]>;
export declare function findProjectConfigPaths(findFiles: ProjectConfigFinder): Promise<string[]>;
