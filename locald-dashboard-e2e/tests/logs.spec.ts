import { test, expect } from "./fixtures";
import fs from "fs";
import path from "path";
import os from "os";

test("dashboard streams logs from a running service", async ({
  page,
  locald,
}) => {
  // 1. Create a temporary project directory
  const projectDir = fs.mkdtempSync(path.join(os.tmpdir(), "locald-e2e-logs-"));
  console.log(`Created temp project dir: ${projectDir}`);

  try {
    // 2. Create a locald.toml with a logging service
    // We use a simple shell loop that prints a counter
    const localdToml = `
[project]
name = "logs-test"

[services.logger]
command = "sh -c 'i=0; while true; do echo \\"Log entry $i\\"; i=$((i+1)); sleep 0.5; done'"
`;
    fs.writeFileSync(path.join(projectDir, "locald.toml"), localdToml);

    // 3. Register and start the project
    // We use 'up' to register and start.
    // The fixture has already started the daemon, so 'up' will talk to it via the sandbox socket.
    await locald.runCli(["up"], projectDir);

    // 4. Navigate to dashboard
    await page.goto(locald.getDashboardUrl());

    // 5. Wait for the service to appear in the grid
    // It might take a moment for the state to sync
    const card = page.locator(".card").filter({ hasText: "logger" });
    await expect(card).toBeVisible({ timeout: 10000 });

    // 6. Verify logs appear in the card (Preview)
    // The card shows the last 3 lines
    await expect(card.locator(".log-line")).toContainText("Log entry", {
      timeout: 10000,
    });

    // 7. Click on the service to open inspector
    await card.click();

    // 8. Verify logs are streaming in the Inspector (xterm.js)
    // We check for the presence of log entries in the xterm accessibility buffer or rows
    const terminal = page.locator(".xterm-accessibility, .xterm-rows");
    await expect(terminal).toContainText("Log entry", { timeout: 10000 });

    // 9. Verify updates (wait for a higher number)
    // This confirms it's streaming, not just static
    // We wait for "Log entry 5" which should appear after ~2.5 seconds
    await expect(terminal).toContainText("Log entry 5", { timeout: 10000 });
  } finally {
    // Cleanup
    try {
      // Stop the service first to release locks/processes?
      // The sandbox cleanup in fixture should handle it, but good practice.
      // fs.rmSync might fail if processes are holding files.
      // We'll rely on the fixture teardown to kill locald, then we can clean up.
      // But we are inside the test, so locald is still running.
      // We can try to stop the project.
      await locald.runCli(["stop"], projectDir);
      fs.rmSync(projectDir, { recursive: true, force: true });
    } catch (e) {
      console.error("Failed to cleanup temp dir", e);
    }
  }
});
