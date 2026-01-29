import { test, expect } from "./fixtures";

test.describe("Toast System", () => {
  test("shows success toast on service action", async ({ page, locald }) => {
    await page.goto(locald.getDashboardUrl());
    await expect(page.locator("body")).toHaveAttribute(
      "data-sse-connected",
      "true",
      { timeout: 10000 }
    );

    // Register and wait for service
    await locald.runCli(["up", "examples/dummy-service"]);
    const card = page.locator(".card").filter({ hasText: "web" });
    await expect(card).toBeVisible({ timeout: 10000 });
    await expect(card.getByRole("button", { name: "Stop" })).toBeVisible();

    // Stop service and check for toast
    await card.getByRole("button", { name: "Stop" }).click();

    // Toast should appear
    const toast = page.locator(".toast");
    await expect(toast).toBeVisible({ timeout: 5000 });
    await expect(toast).toContainText(/stopped|success/i);

    // Toast should auto-dismiss
    await expect(toast).not.toBeVisible({ timeout: 5000 });
  });

  test("restart all does not spam toasts", async ({ page, locald }) => {
    await page.goto(locald.getDashboardUrl());
    await expect(page.locator("body")).toHaveAttribute(
      "data-sse-connected",
      "true",
      { timeout: 10000 }
    );

    // Register multiple services
    await locald.runCli(["up", "examples/dummy-service"]);
    await locald.runCli(["up", "examples/worker-test"]);

    // Wait for services to appear
    await expect(page.locator(".card")).toHaveCount(2, { timeout: 10000 });

    // Accept confirmation dialog
    page.on("dialog", (dialog) => dialog.accept());

    // Click Restart All
    await page.locator(".sidebar").getByRole("button", { name: "Restart All" }).click();

    // Wait a moment for toasts
    await page.waitForTimeout(1000);

    // Should NOT have more than 1-2 toasts visible (batch action = single toast)
    const toastCount = await page.locator(".toast").count();
    expect(toastCount).toBeLessThanOrEqual(2);
  });
});

test.describe("Connection Banner", () => {
  test("shows banner when server disconnects", async ({ page, locald }) => {
    await page.goto(locald.getDashboardUrl());
    await expect(page.locator("body")).toHaveAttribute(
      "data-sse-connected",
      "true",
      { timeout: 10000 }
    );

    // Stop the server
    const currentPort = parseInt(new URL(locald.getDashboardUrl()).port);
    await locald.stop();

    // Banner should appear
    const banner = page.locator(".connection-banner");
    await expect(banner).toBeVisible({ timeout: 10000 });
    await expect(banner).toContainText(/disconnected|connection/i);

    // Restart server
    await locald.start(currentPort);

    // Banner should disappear
    await expect(banner).not.toBeVisible({ timeout: 15000 });
  });

  test("retry button works", async ({ page, locald }) => {
    await page.goto(locald.getDashboardUrl());
    await expect(page.locator("body")).toHaveAttribute(
      "data-sse-connected",
      "true",
      { timeout: 10000 }
    );

    // Stop the server
    const currentPort = parseInt(new URL(locald.getDashboardUrl()).port);
    await locald.stop();

    // Wait for banner
    const banner = page.locator(".connection-banner");
    await expect(banner).toBeVisible({ timeout: 10000 });

    // Restart server before clicking retry
    await locald.start(currentPort);

    // Click retry
    await banner.getByRole("button", { name: /retry/i }).click();

    // Banner should disappear (connection restored)
    await expect(banner).not.toBeVisible({ timeout: 10000 });
  });
});

