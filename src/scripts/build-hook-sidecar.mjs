#!/usr/bin/env node
import { copyFileSync, mkdirSync, chmodSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { execFileSync } from "node:child_process";

const __dirname = dirname(fileURLToPath(import.meta.url));
const workspaceDir = resolve(__dirname, "..");
const desktopDir = join(workspaceDir, "desktop");
const release = process.argv.includes("--release");
const profile = release ? "release" : "debug";
const cargoTarget = process.env.CARGO_BUILD_TARGET || process.env.TARGET || "";
const targetDir = process.env.CARGO_TARGET_DIR
  ? resolve(process.env.CARGO_TARGET_DIR)
  : join(workspaceDir, "target");

const targetTriple = detectTargetTriple();
const exeSuffix = process.platform === "win32" ? ".exe" : "";

execFileSync(
  "cargo",
  [
    "build",
    "-p",
    "vibearound-hook",
    ...(release ? ["--release"] : []),
    ...(cargoTarget ? ["--target", cargoTarget] : []),
  ],
  { cwd: workspaceDir, stdio: "inherit" },
);

const source = cargoTarget
  ? join(targetDir, cargoTarget, profile, `vibearound-hook${exeSuffix}`)
  : join(targetDir, profile, `vibearound-hook${exeSuffix}`);
const binariesDir = join(desktopDir, "binaries");
const target = join(binariesDir, `vibearound-hook-${targetTriple}${exeSuffix}`);

mkdirSync(binariesDir, { recursive: true });
copyFileSync(source, target);
if (process.platform !== "win32") {
  chmodSync(target, 0o755);
}

console.log(`[VibeAround] Hook sidecar ready: ${target}`);

function detectTargetTriple() {
  const explicit = process.env.CARGO_BUILD_TARGET || process.env.TARGET;
  if (explicit) return explicit;

  const output = execFileSync("rustc", ["-vV"], {
    cwd: workspaceDir,
    encoding: "utf8",
  });
  const host = output
    .split(/\r?\n/)
    .find((line) => line.startsWith("host:"))
    ?.slice("host:".length)
    .trim();

  if (!host) {
    throw new Error("Could not detect rust target triple from `rustc -vV`.");
  }
  return host;
}
