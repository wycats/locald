"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.AmbiguousProjectError = void 0;
exports.selectProjectPath = selectProjectPath;
const node_path_1 = require("node:path");
class AmbiguousProjectError extends Error {
    constructor(projectPaths) {
        super(`multiple locald projects match this VS Code window; focus a file inside one project (${projectPaths.join(", ")})`);
        this.name = "AmbiguousProjectError";
    }
}
exports.AmbiguousProjectError = AmbiguousProjectError;
function selectProjectPath(configPaths, activeFilePath) {
    const projectPaths = [...new Set(configPaths.map((path) => (0, node_path_1.dirname)(path)))];
    if (projectPaths.length === 0) {
        return undefined;
    }
    if (activeFilePath) {
        const matching = projectPaths
            .filter((path) => isPathWithin(path, activeFilePath))
            .sort((left, right) => right.length - left.length);
        if (matching.length > 0) {
            return matching[0];
        }
    }
    if (projectPaths.length === 1) {
        return projectPaths[0];
    }
    throw new AmbiguousProjectError(projectPaths.sort());
}
function isPathWithin(root, candidate) {
    const path = (0, node_path_1.relative)(root, candidate);
    return path === "" || (!path.startsWith("..") && !path.startsWith("/"));
}
//# sourceMappingURL=project-selection.js.map