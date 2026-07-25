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
    operationRenewalPath;
    uncertainRenewalPaths = new Set();
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
            this.operationRenewalPath = initial.project_path;
            try {
                return await operation(initial, (nextReason) => this.ensureProjectWithinQueue(initial.project_path, nextReason));
            }
            finally {
                this.operationRenewalPath = undefined;
            }
        });
    }
    async renewCurrent() {
        const directPaths = this.directRenewalPaths();
        if (!this.disposed && directPaths.length > 0) {
            await Promise.all(directPaths.map((projectPath) => this.renewProject(projectPath)));
            return;
        }
        await this.enqueue(async () => {
            if (this.disposed || !this.currentPath) {
                return;
            }
            await this.renewProject(this.currentPath);
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
        const projectPaths = new Set(this.uncertainRenewalPaths);
        if (this.currentPath) {
            projectPaths.add(this.currentPath);
        }
        this.currentPath = undefined;
        this.uncertainRenewalPaths.clear();
        let firstError;
        for (const projectPath of projectPaths) {
            try {
                await this.client.release(projectPath, this.windowId, this.hostPid);
                this.log.info(`Released editor availability for ${projectPath}`);
            }
            catch (error) {
                firstError ??= error;
                this.log.warn(`Failed to release editor demand for ${projectPath}; it will expire automatically: ${formatError(error)}`);
            }
        }
        if (firstError) {
            throw firstError;
        }
    }
    async ensureCurrentWithinQueue(reason) {
        this.ensureNotDisposed();
        const nextPath = await this.resolveProject();
        if (!nextPath) {
            await this.releaseCurrentWithinQueue();
            return undefined;
        }
        const previousPath = this.currentPath;
        this.uncertainRenewalPaths.add(nextPath);
        const result = await this.ensureProjectWithinQueue(nextPath, reason);
        this.uncertainRenewalPaths.delete(nextPath);
        this.currentPath = nextPath;
        for (const uncertainPath of [...this.uncertainRenewalPaths]) {
            if (uncertainPath === previousPath) {
                continue;
            }
            try {
                await this.client.release(uncertainPath, this.windowId, this.hostPid);
                this.uncertainRenewalPaths.delete(uncertainPath);
                this.log.info(`Released uncertain editor availability for ${uncertainPath}`);
            }
            catch (error) {
                this.log.warn(`Failed to release uncertain editor demand for ${uncertainPath}; renewal will preserve it for cleanup retry: ${formatError(error)}`);
            }
        }
        if (previousPath && previousPath !== nextPath) {
            this.uncertainRenewalPaths.delete(previousPath);
            try {
                await this.client.release(previousPath, this.windowId, this.hostPid);
            }
            catch (error) {
                this.log.warn(`Failed to release previous editor demand for ${previousPath}; it will expire automatically: ${formatError(error)}`);
            }
        }
        return result;
    }
    directRenewalPaths() {
        const projectPaths = new Set(this.uncertainRenewalPaths);
        if (this.operationRenewalPath) {
            projectPaths.add(this.operationRenewalPath);
        }
        if (projectPaths.size > 0 && this.currentPath) {
            projectPaths.add(this.currentPath);
        }
        return [...projectPaths];
    }
    async ensureProjectWithinQueue(projectPath, reason) {
        const result = await this.client.ensure(projectPath, this.windowId, this.hostPid);
        this.log.info(`Editor availability ready for ${projectPath} after ${reason}`);
        return result;
    }
    async renewProject(projectPath) {
        try {
            await this.client.renew(projectPath, this.windowId, this.hostPid);
        }
        catch (error) {
            this.log.warn(`Failed to renew editor demand for ${projectPath}; the next semantic activity will ensure it again: ${formatError(error)}`);
        }
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