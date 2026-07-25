"use strict";
var __createBinding = (this && this.__createBinding) || (Object.create ? (function(o, m, k, k2) {
    if (k2 === undefined) k2 = k;
    var desc = Object.getOwnPropertyDescriptor(m, k);
    if (!desc || ("get" in desc ? !m.__esModule : desc.writable || desc.configurable)) {
      desc = { enumerable: true, get: function() { return m[k]; } };
    }
    Object.defineProperty(o, k2, desc);
}) : (function(o, m, k, k2) {
    if (k2 === undefined) k2 = k;
    o[k2] = m[k];
}));
var __setModuleDefault = (this && this.__setModuleDefault) || (Object.create ? (function(o, v) {
    Object.defineProperty(o, "default", { enumerable: true, value: v });
}) : function(o, v) {
    o["default"] = v;
});
var __importStar = (this && this.__importStar) || (function () {
    var ownKeys = function(o) {
        ownKeys = Object.getOwnPropertyNames || function (o) {
            var ar = [];
            for (var k in o) if (Object.prototype.hasOwnProperty.call(o, k)) ar[ar.length] = k;
            return ar;
        };
        return ownKeys(o);
    };
    return function (mod) {
        if (mod && mod.__esModule) return mod;
        var result = {};
        if (mod != null) for (var k = ownKeys(mod), i = 0; i < k.length; i++) if (k[i] !== "default") __createBinding(result, mod, k[i]);
        __setModuleDefault(result, mod);
        return result;
    };
})();
Object.defineProperty(exports, "__esModule", { value: true });
exports.selectProjectPath = exports.AmbiguousProjectError = void 0;
exports.resolveCurrentProjectPath = resolveCurrentProjectPath;
const vscode = __importStar(require("vscode"));
const project_selection_js_1 = require("./project-selection.js");
Object.defineProperty(exports, "AmbiguousProjectError", { enumerable: true, get: function () { return project_selection_js_1.AmbiguousProjectError; } });
Object.defineProperty(exports, "selectProjectPath", { enumerable: true, get: function () { return project_selection_js_1.selectProjectPath; } });
const project_discovery_js_1 = require("./project-discovery.js");
async function resolveCurrentProjectPath() {
    const configPaths = await (0, project_discovery_js_1.findProjectConfigPaths)((include, exclude) => vscode.workspace.findFiles(include, exclude));
    const activeDocument = vscode.window.activeTextEditor?.document;
    const activeFilePath = activeDocument?.uri.scheme === "file"
        ? activeDocument.uri.fsPath
        : undefined;
    return (0, project_selection_js_1.selectProjectPath)(configPaths, activeFilePath);
}
//# sourceMappingURL=workspace-project.js.map