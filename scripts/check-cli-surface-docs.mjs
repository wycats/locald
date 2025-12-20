import fs from "node:fs";
import path from "node:path";

const repoRoot = path.resolve(path.dirname(new URL(import.meta.url).pathname), "..");

const manifestPath = path.join(repoRoot, "docs", "surface", "cli-manifest.json");

function readManifest() {
  const raw = fs.readFileSync(manifestPath, "utf8");
  return JSON.parse(raw);
}

function* walk(dir) {
  const entries = fs.readdirSync(dir, { withFileTypes: true });
  for (const entry of entries) {
    const fullPath = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      yield* walk(fullPath);
    } else {
      yield fullPath;
    }
  }
}

function isDocsFile(filePath) {
  return (
    filePath.endsWith(".md") ||
    filePath.endsWith(".mdx") ||
    filePath.endsWith(".markdown")
  );
}

function stripPrompt(line) {
  return line.replace(/^\s*\$\s+/, "");
}

function stripComment(line) {
  // Remove '#' comments, but only when outside quotes.
  let inSingle = false;
  let inDouble = false;
  for (let i = 0; i < line.length; i++) {
    const ch = line[i];
    if (ch === "\\" && i + 1 < line.length) {
      i++;
      continue;
    }
    if (!inDouble && ch === "'") inSingle = !inSingle;
    else if (!inSingle && ch === '"') inDouble = !inDouble;
    else if (!inSingle && !inDouble && ch === "#") {
      return line.slice(0, i).trimEnd();
    }
  }
  return line;
}

function shlex(line) {
  const tokens = [];
  let cur = "";
  let inSingle = false;
  let inDouble = false;

  const push = () => {
    if (cur.length > 0) tokens.push(cur);
    cur = "";
  };

  for (let i = 0; i < line.length; i++) {
    const ch = line[i];

    if (ch === "\\" && !inSingle && i + 1 < line.length) {
      cur += line[i + 1];
      i++;
      continue;
    }

    if (!inDouble && ch === "'") {
      inSingle = !inSingle;
      continue;
    }
    if (!inSingle && ch === '"') {
      inDouble = !inDouble;
      continue;
    }

    if (!inSingle && !inDouble && /\s/.test(ch)) {
      push();
      continue;
    }

    cur += ch;
  }

  push();
  return tokens;
}

function indexCommandNode(node) {
  const children = new Map();
  for (const child of node.subcommands ?? []) {
    for (const name of [child.name, ...(child.aliases ?? [])]) {
      children.set(name, child);
    }
  }

  const longFlags = new Set();
  const shortFlags = new Set();

  for (const arg of node.args ?? []) {
    if (arg.positional) continue;
    if (arg.long) longFlags.add(arg.long);
    for (const a of arg.aliases ?? []) longFlags.add(a);
    if (arg.short) shortFlags.add(arg.short);
  }

  return { node, children, longFlags, shortFlags };
}

function buildIndex(root) {
  // Extract global flags from the root.
  const globalLong = new Set();
  const globalShort = new Set();

  for (const arg of root.args ?? []) {
    if (!arg.global) continue;
    if (arg.positional) continue;
    if (arg.long) globalLong.add(arg.long);
    for (const a of arg.aliases ?? []) globalLong.add(a);
    if (arg.short) globalShort.add(arg.short);
  }

  return { globalLong, globalShort };
}

function findCommand(root, tokens) {
  // Greedy descent through the command tree.
  let current = indexCommandNode(root);
  const consumed = [];

  while (tokens.length > 0) {
    const next = tokens[0];
    const child = current.children.get(next);
    if (!child) break;
    consumed.push(next);
    tokens.shift();
    current = indexCommandNode(child);
  }

  return { command: current, consumedPath: consumed };
}

function parseFlags(tokens) {
  const flags = [];
  for (const tok of tokens) {
    if (tok === "--") break;
    if (!tok.startsWith("-")) break;
    if (tok.startsWith("--")) {
      const name = tok.slice(2).split("=")[0];
      if (name.length > 0) flags.push({ kind: "long", name });
      continue;
    }
    if (tok.startsWith("-") && tok !== "-") {
      const cluster = tok.slice(1);
      for (const ch of cluster) {
        flags.push({ kind: "short", name: ch });
      }
    }
  }
  return flags;
}