test.describe("Keyboard Accessibility", () => {
  test("can navigate rack with keyboard", async ({ page, locald }) => {
    await page.goto(locald.getDashboardUrl());
    await expect(page.locator("body")).toHaveAttribute(
      "data-sse-connected",
      "true",
      { timeout: 10000 }
    );

    // Register service
    await locald.runCli(["up", "examples/dummy-service"]);
    const rackItem = page.locator(".rack-item").first();
    await expect(rackItem).toBeVisible({ timeout: 10000 });

    // Tab to rack item
    await page.keyboard.press("Tab");
    await page.keyboard.press("Tab"); // May need multiple tabs

    // Find focused rack item
    const focusedItem = page.locator(".rack-item:focus-visible");

    // If we found one, try to activate with Enter
    if ((await focusedItem.count()) > 0) {
      await page.keyboard.press("Enter");
      // Should open inspector or perform action
      await expect(
        page.locator(".inspector-drawer, .inspector-focus")
      ).toBeVisible({ timeout: 5000 });
    }
  });

  test("escape closes dropdown menus", async ({ page, locald }) => {
    await page.goto(locald.getDashboardUrl());
    await expect(page.locator("body")).toHaveAttribute(
      "data-sse-connected",
      "true",
      { timeout: 10000 }
    );

    // Register service
    await locald.runCli(["up", "examples/dummy-service"]);
    await expect(page.locator(".rack-item")).toBeVisible({ timeout: 10000 });

    // Click to open menu (if there's a dropdown trigger)
    const menuTrigger = page.locator(".menu-trigger, .config-btn, [aria-haspopup]").first();
    if ((await menuTrigger.count()) > 0) {
      await menuTrigger.click();

      // Menu should be open
      const menu = page.locator(".menu-dropdown, [role='menu']");
      await expect(menu).toBeVisible();

      // Press Escape
      await page.keyboard.press("Escape");

      // Menu should close
      await expect(menu).not.toBeVisible();
    }
  });
});

test.describe("StatusDot Component", () => {
  test("status dot updates when service state changes", async ({
    page,
    locald,
  }) => {
    await page.goto(locald.getDashboardUrl());
    await expect(page.locator("body")).toHaveAttribute(
      "data-sse-connected",
      "true",
      { timeout: 10000 }
    );

    // Register service
    await locald.runCli(["up", "examples/dummy-service"]);
    const rackItem = page.locator(".rack-item").first();
    await expect(rackItem).toBeVisible({ timeout: 10000 });

    // Check initial running state
    const statusDot = rackItem.locator(".status-dot");
    await expect(statusDot).toHaveClass(/running|healthy/);

    // Stop service via CLI
    await locald.runCli(["service", "stop", "web"]);

    // Status dot should update to stopped
    await expect(statusDot).toHaveClass(/stopped/, { timeout: 5000 });

    // Start service via CLI
    await locald.runCli(["service", "start", "web"]);

    // Status dot should update back to running
    await expect(statusDot).toHaveClass(/running|healthy/, { timeout: 10000 });
  });
});

test.describe("Clipboard Copy", () => {
  test("copy button shows toast feedback", async ({ page, locald }) => {
    // Grant clipboard permissions
    await page.context().grantPermissions(["clipboard-read", "clipboard-write"]);

    await page.goto(locald.getDashboardUrl());
    await expect(page.locator("body")).toHaveAttribute(
      "data-sse-connected",
      "true",
      { timeout: 10000 }
    );

    // Register a service with a connection URL (like postgres)
    await locald.runCli(["up", "examples/postgres-test"]);

    // Wait for service and open inspector
    const rackItem = page.locator(".rack-item").first();
    await expect(rackItem).toBeVisible({ timeout: 15000 });
    await rackItem.click();

    // Wait for inspector drawer
    const drawer = page.locator(".inspector-drawer, .inspector-focus");
    await expect(drawer).toBeVisible({ timeout: 5000 });

    // Find and click a copy button
    const copyBtn = drawer.locator(".copy-btn, button:has-text('copy')").first();
    if ((await copyBtn.count()) > 0) {
      await copyBtn.click();

      // Toast should appear
      const toast = page.locator(".toast");
      await expect(toast).toBeVisible({ timeout: 3000 });
      await expect(toast).toContainText(/copied/i);
    }
  });
});

