import fs from "node:fs";
import path from "node:path";

const scriptDir = path.dirname(new URL(import.meta.url).pathname);
const localdDocsDir = path.dirname(scriptDir);
const repoRoot = path.dirname(localdDocsDir);

const srcDesign = path.join(repoRoot, "docs", "design");
const destConcepts = path.join(
  localdDocsDir,
  "src",
  "content",
  "docs",
  "concepts"
);
const destInternals = path.join(
  localdDocsDir,
  "src",
  "content",
  "docs",
  "internals"
);

function ensureDir(dir) {
  fs.mkdirSync(dir, { recursive: true });
}

function extractTitle(markdown, titleOverride) {
  if (titleOverride) return titleOverride;
  const match = markdown.match(/^#\s+(.+)\s*$/m);
  return match ? match[1].trim() : "Untitled";
}

function stripFirstH1(markdown) {
  const lines = markdown.split(/\r?\n/);
  let removed = false;
  const out = [];
  for (const line of lines) {
    if (!removed && line.startsWith("# ")) {
      removed = true;
      continue;
    }
    out.push(line);
  }
  return out.join("\n");
}

function writeWithFrontmatter(srcFile, destFile, titleOverride = "") {
  if (!fs.existsSync(srcFile)) {
    console.warn(`Warning: Source file not found: ${srcFile}`);
    return;
  }

  ensureDir(path.dirname(destFile));

  const raw = fs.readFileSync(srcFile, "utf8");
  const title = extractTitle(raw, titleOverride);

  console.log(`Syncing ${srcFile} -> ${destFile} (Title: ${title})`);

  const body = stripFirstH1(raw);
  const content = `---\ntitle: "${title.replaceAll(
    '"',
    '\\"'
  )}"\n---\n\n${body}\n`;

  fs.writeFileSync(destFile, content);
}

function walkMarkdownFiles(dir) {
  const out = [];
  if (!fs.existsSync(dir)) return out;

  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) out.push(...walkMarkdownFiles(full));
    else if (entry.isFile() && full.endsWith(".md")) out.push(full);
  }
  return out;
}

function main() {
  ensureDir(destConcepts);
  ensureDir(destInternals);
  ensureDir(path.join(destInternals, "axioms"));

  const concepts = [
    "vision.md",
    "generative-design.md",
    "modes.md",
    "personas.md",
    "user-interaction-modes.md",
    "workflow-axioms.md",
  ];

  for (const file of concepts) {
    writeWithFrontmatter(
      path.join(srcDesign, file),
      path.join(destConcepts, file)
    );
  }

  // Axioms index
  writeWithFrontmatter(
    path.join(srcDesign, "axioms.md"),
    path.join(destInternals, "axioms.md")
  );

  // Axioms subfiles
  const srcAxiomsDir = path.join(srcDesign, "axioms");
  const destAxiomsDir = path.join(destInternals, "axioms");

  if (fs.existsSync(srcAxiomsDir)) {
    fs.rmSync(destAxiomsDir, { recursive: true, force: true });
    ensureDir(destAxiomsDir);

    for (const srcFile of walkMarkdownFiles(srcAxiomsDir)) {
      const rel = path.relative(srcAxiomsDir, srcFile);
      const destFile = path.join(destAxiomsDir, rel);
      writeWithFrontmatter(srcFile, destFile);
    }
  }

  console.log(`Design docs synced to ${destConcepts} and ${destInternals}`);
}

main();
