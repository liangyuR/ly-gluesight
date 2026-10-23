import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

function run(command, args, options = {}) {
  const { codes = [0], ...spawnOptions } = options;
  const result = spawnSync(command, args, {
    maxBuffer: 32 * 1024 * 1024,
    windowsHide: true,
    ...spawnOptions,
  });
  if (result.error) throw result.error;
  if (!codes.includes(result.status)) {
    throw new Error(
      `${command} failed (${result.status ?? result.signal}): ${result.stderr.toString("utf8").trim()}`,
    );
  }
  return result.stdout;
}

function text(bytes) {
  return new TextDecoder("utf-8", { fatal: true }).decode(bytes).replace(/\r\n/g, "\n");
}

function hunks(patch) {
  return Array.from(
    patch.matchAll(/^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/gm),
    (match) => ({
      oldStart: Number(match[1]),
      oldCount: Number(match[2] ?? 1),
      newStart: Number(match[3]),
      newCount: Number(match[4] ?? 1),
    }),
  );
}

function intersects(edit, changed) {
  const first = edit.oldCount ? edit.oldStart : Math.max(1, edit.oldStart);
  const last = edit.oldCount ? edit.oldStart + edit.oldCount - 1 : edit.oldStart + 1;
  return changed.some(([start, end]) => first <= end && last >= start);
}

function main() {
  const options = {};
  const args = process.argv.slice(2);
  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    if (arg === "--base" || arg === "--head") {
      const value = args[++index];
      if (!value || value.startsWith("--")) throw new Error(`Missing value for ${arg}.`);
      options[arg.slice(2)] = value;
    } else throw new Error(`Unknown argument: ${arg}`);
  }
  if (!options.base) {
    throw new Error("Usage: node scripts/Check-RustFormat.mjs --base <commit> [--head <commit>]");
  }
  const root = run("git", ["rev-parse", "--show-toplevel"]).toString("utf8").trim();
  process.chdir(root);
  const commit = (ref) => run("git", ["rev-parse", "--verify", "--end-of-options", `${ref}^{commit}`]).toString("utf8").trim();
  const revisions = [commit(options.base)];
  if (options.head) revisions.push(commit(options.head));
  const cargo = text(options.head
    ? run("git", ["show", `${revisions[1]}:src-tauri/Cargo.toml`])
    : readFileSync("src-tauri/Cargo.toml"));
  const edition = cargo.match(/^edition\s*=\s*"([^"]+)"/m)?.[1];
  if (!edition) throw new Error("Missing Rust edition in src-tauri/Cargo.toml.");
  const diffArgs = ["diff", "--no-ext-diff", "--no-textconv", "--no-renames", "--no-color"];
  const entries = run("git", [...diffArgs, "--name-status", "-z", "--diff-filter=ACMT", ...revisions, "--", "*.rs"])
    .toString("utf8").split("\0");
  entries.pop();
  if (entries.length % 2) throw new Error("Unexpected git name-status output.");
  const temporary = mkdtempSync(join(tmpdir(), "gluesight-rust-format-"));
  let checked = 0;
  let failed = 0;
  try {
    for (let index = 0; index < entries.length; index += 2) {
      const [status, file] = entries.slice(index, index + 2);
      const original = options.head
        ? run("git", ["show", `${revisions[1]}:${file}`])
        : readFileSync(file);
      const source = text(original);
      let formatted;
      try {
        formatted = text(run("rustfmt", ["--emit", "stdout", "--edition", edition, "--config", "skip_children=true"], { input: source }));
      } catch (error) {
        throw new Error(`${file}: ${error.message}`);
      }
      checked += 1;
      const changed = hunks(text(run("git", [...diffArgs, "--unified=0", ...revisions, "--", file])))
        .filter((hunk) => hunk.newCount > 0)
        .map((hunk) => [hunk.newStart, hunk.newStart + hunk.newCount - 1]);
      if (status !== "A" && changed.length === 0) continue;
      const before = join(temporary, "before.rs");
      const after = join(temporary, "after.rs");
      writeFileSync(before, source);
      writeFileSync(after, formatted);
      const edits = hunks(text(run("git", [
        "-c", "core.autocrlf=false", "diff", "--no-index", "--no-ext-diff", "--no-textconv", "--no-color", "--unified=0", "--", before, after,
      ], { codes: [0, 1] }))).filter((edit) => status === "A" || intersects(edit, changed));
      if (!edits.length) continue;
      failed += 1;
      for (const edit of edits) console.error(`${file}:${Math.max(1, edit.oldStart)}: rustfmt change intersects added or modified lines.`);
    }
  } finally {
    rmSync(temporary, { recursive: true, force: true });
  }
  console.log(`Rustfmt parsed ${checked} changed Rust files; ${failed} files need incremental formatting.`);
  if (failed) process.exitCode = 1;
}

try {
  main();
} catch (error) {
  console.error(error.message);
  process.exitCode = 1;
}
