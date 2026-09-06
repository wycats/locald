import { defineConfig, devices } from "@playwright/test";

const port = Number(process.env.SERVICE_CARD_FIXTURE_PORT ?? 47831);
export default defineConfig({
  testDir: "./tests",
  testMatch: "service-cards.spec.ts",
  fullyParallel: false,
  workers: 1,
  retries: 0,
  forbidOnly: !!process.env.CI,
  reporter: "list",
  use: {
    baseURL: `http://127.0.0.1:${port}`,
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
  },
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],
  webServer: {
    command: "node scripts/service-card-fixture.mjs",
    url: `http://127.0.0.1:${port}/__fixture/health`,
    reuseExistingServer: false,
    timeout: 10000,
  },
});
