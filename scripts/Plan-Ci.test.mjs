import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const planner = fileURLToPath(new URL("./Plan-Ci.mjs", import.meta.url));

for (const { name, event = "pull_request", change, required } of [
  { name: "只有 README 和文档变更 → 跳过校验", required: false,
    change: ({ write }) => { write("README.md", "updated\n"); write("docs/guide.md", "guide\n"); } },
  { name: "文档与代码混合变更 → 必须校验", required: true,
    change: ({ write }) => { write("README.md", "updated\n"); write("src/example.rs", "fn changed() {}\n"); } },
  { name: "代码移动到 docs → 删除的代码仍触发校验", required: true,
    change: ({ git, write }) => { write("docs/guide.md", "guide\n"); git("mv", "src/example.rs", "docs/example.rs"); } },
  { name: "一次 push 含代码和后续文档提交 → 校验完整提交范围", event: "push", required: true,
    change: ({ write, commit }) => { write("src/example.rs", "fn changed() {}\n"); commit(); write("README.md", "updated\n"); } },
  { name: "手动运行只有文档变更 → 必须校验", event: "workflow_dispatch", required: true,
    change: ({ write }) => { write("docs/guide.md", "guide\n"); } },
]) {
  test(name, () => {
    const root = mkdtempSync(join(tmpdir(), "gluesight-ci-plan-"));
    const git = (...args) => execFileSync("git", args, { cwd: root, encoding: "utf8" }).trim();
    const write = (path, content) => {
      const target = join(root, path);
      mkdirSync(dirname(target), { recursive: true });
      writeFileSync(target, content);
    };
    const commit = () => { git("add", "--all"); git("-c", "commit.gpgsign=false", "commit", "-qm", "fixture"); };
    try {
      git("init", "--quiet");
      git("config", "core.autocrlf", "false");
      git("config", "user.name", "CI routing test");
      git("config", "user.email", "ci-routing@example.invalid");
      write("package.json", JSON.stringify({ engines: { node: ">=24.15.0" }, packageManager: "pnpm@12.15.0" }));
      write("rust-toolchain.toml", '[toolchain]\nchannel = "1.99.1"\n');
      write("README.md", "initial\n");
      write("src/example.rs", "fn initial() {}\n");
      commit();
      const base = git("rev-parse", "HEAD");
      change({ git, write, commit });
      commit();
      const output = join(root, "github-output");
      execFileSync(process.execPath, [planner], {
        cwd: root,
        encoding: "utf8",
        env: { ...process.env, GITHUB_SHA: git("rev-parse", "HEAD"), FORMAT_BASE: base,
          GITHUB_EVENT_NAME: event, GITHUB_OUTPUT: output },
      });
      const outputs = Object.fromEntries(readFileSync(output, "utf8").trim().split("\n").map((line) => line.split("=")));
      assert.equal(outputs.required, String(required));
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });
}
