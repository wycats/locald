import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

// <repo>/e2e/node/locald-dashboard-e2e/scripts
const REPO_ROOT = path.resolve(__dirname, "..", "..", "..", "..");

const SNAPSHOT_DIR = path.join(
  REPO_ROOT,
  "e2e/node/locald-dashboard-e2e",
  "tests",
  "docs-screenshots.spec.ts-snapshots"
);

const DEST_PUBLIC = path.join(
  REPO_ROOT,
  "locald-docs",
  "public",
  "screenshots"
);
const DEST_ASSETS = path.join(
  REPO_ROOT,
  "locald-docs",
  "src",
  "assets",
  "screenshots"
);

const FILES = ["dashboard-overview.png", "dashboard-system-plane.png"];

function ensureDir(dir) {
  fs.mkdirSync(dir, { recursive: true });
}

function resolveSnapshotPath(fileName) {
  const exact = path.join(SNAPSHOT_DIR, fileName);
  if (fs.existsSync(exact)) return exact;

  const { name, ext } = path.parse(fileName);
  const entries = fs.readdirSync(SNAPSHOT_DIR);
  const candidates = entries
    .filter(
      (entry) => entry.startsWith(`${name}-chromium`) && entry.endsWith(ext)
    )
    .sort();

  if (candidates.length === 1) {
    return path.join(SNAPSHOT_DIR, candidates[0]);
  }

  // Prefer linux snapshots (common in CI/dev) when multiple exist.
  const linux = candidates.find((c) => c.includes("-linux"));
  if (linux) return path.join(SNAPSHOT_DIR, linux);

  if (candidates.length > 0) {
    return path.join(SNAPSHOT_DIR, candidates[0]);
  }

  return null;
}

function copyOne(fileName) {
  const src = resolveSnapshotPath(fileName);
  if (!src) {
    throw new Error(
      `Missing snapshot for: ${fileName}\nLooked in: ${SNAPSHOT_DIR}\n\nRun: pnpm -C e2e/node/locald-dashboard-e2e screenshots:update (or screenshots:ui)`
    );
  }

  const destPublic = path.join(DEST_PUBLIC, fileName);
  const destAssets = path.join(DEST_ASSETS, fileName);

  fs.copyFileSync(src, destPublic);
  fs.copyFileSync(src, destAssets);

  process.stdout.write(`Synced: ${fileName}\n`);
}

function main() {
  ensureDir(DEST_PUBLIC);
  ensureDir(DEST_ASSETS);

  process.stdout.write(`Using snapshots from: ${SNAPSHOT_DIR}\n`);

  for (const fileName of FILES) {
    copyOne(fileName);
  }
}

main();
