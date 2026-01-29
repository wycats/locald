import { test, expect } from "./fixtures";

test("can start and stop services", async ({ page, locald }) => {
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

  // 3. Wait for service to appear and be running
  // The UI uses .rack-item for service items, not .card
  const rackItem = page.locator(".rack-item").filter({ hasText: "web" });
  await expect(rackItem).toBeVisible({ timeout: 10000 });

  // Check for running status - status dot should have 'running' class
  const statusDot = rackItem.locator(".status-dot");
  await expect(statusDot).toHaveClass(/running/, { timeout: 5000 });

  // 4. Stop the service via More dropdown
  await rackItem.hover();
  const moreBtn = rackItem.getByRole("button", { name: "More" });
  await expect(moreBtn).toBeVisible();
  await moreBtn.click();
  await page.locator(".menu-dropdown").getByText("Stop").click();

  // 5. Verify it stops - status dot should change to 'stopped'
  await expect(statusDot).toHaveClass(/stopped/, { timeout: 10000 });

  // Start button should appear (in toolbar when stopped)
  await rackItem.hover();
  await expect(rackItem.getByRole("button", { name: "Start" })).toBeVisible();

  // 6. Start the service
  await rackItem.getByRole("button", { name: "Start" }).click();

  // 7. Verify it starts
  await expect(statusDot).toHaveClass(/running/, { timeout: 10000 });

  // 8. Stop Group via group header
  page.on("dialog", (dialog) => dialog.accept());
  await page.locator(".group-btn[title='Stop Group']").first().click();

  // 9. Verify service stops
  await expect(statusDot).toHaveClass(/stopped/, { timeout: 10000 });
});
