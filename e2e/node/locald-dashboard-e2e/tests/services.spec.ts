import { test, expect } from "./fixtures";

test("can register and inspect a service", async ({ page, locald }) => {
  // 1. Start the dashboard
  await page.goto(locald.getDashboardUrl());
  await expect(page).toHaveTitle(/locald/i);

  // 2. Register and start the dummy service
  // We use 'up' which registers the project in the current directory (or path provided)
  // and starts the services.
  await locald.runCli(["up", "examples/dummy-service"]);

  // 3. Verify the service appears in the grid
  // The dummy service is named "dummy" in locald.toml, and the service is "web".
  // So it might show up as "dummy" (project) or "web" (service).
  // The dashboard groups by project.

  // Wait for the project card to appear
  // We expect a heading with the project name in the main content
  const main = page.locator("main");
  await expect(main.getByRole("heading", { name: "dummy" })).toBeVisible({
    timeout: 10000,
  });

  // Verify the service "web" is listed under the project in the main content
  await expect(main.getByText("web", { exact: true })).toBeVisible();

  // 4. Inspect the service
  // Click on the service card/row to open inspector
  await main.getByText("web", { exact: true }).click();

  // 5. Verify Inspector Drawer opens
  // It should show the service name and status
  await expect(page.getByRole("heading", { name: "web" })).toBeVisible();

  // Verify logs are present (it echoes "Starting dummy server...")
  // We might need to wait a bit for logs to stream
  await expect(page.getByText("Starting dummy server")).toBeVisible({
    timeout: 10000,
  });

  // 6. Close Inspector
  await page.getByRole("button", { name: "Close" }).click();
  await expect(page.getByRole("heading", { name: "web" })).not.toBeVisible();
});
