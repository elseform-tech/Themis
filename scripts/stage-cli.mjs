#!/usr/bin/env node
// Builds the console `themis` CLI (crates/desktop/src/bin/themis.rs) in release and stages it as
// crates/desktop/binaries/themis-cli-<target triple>[.exe], which the Windows bundle ships next to
// the app (`bundle.externalBin` in crates/desktop/tauri.windows.conf.json; Tauri runs this as
// part of `beforeBuildCommand`). The app copies it to `agent-cli\themis.exe` in its data folder
// and puts that folder first on every agent run's PATH (crates/desktop/src/server/agent_path.rs).
// macOS and Linux need no separate CLI: the desktop app acts as `themis` through a link.
//
// Usage: node scripts/stage-cli.mjs   (honours TAURI_ENV_TARGET_TRIPLE and CARGO_TARGET_DIR)

import { execFileSync } from "node:child_process";
import { copyFileSync, mkdirSync, statSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const host = execFileSync("rustc", ["-vV"], { encoding: "utf8" }).match(/^host: (\S+)$/m)[1];
const triple = process.env.TAURI_ENV_TARGET_TRIPLE || host;
const suffix = triple.includes("windows") ? ".exe" : "";
const targetDir = process.env.CARGO_TARGET_DIR ? resolve(process.env.CARGO_TARGET_DIR) : join(root, "target");

const args = ["build", "--release", "-p", "themis-desktop", "--bin", "themis"];
if (triple !== host) args.push("--target", triple);
console.log(`$ cargo ${args.join(" ")}`);
execFileSync("cargo", args, { cwd: root, stdio: "inherit" });

const built = join(targetDir, ...(triple !== host ? [triple] : []), "release", `themis${suffix}`);
if (!(statSync(built).size > 0)) throw new Error(`${built} is empty`);
const out = join(root, "crates", "desktop", "binaries");
mkdirSync(out, { recursive: true });
const staged = join(out, `themis-cli-${triple}${suffix}`);
copyFileSync(built, staged);
console.log(`staged ${staged} (${statSync(staged).size} bytes)`);
