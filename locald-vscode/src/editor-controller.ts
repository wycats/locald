import type { EnsureProjectResult } from "./plumbing.js";

export const EDITOR_RENEWAL_INTERVAL_MS = 30_000;

export interface EditorLifecycleClient {
  ensure(
    projectPath: string,
    windowId: string,
    hostPid: number,
  ): Promise<EnsureProjectResult>;
  renew(
    projectPath: string,
    windowId: string,
    hostPid: number,
  ): Promise<void>;
  release(
    projectPath: string,
    windowId: string,
    hostPid: number,
  ): Promise<void>;
}

export interface ControllerLog {
  info(message: string): void;
  warn(message: string): void;
}

export type ProjectResolver = () => Promise<string | undefined>;
export type RenewalScheduler = (
  renew: () => void,
  intervalMs: number,
) => () => void;
export type CurrentProjectOperation<T> = (
  initial: EnsureProjectResult,
  ensureTarget: (reason: string) => Promise<EnsureProjectResult>,
) => Promise<T>;

export class EditorAvailabilityController {
  private readonly windowId: string;
  private readonly hostPid: number;
  private readonly resolveProject: ProjectResolver;
  private readonly client: EditorLifecycleClient;
  private readonly log: ControllerLog;
  private readonly renewalIntervalMs: number;
  private readonly scheduleRenewal: RenewalScheduler;
  private currentPath: string | undefined;
  private operationRenewalPath: string | undefined;
  private readonly uncertainRenewalPaths = new Set<string>();
  private cancelHeartbeat: (() => void) | undefined;
  private pending: Promise<void> = Promise.resolve();
  private disposed = false;

  constructor(options: {
    windowId: string;
    hostPid: number;
    resolveProject: ProjectResolver;
    client: EditorLifecycleClient;
    log: ControllerLog;
    renewalIntervalMs?: number;
    scheduleRenewal?: RenewalScheduler;
  }) {
    this.windowId = options.windowId;
    this.hostPid = options.hostPid;
    this.resolveProject = options.resolveProject;
    this.client = options.client;
    this.log = options.log;
    this.renewalIntervalMs =
      options.renewalIntervalMs ?? EDITOR_RENEWAL_INTERVAL_MS;
    this.scheduleRenewal =
      options.scheduleRenewal ??
      ((renew, intervalMs) => {
        const timer = setInterval(renew, intervalMs);
        return () => clearInterval(timer);
      });
  }

  get projectPath(): string | undefined {
    return this.currentPath;
  }

  async activate(): Promise<EnsureProjectResult | undefined> {
    this.startHeartbeat();
    return this.ensureCurrent("activation");
  }

  async ensureCurrent(reason: string): Promise<EnsureProjectResult | undefined> {
    return this.enqueue(() => this.ensureCurrentWithinQueue(reason));
  }

  async recoverAfterDaemonReconnect(
    paused: boolean,
  ): Promise<EnsureProjectResult | undefined> {
    if (paused) {
      this.log.info(
        "locald daemon recovered while the current project is paused; preserving the pause",
      );
      return undefined;
    }
    return this.ensureCurrent("daemon recovery");
  }

  async withCurrentProject<T>(
    reason: string,
    operation: CurrentProjectOperation<T>,
  ): Promise<T | undefined> {
    return this.enqueue(async () => {
      const initial = await this.ensureCurrentWithinQueue(reason);
      if (!initial) {
        return undefined;
      }
      this.operationRenewalPath = initial.project_path;
      try {
        return await operation(initial, (nextReason) =>
          this.ensureProjectWithinQueue(initial.project_path, nextReason),
        );
      } finally {
        this.operationRenewalPath = undefined;
      }
    });
  }

  async renewCurrent(): Promise<void> {
    const directPaths = this.directRenewalPaths();
    if (!this.disposed && directPaths.length > 0) {
      await Promise.all(
        directPaths.map((projectPath) =>
          this.renewProject(projectPath),
        ),
      );
      return;
    }

    await this.enqueue(async () => {
      if (this.disposed || !this.currentPath) {
        return;
      }
      await this.renewProject(this.currentPath);
    });
  }

  async releaseCurrent(): Promise<void> {
    await this.enqueue(async () => {
      await this.releaseCurrentWithinQueue();
    });
  }

