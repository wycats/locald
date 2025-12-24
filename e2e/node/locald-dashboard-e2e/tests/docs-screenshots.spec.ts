import { test, expect } from "@playwright/test";

const DASHBOARD_URL = (
  process.env.DASHBOARD_URL || "https://dev.locald.localhost/"
).replace(/\/+$/, "/");

test.describe("Docs screenshots (visual)", () => {
  test.use({
    viewport: { width: 1440, height: 900 },
    ignoreHTTPSErrors: true,
  });

  test("dashboard overview", async ({ page }) => {
    await page.goto(DASHBOARD_URL, { waitUntil: "load" });

    // Avoid waiting for "networkidle" due to SSE/websockets.
    await Promise.race([
      page.waitForSelector("main", { timeout: 5000 }),
      page.waitForSelector(".rack", { timeout: 5000 }),
      page.getByText("Rack", { exact: false }).waitFor({ timeout: 5000 }),
      page.getByText("Stream", { exact: false }).waitFor({ timeout: 5000 }),
    ]).catch(() => undefined);

    // Invariant: overview should be in Stream mode.
    await page.locator('[data-testid="stream"]').waitFor({ timeout: 10000 });

    await expect(page).toHaveScreenshot("dashboard-overview.png", {
      fullPage: true,
      mask: [page.locator(".terminal-container")],
    });
  });

  test("system plane", async ({ page }) => {
    await page.goto(DASHBOARD_URL, { waitUntil: "load" });

    const systemFooter = page.locator(".rack-footer", {
      hasText: "System Normal",
    });
    await systemFooter.waitFor({ timeout: 10000 });

    // The System pin can be persisted across runs; ensure it's pinned (active)
    // rather than blindly toggling.
    const isActive = await systemFooter.evaluate((el) =>
      el.classList.contains("active")
    );
    if (!isActive) {
      await systemFooter.click();
    }

    // Invariant: pinning System should switch the main view into Deck mode.
    await page.locator('[data-testid="deck"]').waitFor({ timeout: 10000 });

    await page.waitForSelector('[data-testid="daemon-control-center"]', {
      timeout: 10000,
    });
    await page.waitForTimeout(300);

    await expect(page).toHaveScreenshot("dashboard-system-plane.png", {
      fullPage: true,
      mask: [page.locator(".terminal-container")],
    });
  });
});