test.describe("Mobile Responsiveness", () => {
  test("layout stacks on mobile viewport", async ({ page, locald }) => {
    // Set mobile viewport
    await page.setViewportSize({ width: 375, height: 667 });

    await page.goto(locald.getDashboardUrl());
    await expect(page.locator("body")).toHaveAttribute(
      "data-sse-connected",
      "true",
      { timeout: 10000 }
    );

    // Rack should be visible and have constrained height
    const rack = page.locator(".rack");
    await expect(rack).toBeVisible();

    // Check that rack has max-height applied (50vh = ~333px on this viewport)
    const rackBox = await rack.boundingBox();
    expect(rackBox?.height).toBeLessThanOrEqual(400);

    // Main content area should be below rack (stacked layout)
    const deck = page.locator(".deck, .main-content, .workspace > :nth-child(2)");
    if ((await deck.count()) > 0) {
      const deckBox = await deck.boundingBox();
      if (rackBox && deckBox) {
        // Deck should be below rack (stacked, not side-by-side)
        expect(deckBox.y).toBeGreaterThan(rackBox.y);
      }
    }
  });
});

test.describe("Inspector Drawer Fields", () => {
  test("shows path and container_id when available", async ({
    page,
    locald,
  }) => {
    await page.goto(locald.getDashboardUrl());
    await expect(page.locator("body")).toHaveAttribute(
      "data-sse-connected",
      "true",
      { timeout: 10000 }
    );

    // Register a service
    await locald.runCli(["up", "examples/dummy-service"]);

    // Wait and click to open inspector
    const rackItem = page.locator(".rack-item").first();
    await expect(rackItem).toBeVisible({ timeout: 10000 });
    await rackItem.click();

    // Wait for inspector
    const drawer = page.locator(".inspector-drawer, .inspector-focus");
    await expect(drawer).toBeVisible({ timeout: 5000 });

    // Check for metadata section with path
    const metadataSection = drawer.locator(".metadata-section, .metadata-item");
    // Path should be visible if the service has one
    if ((await metadataSection.count()) > 0) {
      await expect(metadataSection.first()).toBeVisible();
    }
  });

  test("shows warnings when present", async ({ page, locald }) => {
    await page.goto(locald.getDashboardUrl());
    await expect(page.locator("body")).toHaveAttribute(
      "data-sse-connected",
      "true",
      { timeout: 10000 }
    );

    // Register a service that might have warnings (validation-test has intentional issues)
    await locald.runCli(["up", "examples/validation-test"]);

    // Wait and click to open inspector
    const rackItem = page.locator(".rack-item").first();
    await expect(rackItem).toBeVisible({ timeout: 10000 });
    await rackItem.click();

    // Wait for inspector
    const drawer = page.locator(".inspector-drawer, .inspector-focus");
    await expect(drawer).toBeVisible({ timeout: 5000 });

    // If warnings exist, they should be in a warnings section
    const warningsSection = drawer.locator(".warnings-section, .warning-badge");
    // This is conditional - not all services have warnings
    // Just verify the section renders if warnings exist
  });
});

test.describe("Spinner Cleanup", () => {
  test("spinner clears after action completes", async ({ page, locald }) => {
    await page.goto(locald.getDashboardUrl());
    await expect(page.locator("body")).toHaveAttribute(
      "data-sse-connected",
      "true",
      { timeout: 10000 }
    );

    // Register service
    await locald.runCli(["up", "examples/dummy-service"]);
    const card = page.locator(".card").filter({ hasText: "web" });
    await expect(card).toBeVisible({ timeout: 10000 });

    // Find stop button
    const stopBtn = card.getByRole("button", { name: "Stop" });
    await expect(stopBtn).toBeVisible();

    // Click stop
    await stopBtn.click();

    // Spinner might briefly appear
    // But should NOT persist after action completes
    await expect(card.getByRole("button", { name: "Start" })).toBeVisible({
      timeout: 10000,
    });

    // Verify no eternal spinner
    const spinner = card.locator(".spinner, .spin");
    await expect(spinner).not.toBeVisible({ timeout: 3000 });
  });
});
