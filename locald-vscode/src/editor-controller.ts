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

  async withCurrentProject<T>(
    reason: string,
    operation: CurrentProjectOperation<T>,
  ): Promise<T | undefined> {
    return this.enqueue(async () => {
      const initial = await this.ensureCurrentWithinQueue(reason);
      if (!initial) {
        return undefined;
      }
      return operation(initial, (nextReason) =>
        this.ensureProjectWithinQueue(initial.project_path, nextReason),
      );
    });
  }

  async renewCurrent(): Promise<void> {
    await this.enqueue(async () => {
      if (this.disposed || !this.currentPath) {
        return;
      }
      const projectPath = this.currentPath;
      try {
        await this.client.renew(projectPath, this.windowId, this.hostPid);
      } catch (error) {
        this.log.warn(
          `Failed to renew editor demand for ${projectPath}; the next semantic activity will ensure it again: ${formatError(error)}`,
        );
      }
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
    const projectPath = this.currentPath;
    this.currentPath = undefined;
    if (!projectPath) {
      return;
    }
    await this.client.release(projectPath, this.windowId, this.hostPid);
    this.log.info(`Released editor availability for ${projectPath}`);
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
    this.currentPath = nextPath;
    let result: EnsureProjectResult;
    try {
      result = await this.ensureProjectWithinQueue(nextPath, reason);
    } finally {
      if (previousPath && previousPath !== nextPath) {
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
    }

    return result;
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