function extractLocaldInvocations(filePath) {
  const content = fs.readFileSync(filePath, "utf8");
  const lines = content.split(/\r?\n/);

  const invocations = [];

  let inFence = false;
  let fenceLang = "";

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];

    const fenceMatch = line.match(/^```\s*([^\s]*)\s*$/);
    if (fenceMatch) {
      if (!inFence) {
        inFence = true;
        fenceLang = (fenceMatch[1] ?? "").toLowerCase();
      } else {
        inFence = false;
        fenceLang = "";
      }
      continue;
    }

    if (!inFence) continue;

    // Only lint shell-ish fences to avoid false positives.
    if (
      fenceLang &&
      ![
        "bash",
        "sh",
        "shell",
        "zsh",
        "console",
        "terminal",
        "",
      ].includes(fenceLang)
    ) {
      continue;
    }

    const cleaned = stripComment(stripPrompt(line)).trim();
    if (cleaned.length === 0) continue;

    const tokens = shlex(cleaned);
    if (tokens.length === 0) continue;

    // Skip leading env assignments.
    while (tokens.length > 0 && /^[A-Za-z_][A-Za-z0-9_]*=/.test(tokens[0])) {
      tokens.shift();
    }

    if (tokens[0] === "sudo") tokens.shift();

    if (tokens[0] !== "locald") continue;

    invocations.push({
      filePath,
      line: i + 1,
      tokens,
    });
  }

  return invocations;
}

function validateInvocation(manifestRoot, globalIndex, inv) {
  // Clone tokens so we can shift.
  const tokens = [...inv.tokens];

  // consume locald
  tokens.shift();

  // Find the deepest matching command path.
  const { command, consumedPath } = findCommand(manifestRoot, tokens);

  // If the next token looks like a subcommand but doesn't match, fail.
  if (tokens.length > 0 && !tokens[0].startsWith("-")) {
    // Not a flag; could be a positional arg, but only root supports direct positionals in this CLI.
    // For taught docs, we want explicit commands, so treat this as suspicious.
    // However, allow `locald` alone.
    if (consumedPath.length === 0) {
      // `locald <something>` at root level.
      return {
        ok: false,
        message: `Unknown command at root: ${tokens[0]}`,
      };
    }
  }

  if (command.node.hidden) {
    return {
      ok: false,
      message: `Command is hidden/internal: ${consumedPath.join(" ") || "<root>"}`,
    };
  }

  const allowedLong = new Set([...globalIndex.globalLong, ...command.longFlags]);
  const allowedShort = new Set([...globalIndex.globalShort, ...command.shortFlags]);

  const flags = parseFlags(tokens);
  for (const f of flags) {
    if (f.kind === "long") {
      if (!allowedLong.has(f.name)) {
        return {
          ok: false,
          message: `Unknown flag --${f.name} for command: ${consumedPath.join(" ") || "<root>"}`,
        };
      }
    } else {
      if (!allowedShort.has(f.name)) {
        return {
          ok: false,
          message: `Unknown flag -${f.name} for command: ${consumedPath.join(" ") || "<root>"}`,
        };
      }
    }
  }

  return { ok: true };
}

function main() {
  if (!fs.existsSync(manifestPath)) {
    process.stderr.write(`Missing CLI manifest at ${path.relative(repoRoot, manifestPath)}\n`);
    process.stderr.write("Run: cargo run -p locald-cli -- __surface cli-manifest > docs/surface/cli-manifest.json\n");
    process.exit(1);
  }

  const manifest = readManifest();
  const root = manifest.root;
  const globalIndex = buildIndex(root);

  const targets = [
    path.join(repoRoot, "README.md"),
    path.join(repoRoot, "locald-docs", "src", "content", "docs"),
  ];

  const files = [];
  for (const t of targets) {
    if (!fs.existsSync(t)) continue;
    const stat = fs.statSync(t);
    if (stat.isDirectory()) {
      for (const file of walk(t)) {
        if (isDocsFile(file)) files.push(file);
      }
    } else {
      if (isDocsFile(t)) files.push(t);
    }
  }

  const errors = [];

  for (const filePath of files) {
    const invocations = extractLocaldInvocations(filePath);
    for (const inv of invocations) {
      const res = validateInvocation(root, globalIndex, inv);
      if (!res.ok) {
        errors.push({
          filePath,
          line: inv.line,
          message: res.message,
          snippet: inv.tokens.join(" "),
        });
      }
    }
  }

  if (errors.length > 0) {
    process.stderr.write("CLI surface docs lint failed:\n\n");
    for (const e of errors) {
      process.stderr.write(
        `- ${path.relative(repoRoot, e.filePath)}:${e.line}: ${e.message}\n  ${e.snippet}\n`
      );
    }
    process.stderr.write(
      "\nFix the docs, or update the CLI manifest if the CLI surface changed.\n"
    );
    process.exit(1);
  }

  process.stdout.write("CLI surface docs lint: OK\n");
}

main();
