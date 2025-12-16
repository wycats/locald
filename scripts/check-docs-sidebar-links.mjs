import fs from "node:fs";
import path from "node:path";

const repoRoot = path.resolve(
  path.dirname(new URL(import.meta.url).pathname),
  ".."
);
const configPath = path.join(repoRoot, "locald-docs", "astro.config.mjs");

function normalizeLink(link) {
  if (link === "/") return "/";
  if (!link.startsWith("/")) link = `/${link}`;
  return link.endsWith("/") ? link : `${link}/`;
}

function main() {
  const source = fs.readFileSync(configPath, "utf8");

  const linkRegex = /\blink\s*:\s*['"]([^'"]+)['"]/g;
  const seen = new Map();
  const dups = new Map();

  for (const match of source.matchAll(linkRegex)) {
    const raw = match[1];
    const normalized = normalizeLink(raw);

    const firstIndex = seen.get(normalized);
    if (firstIndex === undefined) {
      seen.set(normalized, match.index ?? -1);
      continue;
    }

    const entries = dups.get(normalized) ?? [firstIndex];
    entries.push(match.index ?? -1);
    dups.set(normalized, entries);
  }

  if (dups.size > 0) {
    process.stderr.write(
      `Duplicate sidebar links detected in ${path.relative(
        repoRoot,
        configPath
      )}\n\n`
    );
    for (const [link, positions] of dups.entries()) {
      process.stderr.write(
        `- ${link} (occurrences: ${positions.length + 1})\n`
      );
    }
    process.stderr.write(
      "\nEach docs route should appear in exactly one sidebar group.\n"
    );
    process.exit(1);
  }

  process.stdout.write("Docs sidebar links: OK\n");
}

main();
