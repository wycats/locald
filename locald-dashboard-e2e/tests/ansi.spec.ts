import { test, expect } from "./fixtures";

test("renders ansi codes correctly", async ({ page, locald }) => {
  // 1. Go to dashboard
  await page.goto(locald.getDashboardUrl());

  // Wait for SSE connection
  await expect(page.locator("body")).toHaveAttribute(
    "data-sse-connected",
    "true",
    { timeout: 10000 }
  );

  // 2. Register ansi-test service
  await locald.runCli(["up", "examples/ansi-test"]);

  // 3. Wait for service card
  const card = page.locator(".card").filter({ hasText: "ansi-log" });
  await expect(card).toBeVisible({ timeout: 10000 });

  // 4. Verify logs appear and are styled
  // We see "mise WARN" with ANSI codes in the logs
  // The "WARN" text should be styled
  const warnLog = card
    .locator(".log-line span")
    .filter({ hasText: "WARN" })
    .first();
  await expect(warnLog).toBeVisible({ timeout: 5000 });

  // Check that it has color style
  await expect(warnLog).toHaveAttribute("style", /color:/);

  // 5. Verify raw ANSI codes are NOT present in the text content
  const logText = await card.locator(".body").textContent();
  expect(logText).not.toContain("[33m");
  expect(logText).not.toContain("\x1b[");
});
