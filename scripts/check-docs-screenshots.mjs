import fs from "node:fs";
import path from "node:path";

const repoRoot = path.resolve(
  path.dirname(new URL(import.meta.url).pathname),
  ".."
);

const docsContentRoot = path.join(
  repoRoot,
  "locald-docs",
  "src",
  "content",
  "docs"
);
const screenshotsRoot = path.join(
  repoRoot,
  "locald-docs",
  "public",
  "screenshots"
);

function walk(dir) {
  const out = [];
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) out.push(...walk(full));
    else out.push(full);
  }
  return out;
}

function main() {
  const files = walk(docsContentRoot).filter(
    (p) => p.endsWith(".md") || p.endsWith(".mdx")
  );

  const referenced = new Set();
  const refRegex = /\/(screenshots\/[^\s)\]\"']+\.png)/g;

  for (const filePath of files) {
    const text = fs.readFileSync(filePath, "utf8");
    for (const match of text.matchAll(refRegex)) {
      referenced.add(match[1]);
    }
  }

  const missing = [];
  for (const rel of referenced) {
    const fileName = rel.replace(/^screenshots\//, "");
    const abs = path.join(screenshotsRoot, fileName);
    if (!fs.existsSync(abs)) {
      missing.push(rel);
    }
  }

  if (missing.length > 0) {
    process.stderr.write(
      "Missing docs screenshot assets (referenced by docs content):\n"
    );
    for (const m of missing.sort()) {
      process.stderr.write(`- /${m}\n`);
    }
    process.stderr.write(
      `\nExpected files under: ${path.relative(repoRoot, screenshotsRoot)}/\n`
    );
    process.exit(1);
  }

  process.stdout.write("Docs screenshot assets: OK\n");
}

main();
