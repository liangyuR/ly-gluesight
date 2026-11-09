import { execFileSync } from "node:child_process";
import { appendFileSync, readFileSync } from "node:fs";
import { pathToFileURL } from "node:url";

export function selectValidation(paths, event) {
  return {
    required: event === "workflow_dispatch" || paths.some((path) => path !== "README.md" && !path.startsWith("docs/")),
    rustFormat: paths.some((path) => path.endsWith(".rs")),
  };
}

function main() {
  const git = (...args) => execFileSync("git", args, { encoding: "utf8" }).trim();
  const head = process.env.GITHUB_SHA;
  let base = process.env.FORMAT_BASE;
  if (!base || /^0+$/.test(base)) base = git("rev-parse", `${head}^`);
  const paths = execFileSync("git", ["diff", "--no-renames", "--name-only", "-z", base, head], { encoding: "utf8" }).split("\0").filter(Boolean);
  const plan = selectValidation(paths, process.env.GITHUB_EVENT_NAME);
  const pkg = JSON.parse(readFileSync("package.json", "utf8"));
  const toolchain = readFileSync("rust-toolchain.toml", "utf8");
  const rust = toolchain.match(/^channel\s*=\s*"(\d+\.\d+\.\d+)"\s*$/m)?.[1];
  if (!rust) throw new Error("Rust toolchain must pin a complete version.");
  const outputs = {
    required: plan.required,
    "rust-format": plan.rustFormat,
    base,
    node: pkg.engines.node.replace(/^>=/, ""),
    pnpm: pkg.packageManager.replace(/^pnpm@/, ""),
    rust,
  };
  appendFileSync(process.env.GITHUB_OUTPUT, Object.entries(outputs).map(([key, value]) => `${key}=${value}\n`).join(""));
  console.log(`Validation required: ${plan.required}; changed paths: ${paths.length}`);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) main();