  dispose(): void {
    this.disposed = true;
    this.cancelHeartbeat?.();
    this.cancelHeartbeat = undefined;
  }

  private startHeartbeat(): void {
    if (this.cancelHeartbeat !== undefined) {
      return;
    }
    this.cancelHeartbeat = this.scheduleRenewal(() => {
      void this.renewCurrent();
    }, this.renewalIntervalMs);
  }

  private async releaseCurrentWithinQueue(): Promise<void> {
    const projectPaths = new Set(this.uncertainRenewalPaths);
    if (this.currentPath) {
      projectPaths.add(this.currentPath);
    }
    this.currentPath = undefined;
    this.uncertainRenewalPaths.clear();
    let firstError: unknown;
    for (const projectPath of projectPaths) {
      try {
        await this.client.release(
          projectPath,
          this.windowId,
          this.hostPid,
        );
        this.log.info(`Released editor availability for ${projectPath}`);
      } catch (error) {
        firstError ??= error;
        this.log.warn(
          `Failed to release editor demand for ${projectPath}; it will expire automatically: ${formatError(error)}`,
        );
      }
    }
    if (firstError) {
      throw firstError;
    }
  }

  private async ensureCurrentWithinQueue(
    reason: string,
  ): Promise<EnsureProjectResult | undefined> {
    this.ensureNotDisposed();
    const nextPath = await this.resolveProject();
    if (!nextPath) {
      await this.releaseCurrentWithinQueue();
      return undefined;
    }

    const previousPath = this.currentPath;
    this.uncertainRenewalPaths.add(nextPath);
    const result = await this.ensureProjectWithinQueue(nextPath, reason);
    const confirmedPath = result.project_path;
    this.uncertainRenewalPaths.delete(nextPath);
    this.uncertainRenewalPaths.delete(confirmedPath);
    this.currentPath = confirmedPath;
    for (const uncertainPath of [...this.uncertainRenewalPaths]) {
      if (uncertainPath === previousPath) {
        continue;
      }
      try {
        await this.client.release(
          uncertainPath,
          this.windowId,
          this.hostPid,
        );
        this.uncertainRenewalPaths.delete(uncertainPath);
        this.log.info(
          `Released uncertain editor availability for ${uncertainPath}`,
        );
      } catch (error) {
        this.log.warn(
          `Failed to release uncertain editor demand for ${uncertainPath}; renewal will preserve it for cleanup retry: ${formatError(error)}`,
        );
      }
    }
    if (previousPath && previousPath !== confirmedPath) {
      this.uncertainRenewalPaths.delete(previousPath);
      try {
        await this.client.release(
          previousPath,
          this.windowId,
          this.hostPid,
        );
      } catch (error) {
        this.log.warn(
          `Failed to release previous editor demand for ${previousPath}; it will expire automatically: ${formatError(error)}`,
        );
      }
    }

    return result;
  }

  private directRenewalPaths(): string[] {
    const projectPaths = new Set(this.uncertainRenewalPaths);
    if (this.operationRenewalPath) {
      projectPaths.add(this.operationRenewalPath);
    }
    if (projectPaths.size > 0 && this.currentPath) {
      projectPaths.add(this.currentPath);
    }
    return [...projectPaths];
  }

  private async ensureProjectWithinQueue(
    projectPath: string,
    reason: string,
  ): Promise<EnsureProjectResult> {
    const result = await this.client.ensure(
      projectPath,
      this.windowId,
      this.hostPid,
    );
    this.log.info(
      `Editor availability ready for ${projectPath} after ${reason}`,
    );
    return result;
  }

  private async renewProject(projectPath: string): Promise<void> {
    try {
      await this.client.renew(
        projectPath,
        this.windowId,
        this.hostPid,
      );
    } catch (error) {
      this.log.warn(
        `Failed to renew editor demand for ${projectPath}; the next semantic activity will ensure it again: ${formatError(error)}`,
      );
    }
  }

  private ensureNotDisposed(): void {
    if (this.disposed) {
      throw new Error("editor availability controller is disposed");
    }
  }

  private enqueue<T>(operation: () => Promise<T>): Promise<T> {
    const result = this.pending.then(operation, operation);
    this.pending = result.then(
      () => undefined,
      () => undefined,
    );
    return result;
  }
}

function formatError(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
