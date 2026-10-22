#!/usr/bin/env node
// Builds the SearXNG that ships inside Themis: a standalone CPython (python-build-standalone,
// through uv) with a pinned SearXNG and its requirements, trimmed, in
// crates/desktop/searxng-runtime/. Release builds ship that folder as the `searxng` resource
// (`tauri build --config tauri.searxng.conf.json`); the app starts it on a private
// 127.0.0.1 port (crates/desktop/src/state/searxng.rs).
//
// Usage: node scripts/bundle-searxng.mjs [--if-missing]
// Needs uv and git on PATH. macOS and Linux only: SearXNG needs the Unix `pwd` module, so
// Windows builds ship without it (CI skips this step there).

import { execFileSync } from "node:child_process";
import { randomBytes } from "node:crypto";
import { cpSync, existsSync, lstatSync, mkdtempSync, readdirSync, readFileSync, readlinkSync, realpathSync, rmSync, statSync, symlinkSync, unlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, isAbsolute, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

// Pinned so every build ships the same SearXNG; bump deliberately and re-run the live check.
const SEARXNG_COMMIT = "81371861f67226091d1c4df2e3908a11d3b54eb9";
const PYTHON = "3.12";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const out = join(root, "crates", "desktop", "searxng-runtime");
const windows = process.platform === "win32";

if (process.argv.includes("--if-missing") && existsSync(join(out, "VERSION"))) {
  const version = readFileSync(join(out, "VERSION"), "utf8");
  if (version.includes(SEARXNG_COMMIT)) {
    console.log(`searxng-runtime is current (${SEARXNG_COMMIT.slice(0, 7)})`);
    process.exit(0);
  }
}

const run = (program, args, options = {}) => {
  console.log(`$ ${program} ${args.join(" ")}`);
  execFileSync(program, args, { stdio: "inherit", ...options });
};

const work = mkdtempSync(join(tmpdir(), "themis-searxng-"));
try {
  // 1. A relocatable CPython of our own; nothing from the build machine is used at runtime.
  const pythons = join(work, "pythons");
  run("uv", ["python", "install", PYTHON, "--install-dir", pythons, "--no-bin"]);
  const installed = readdirSync(pythons).find(name => name.startsWith(`cpython-${PYTHON}`));
  if (!installed) throw new Error("uv did not install CPython");
  rmSync(out, { recursive: true, force: true });
  // uv's "cpython-3.12-..." entry can be an alias link to the full version's folder: copy
  // the real folder, never a link back into this temporary directory.
  cpSync(realpathSync(join(pythons, installed)), join(out, "python"), { recursive: true, verbatimSymlinks: true });
  // Links inside the copy (bin/python3 -> python3.12, ...) must stay inside the bundle.
  const realPython = realpathSync(join(pythons, installed));
  const relink = dir => {
    for (const name of readdirSync(dir)) {
      const path = join(dir, name);
      const link = lstatSync(path);
      if (link.isSymbolicLink()) {
        const target = readlinkSync(path);
        const absolute = resolve(dir, target);
        const inside = absolute.startsWith(realPython) ? join(out, "python", relative(realPython, absolute)) : absolute;
        if (absolute.startsWith(realPython) || isAbsolute(target)) {
          unlinkSync(path);
          symlinkSync(relative(dir, inside), path);
        }
      } else if (link.isDirectory()) {
        relink(path);
      }
    }
  };
  relink(join(out, "python"));
  const python = windows ? join(out, "python", "python.exe") : join(out, "python", "bin", "python3");

  // 2. SearXNG at the pinned commit, its requirements first (its setup imports them).
  const source = join(work, "searxng");
  run("git", ["init", "-q", source]);
  run("git", ["-C", source, "fetch", "-q", "--depth", "1", "https://github.com/searxng/searxng", SEARXNG_COMMIT]);
  // utils/ holds server config templates with ':' in their names, which Windows cannot check
  // out; SearXNG does not need them to run. The sparse checkout never writes them, but Git on
  // Windows refuses any tree that lists such a path unless its NTFS check is off for this step.
  run("git", ["-C", source, "sparse-checkout", "set", "--no-cone", "/*", "!/utils/"]);
  run("git", ["-C", source, "-c", "core.protectNTFS=false", "checkout", "-q", "FETCH_HEAD"]);
  const pip = ["pip", "install", "--python", python, "--break-system-packages", "--no-cache"];
  run("uv", [...pip, "-r", join(source, "requirements.txt"), "msgspec", "setuptools", "wheel", "pyyaml", "typing_extensions"]);
  run("uv", [...pip, "--no-build-isolation", source]);

  // 3. Trim what never runs: tests, caches, the Tk GUI, IDLE, pip's own copy.
  let removed = 0;
  const prune = dir => {
    for (const name of readdirSync(dir)) {
      const path = join(dir, name);
      if (!statSync(path, { throwIfNoEntry: false })?.isDirectory()) continue;
      if (["__pycache__", "test", "tests", "idlelib", "tkinter", "turtledemo", "ensurepip", "pip"].includes(name)) {
        rmSync(path, { recursive: true, force: true });
        removed++;
      } else {
        prune(path);
      }
    }
  };
  prune(join(out, "python"));
  // The Tk GUI toolkit, C headers and build-only packages are never used at runtime.
  const lib = join(out, "python", "lib");
  for (const name of existsSync(lib) ? readdirSync(lib) : []) {
    if (/^(tcl|tk|itcl|thread)\d/.test(name) || /^lib(tcl|tk)/.test(name)) rmSync(join(lib, name), { recursive: true, force: true });
  }
  rmSync(join(out, "python", "include"), { recursive: true, force: true });
  const sitePackages = windows ? join(out, "python", "Lib", "site-packages") : join(lib, `python${PYTHON}`, "site-packages");
  for (const name of existsSync(sitePackages) ? readdirSync(sitePackages) : []) {
    if (/^(setuptools|wheel|_distutils_hack|pip)([-.]|$)/.test(name) || name === "distutils-precedence.pth") rmSync(join(sitePackages, name), { recursive: true, force: true });
  }
  writeFileSync(join(out, "VERSION"), `searxng ${SEARXNG_COMMIT}\npython ${installed}\n`);

  // 4. It must load: import the app with a throwaway settings file (a real secret, as
  // SearXNG refuses its placeholder one).
  const settings = join(work, "settings.yml");
  writeFileSync(settings, `use_default_settings: true\nserver:\n  secret_key: "${randomBytes(32).toString("hex")}"\n  limiter: false\nsearch:\n  formats: [html, json]\n`);
  run(python, ["-I", "-c", "import searx.webapp, sys; print('searxng imports with', sys.version.split()[0])"], {
    env: { ...process.env, SEARXNG_SETTINGS_PATH: settings },
  });
  console.log(`searxng-runtime ready in ${out} (${removed} folders trimmed)`);
} finally {
  rmSync(work, { recursive: true, force: true });
}

// After the build folder is gone: the bundle must still run on its own.
const bundled = windows ? join(out, "python", "python.exe") : join(out, "python", "bin", "python3");
run(bundled, ["-I", "-c", "import searx, sys; print('bundle runs on its own:', sys.executable)"]);
