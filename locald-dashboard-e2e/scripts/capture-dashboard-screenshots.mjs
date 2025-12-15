import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "@playwright/test";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

// This script lives at: <repo>/locald-dashboard-e2e/scripts/*.mjs
// So: repo root is two levels up from this directory.
const REPO_ROOT = path.resolve(__dirname, "..", "..");
const OUT_DIR_ASSETS = path.join(
  REPO_ROOT,
  "locald-docs",
  "src",
  "assets",
  "screenshots"
);
const OUT_DIR_PUBLIC = path.join(
  REPO_ROOT,
  "locald-docs",
  "public",
  "screenshots"
);

const DASHBOARD_URL = (
  process.env.DASHBOARD_URL || "http://locald.localhost/"
).replace(/\/+$/, "/");

function ensureDir(dir) {
  fs.mkdirSync(dir, { recursive: true });
}

async function capture(page, fileBaseName) {
  const fileName = `${fileBaseName}.png`;
  const assetPath = path.join(OUT_DIR_ASSETS, fileName);
  const publicPath = path.join(OUT_DIR_PUBLIC, fileName);
  await page.screenshot({ path: assetPath, fullPage: true });
  // Also write a stable, directly-served PNG for clickable links in the docs.
  fs.copyFileSync(assetPath, publicPath);
  return { assetPath, publicPath };
}

async function main() {
  ensureDir(OUT_DIR_ASSETS);
  ensureDir(OUT_DIR_PUBLIC);

  const browser = await chromium.launch();
  const context = await browser.newContext({
    // Smaller viewport reads better in docs and matches typical laptop sizes.
    viewport: { width: 1440, height: 900 },
    ignoreHTTPSErrors: true,
  });

  const page = await context.newPage();

  const response = await page.goto(DASHBOARD_URL, { waitUntil: "load" });
  if (!response) {
    throw new Error(
      `Navigation failed (no response). Is the dashboard running at ${DASHBOARD_URL}?`
    );
  }
  if (response.status() >= 400) {
    throw new Error(
      `Dashboard returned HTTP ${response.status()}. Is the dashboard serving ${DASHBOARD_URL}?`
    );
  }

  // Avoid waiting for "networkidle" (dashboard likely uses websockets / long-polling).
  // Instead, wait briefly and try to observe a few likely UI markers.
  await page.waitForTimeout(750);
  await Promise.race([
    page.waitForSelector("main", { timeout: 3000 }),
    page.waitForSelector("[data-testid]", { timeout: 3000 }),
    page.getByText("Rack", { exact: false }).waitFor({ timeout: 3000 }),
    page.getByText("Stream", { exact: false }).waitFor({ timeout: 3000 }),
  ]).catch(() => undefined);

  const overview = await capture(page, "dashboard-overview");

  // Best-effort: try to open the System Plane if the UI exposes a "System Normal" entry.
  // If this selector fails, we still keep the overview screenshot.
  try {
    const systemFooter = page.locator(".rack-footer", {
      hasText: "System Normal",
    });
    await systemFooter.waitFor({ timeout: 5000 });
    await systemFooter.click({ timeout: 1500 });
    // Wait for the control center to render something non-empty.
    await page.waitForSelector('[data-testid="daemon-control-center"]', {
      timeout: 3000,
    });
    await page.waitForTimeout(500);
    const system = await capture(page, "dashboard-system-plane");
    process.stdout.write(`Captured: ${system.assetPath}\n`);
    process.stdout.write(`Copied:   ${system.publicPath}\n`);
  } catch (err) {
    const title = await page.title().catch(() => "<no title>");
    const footerCount = await page
      .locator(".rack-footer")
      .count()
      .catch(() => -1);
    const html = await page.content().catch(() => "");
    const hasText = html.includes("System Normal");
    process.stdout.write(
      `System Plane entry not found; skipped. (title=${JSON.stringify(
        title
      )} rackFooter=${footerCount} hasText=${hasText})\n`
    );
    if (err && err.message) {
      process.stdout.write(`Reason: ${err.message}\n`);
    }
  }

  process.stdout.write(`Captured: ${overview.assetPath}\n`);
  process.stdout.write(`Copied:   ${overview.publicPath}\n`);

  await context.close();
  await browser.close();
}

main().catch((err) => {
  console.error(err);
  process.exitCode = 1;
});
