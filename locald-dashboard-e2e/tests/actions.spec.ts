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
  const card = page.locator(".card").filter({ hasText: "web" });
  await expect(card).toBeVisible({ timeout: 10000 });

  // Check for running status (dot has class 'running')
  // We can check if the Stop button is visible, which implies running state
  await expect(card.getByRole("button", { name: "Stop" })).toBeVisible();

  // 4. Stop the service via card button
  await card.getByRole("button", { name: "Stop" }).click();

  // 5. Verify it stops
  // "Start" button should appear
  await expect(card.getByRole("button", { name: "Start" })).toBeVisible();
  // "Stop" button should disappear
  await expect(card.getByRole("button", { name: "Stop" })).not.toBeVisible();

  // 6. Start the service
  await card.getByRole("button", { name: "Start" }).click();

  // 7. Verify it starts
  await expect(card.getByRole("button", { name: "Stop" })).toBeVisible();

  // 8. Stop All via sidebar
  // Handle dialog
  page.on("dialog", (dialog) => dialog.accept());

  const sidebar = page.locator(".sidebar");
  await sidebar.getByRole("button", { name: "Stop All" }).click();

  // 9. Verify service stops
  await expect(card.getByRole("button", { name: "Start" })).toBeVisible();
});
