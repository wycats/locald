import { test, expect } from "./fixtures";

test("can navigate between projects", async ({ page, locald }) => {
  // 1. Go to dashboard first
  await page.goto(locald.getDashboardUrl());
  await expect(page).toHaveTitle(/locald/i);

  // 2. Register two projects
  await locald.runCli(["up", "examples/dummy-service"]);
  await locald.runCli(["up", "examples/shop-backend"]);

  // 3. Verify both projects are visible in the main view (All Projects)
  const main = page.locator("main");
  // Wait for them to appear in the sidebar first, as a sanity check
  const sidebar = page.locator(".sidebar");
  await expect(sidebar.getByRole("button", { name: "dummy" })).toBeVisible({
    timeout: 10000,
  });
  await expect(
    sidebar.getByRole("button", { name: "shop-backend" })
  ).toBeVisible({ timeout: 10000 });

  // Now check main view
  await expect(main.getByRole("heading", { name: "dummy" })).toBeVisible();
  await expect(
    main.getByRole("heading", { name: "shop-backend" })
  ).toBeVisible();

  // 4. Filter by "dummy"
  // Click "dummy" in the sidebar (it's a button, not a link)
  await sidebar.getByRole("button", { name: "dummy" }).click();

  // 5. Verify only "dummy" is visible
  await expect(main.getByRole("heading", { name: "dummy" })).toBeVisible();
  await expect(
    main.getByRole("heading", { name: "shop-backend" })
  ).not.toBeVisible();

  // 6. Filter by "shop-backend"
  await sidebar.getByRole("button", { name: "shop-backend" }).click();

  // 7. Verify only "shop-backend" is visible
  await expect(
    main.getByRole("heading", { name: "shop-backend" })
  ).toBeVisible();
  await expect(main.getByRole("heading", { name: "dummy" })).not.toBeVisible();

  // 8. Go back to "All Projects"
  await sidebar.getByRole("button", { name: "All Projects" }).click();
  await expect(main.getByRole("heading", { name: "dummy" })).toBeVisible();
  await expect(
    main.getByRole("heading", { name: "shop-backend" })
  ).toBeVisible();
});
