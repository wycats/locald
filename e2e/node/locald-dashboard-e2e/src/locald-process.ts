import { spawn, execFile, ChildProcess } from "child_process";
import path from "path";
import fs from "fs";
import net from "net";
import { promisify } from "util";

const execFileAsync = promisify(execFile);

export class LocaldProcess {
  private process: ChildProcess | null = null;
  private sandboxName: string;
  private projectRoot: string;
  private port: number = 0;

  constructor(sandboxName: string) {
    this.sandboxName = sandboxName;
    // __dirname: <repo>/e2e/node/locald-dashboard-e2e/src
    // repo root is four levels up from here.
    this.projectRoot = path.resolve(__dirname, "../../../../");
  }

  async start(port?: number) {
    const binaryPath = path.join(this.projectRoot, "target/debug/locald");

    if (!fs.existsSync(binaryPath)) {
      throw new Error(
        `locald binary not found at ${binaryPath}. Please run 'cargo build' first.`
      );
    }

    if (port) {
      this.port = port;
    } else {
      this.port = await this.findFreePort();
    }

    console.log(
      `Starting locald with sandbox: ${this.sandboxName} on port ${this.port}`
    );

    this.process = spawn(
      binaryPath,
      ["--sandbox", this.sandboxName, "server", "start"],
      {
        cwd: this.projectRoot,
        stdio: "pipe",
        env: {
          ...process.env,
          RUST_LOG: "info",
          LOCALD_HTTP_PORT: this.port.toString(),
        },
      }
    );

    this.process.stdout?.on("data", (data) => {
      console.log(`[locald]: ${data}`);
    });

    this.process.stderr?.on("data", (data) => {
      console.error(`[locald err]: ${data}`);
    });

    // Wait for health check
    await this.waitForHealth();
  }

  async stop() {
    if (this.process) {
      console.log("Stopping locald...");
      this.process.kill("SIGTERM");
      // Give it a moment to shut down gracefully
      await new Promise((resolve) => setTimeout(resolve, 1000));
      if (this.process.exitCode === null) {
        this.process.kill("SIGKILL");
      }
      this.process = null;
    }
  }

  private async waitForHealth(retries = 30, delay = 500): Promise<void> {
    for (let i = 0; i < retries; i++) {
      try {
        if (this.process?.exitCode !== null) {
          throw new Error(
            `locald exited unexpectedly with code ${this.process?.exitCode}`
          );
        }

        // Try to connect to the port
        await this.checkPort(this.port);
        return;
      } catch (e) {
        await new Promise((r) => setTimeout(r, delay));
      }
    }
    throw new Error("Timed out waiting for locald to start");
  }

  private checkPort(port: number): Promise<void> {
    return new Promise((resolve, reject) => {
      const socket = new net.Socket();
      socket.setTimeout(1000);
      socket.on("connect", () => {
        socket.destroy();
        resolve();
      });
      socket.on("timeout", () => {
        socket.destroy();
        reject(new Error("Timeout"));
      });
      socket.on("error", (err) => {
        socket.destroy();
        reject(err);
      });
      socket.connect(port, "localhost");
    });
  }

  private findFreePort(): Promise<number> {
    return new Promise((resolve, reject) => {
      const server = net.createServer();
      server.unref();
      server.on("error", reject);
      server.listen(0, () => {
        const port = (server.address() as net.AddressInfo).port;
        server.close(() => {
          resolve(port);
        });
      });
    });
  }

  getDashboardUrl(): string {
    return `http://localhost:${this.port}`;
  }

  async runCli(
    args: string[],
    cwd?: string
  ): Promise<{ stdout: string; stderr: string }> {
    const binaryPath = path.join(this.projectRoot, "target/debug/locald");
    const allArgs = ["--sandbox", this.sandboxName, ...args];

    console.log(`Running CLI: locald ${allArgs.join(" ")}`);

    return execFileAsync(binaryPath, allArgs, {
      cwd: cwd || this.projectRoot,
      env: {
        ...process.env,
        // Ensure CLI knows where to find the socket if needed, though --sandbox should handle it
      },
    });
  }
}
