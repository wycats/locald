"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.EditorAvailabilityController = exports.EDITOR_RENEWAL_INTERVAL_MS = void 0;
exports.EDITOR_RENEWAL_INTERVAL_MS = 30_000;
class EditorAvailabilityController {
    windowId;
    hostPid;
    resolveProject;
    client;
    log;
    renewalIntervalMs;
    scheduleRenewal;
    currentPath;
    cancelHeartbeat;
    pending = Promise.resolve();
    disposed = false;
    constructor(options) {
        this.windowId = options.windowId;
        this.hostPid = options.hostPid;
        this.resolveProject = options.resolveProject;
        this.client = options.client;
        this.log = options.log;
        this.renewalIntervalMs =
            options.renewalIntervalMs ?? exports.EDITOR_RENEWAL_INTERVAL_MS;
        this.scheduleRenewal =
            options.scheduleRenewal ??
                ((renew, intervalMs) => {
                    const timer = setInterval(renew, intervalMs);
                    return () => clearInterval(timer);
                });
    }
    get projectPath() {
        return this.currentPath;
    }
    async activate() {
        this.startHeartbeat();
        return this.ensureCurrent("activation");
    }
    async ensureCurrent(reason) {
        return this.enqueue(() => this.ensureCurrentWithinQueue(reason));
    }
    async withCurrentProject(reason, operation) {
        return this.enqueue(async () => {
            const initial = await this.ensureCurrentWithinQueue(reason);
            if (!initial) {
                return undefined;
            }
            return operation(initial, (nextReason) => this.ensureProjectWithinQueue(initial.project_path, nextReason));
        });
    }
    async renewCurrent() {
        await this.enqueue(async () => {
            if (this.disposed || !this.currentPath) {
                return;
            }
            const projectPath = this.currentPath;
            try {
                await this.client.renew(projectPath, this.windowId, this.hostPid);
            }
            catch (error) {
                this.log.warn(`Failed to renew editor demand for ${projectPath}; the next semantic activity will ensure it again: ${formatError(error)}`);
            }
        });
    }
    async releaseCurrent() {
        await this.enqueue(async () => {
            await this.releaseCurrentWithinQueue();
        });
    }
    dispose() {
        this.disposed = true;
        this.cancelHeartbeat?.();
        this.cancelHeartbeat = undefined;
    }
    startHeartbeat() {
        if (this.cancelHeartbeat !== undefined) {
            return;
        }
        this.cancelHeartbeat = this.scheduleRenewal(() => {
            void this.renewCurrent();
        }, this.renewalIntervalMs);
    }
    async releaseCurrentWithinQueue() {
        const projectPath = this.currentPath;
        this.currentPath = undefined;
        if (!projectPath) {
            return;
        }
        await this.client.release(projectPath, this.windowId, this.hostPid);
        this.log.info(`Released editor availability for ${projectPath}`);
    }
    async ensureCurrentWithinQueue(reason) {
        this.ensureNotDisposed();
        const nextPath = await this.resolveProject();
        if (!nextPath) {
            await this.releaseCurrentWithinQueue();
            return undefined;
        }
        const previousPath = this.currentPath;
        this.currentPath = nextPath;
        let result;
        try {
            result = await this.ensureProjectWithinQueue(nextPath, reason);
        }
        finally {
            if (previousPath && previousPath !== nextPath) {
                try {
                    await this.client.release(previousPath, this.windowId, this.hostPid);
                }
                catch (error) {
                    this.log.warn(`Failed to release previous editor demand for ${previousPath}; it will expire automatically: ${formatError(error)}`);
                }
            }
        }
        return result;
    }
    async ensureProjectWithinQueue(projectPath, reason) {
        const result = await this.client.ensure(projectPath, this.windowId, this.hostPid);
        this.log.info(`Editor availability ready for ${projectPath} after ${reason}`);
        return result;
    }
    ensureNotDisposed() {
        if (this.disposed) {
            throw new Error("editor availability controller is disposed");
        }
    }
    enqueue(operation) {
        const result = this.pending.then(operation, operation);
        this.pending = result.then(() => undefined, () => undefined);
        return result;
    }
}
exports.EditorAvailabilityController = EditorAvailabilityController;
function formatError(error) {
    return error instanceof Error ? error.message : String(error);
}
//# sourceMappingURL=editor-controller.js.map