import { test, expect } from "./fixtures";

test("dashboard handles server restart gracefully", async ({
  page,
  locald,
}) => {
  // 1. Navigate to dashboard
  await page.goto(locald.getDashboardUrl());

  // 2. Verify initial connection
  await expect(page.locator("body")).toHaveAttribute(
    "data-sse-connected",
    "true",
    { timeout: 10000 }
  );

  // 3. Stop the server
  console.log("Stopping locald server...");
  await locald.stop();

  // 4. Verify disconnected state
  // Note: EventSource might take a moment to realize it's disconnected
  await expect(page.locator("body")).toHaveAttribute(
    "data-sse-connected",
    "false",
    { timeout: 10000 }
  );

  // 5. Restart the server on the SAME port
  console.log("Restarting locald server...");
  const currentPort = parseInt(new URL(locald.getDashboardUrl()).port);
  await locald.start(currentPort);

  // 6. Verify reconnection
  // The dashboard should automatically reconnect via EventSource retry logic
  await expect(page.locator("body")).toHaveAttribute(
    "data-sse-connected",
    "true",
    { timeout: 15000 }
  );
});
