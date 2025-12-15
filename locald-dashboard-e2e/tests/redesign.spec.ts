import { test, expect } from "./fixtures";

test.describe("Dashboard Redesign v2", () => {
  test("Sidebar displays domains and actions", async ({ page, locald }) => {
    await page.goto(locald.getDashboardUrl());
    await expect(page.locator("body")).toHaveAttribute(
      "data-sse-connected",
      "true",
      { timeout: 10000 }
    );

    // Use shop-frontend which has a domain configured
    await locald.runCli(["up", "examples/shop-frontend"]);

    // 1. Sidebar Labeling
    // The project is "shop-frontend", service is "web".
    // Config has `domain = "shop.localhost"`.
    // We expect the sidebar to show the domain or service name.
    // 1. Sidebar should show the domain name "shop" (from shop.localhost)
    // instead of just the service name "web"

    // DEBUG: Print all sidebar items
    const items = page.locator(".sidebar-item");
    await items.first().waitFor();
    const texts = await items.allTextContents();
    console.log("Sidebar items:", texts);

    const sidebarItem = page
      .locator(".sidebar-item")
      .filter({ hasText: "shop" });
    await expect(sidebarItem).toBeVisible();

    // 2. Sidebar Actions (Hover)
    await sidebarItem.hover();
    const actions = sidebarItem.locator(".sidebar-actions");
    await expect(actions).toBeVisible();

    await expect(actions.locator('button[title="Restart"]')).toBeVisible();
    await expect(actions.locator('button[title="Terminal"]')).toBeVisible();
    // Open button might only appear if url is present, which it should be for shop-frontend
    await expect(actions.locator('[title="Open"]')).toBeVisible();
  });

  test("Service Card prefers domain URLs", async ({ page, locald }) => {
    await page.goto(locald.getDashboardUrl());
    await locald.runCli(["up", "examples/shop-frontend"]);

    const card = page.locator(".card").filter({ hasText: "shop" });
    await expect(card).toBeVisible();

    // The link should display the domain, not the port
    // We expect "shop.localhost" or similar, NOT "localhost:12345"
    const link = card.locator("a.link");
    await expect(link).toBeVisible();
    const linkText = await link.textContent();
    expect(linkText).toContain("shop.localhost");
    expect(linkText).not.toMatch(/localhost:\d+/);
  });

  test("Immersive Inspector (Focus Mode)", async ({ page, locald }) => {
    await page.goto(locald.getDashboardUrl());
    await locald.runCli(["up", "examples/shop-frontend"]);

    const sidebarItem = page
      .locator(".sidebar-item")
      .filter({ hasText: "shop" });
    await sidebarItem.hover();

    // Click the Terminal button
    await sidebarItem.locator('button[title="Terminal"]').click();

    // Expect the Immersive Inspector Overlay
    const inspector = page.locator(".inspector-focus");
    await expect(inspector).toBeVisible();

    // Check for the "Rail"
    await expect(inspector.locator(".metadata-rail")).toBeVisible();

    // Check for the Terminal
    await expect(inspector.locator(".terminal-container")).toBeVisible();
  });
});
