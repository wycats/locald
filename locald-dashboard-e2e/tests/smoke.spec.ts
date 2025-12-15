import { test, expect } from "./fixtures";

test("dashboard loads", async ({ page, locald }) => {
  // Navigate to the dashboard
  await page.goto(locald.getDashboardUrl());

  // Verify title or some element
  await expect(page).toHaveTitle(/locald/i);

  // Verify the "Projects" header is visible (basic smoke test)
  // Adjust selector based on actual dashboard content
  // await expect(page.getByText('Projects')).toBeVisible();
});
