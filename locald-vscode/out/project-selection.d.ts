export declare class AmbiguousProjectError extends Error {
    constructor(projectPaths: string[]);
}
export declare function selectProjectPath(configPaths: string[], activeFilePath?: string): string | undefined;
