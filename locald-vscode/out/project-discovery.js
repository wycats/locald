"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.LOCALD_CONFIG_EXCLUDE = exports.LOCALD_CONFIG_GLOB = void 0;
exports.findProjectConfigPaths = findProjectConfigPaths;
exports.LOCALD_CONFIG_GLOB = "**/locald.toml";
exports.LOCALD_CONFIG_EXCLUDE = "**/{.git,node_modules,target,dist,build,.next}/**";
async function findProjectConfigPaths(findFiles) {
    const configs = await findFiles(exports.LOCALD_CONFIG_GLOB, exports.LOCALD_CONFIG_EXCLUDE);
    return configs.map((config) => config.fsPath);
}
//# sourceMappingURL=project-discovery.js.map