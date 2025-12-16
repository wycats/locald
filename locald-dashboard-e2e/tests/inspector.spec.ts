import { test, expect } from "./fixtures";

test("can inspect service details", async ({ page, locald }) => {
  // 1. Go to dashboard
  await page.goto(locald.getDashboardUrl());

  // Wait for SSE connection to be established
  await expect(page.locator("body")).toHaveAttribute(
    "data-sse-connected",
    "true",
    { timeout: 10000 }
  );

  // 2. Register dummy service
  await locald.runCli(["up", "examples/dummy-service"]);

  // 3. Wait for service card
  const card = page.locator(".card").filter({ hasText: "web" });
  await expect(card).toBeVisible({ timeout: 10000 });

  // 4. Click the config/settings button
  await card.locator(".config-btn").click();

  // 5. Verify drawer opens
  const drawer = page.locator(".drawer");
  await expect(drawer).toBeVisible();
  await expect(drawer).toContainText("web");

  // 6. Verify content
  await expect(drawer).toContainText("Status");
  await expect(drawer).toContainText("Configuration");

  // Check for command
  await expect(drawer).toContainText("./server.sh");

  // 7. Close drawer
  await drawer.getByRole("button", { name: "Close" }).click();
  await expect(drawer).not.toBeVisible();
});
