import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

function posixify(p) {
  return p.split(path.sep).join(path.posix.sep);
}

function walk(dir) {
  const out = [];
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) out.push(...walk(full));
    else out.push(full);
  }
  return out;
}

function ensureDir(dir) {
  fs.mkdirSync(dir, { recursive: true });
}

function copyIfChanged(src, dst) {
  const srcBytes = fs.readFileSync(src);
  const dstExists = fs.existsSync(dst);
  const dstBytes = dstExists ? fs.readFileSync(dst) : null;

  if (!dstExists || !dstBytes.equals(srcBytes)) {
    ensureDir(path.dirname(dst));
    fs.writeFileSync(dst, srcBytes);
    return true;
  }
  return false;
}

function main() {
  const here = path.dirname(fileURLToPath(import.meta.url));
  const docsRoot = path.resolve(here, "..", "src", "content", "docs");
  const repoRoot = path.resolve(here, "..", "..");

  const sourceRoot = path.join(repoRoot, "docs", "design", "axioms");
  const destRoot = path.join(docsRoot, "internals", "axioms");

  if (!fs.existsSync(sourceRoot)) {
    process.stderr.write(
      `sync-axioms: missing source root: ${posixify(sourceRoot)}\n`
    );
    process.exit(1);
  }

  const sources = walk(sourceRoot).filter((p) => p.endsWith(".md"));
  let copied = 0;

  for (const src of sources) {
    const rel = path.relative(sourceRoot, src);
    const dst = path.join(destRoot, rel);
    if (copyIfChanged(src, dst)) copied += 1;
  }

  if (copied > 0) {
    process.stdout.write(`sync-axioms: updated ${copied} file(s)\n`);
  } else {
    process.stdout.write("sync-axioms: no changes\n");
  }
}

main();
