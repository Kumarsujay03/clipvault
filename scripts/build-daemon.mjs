// Builds clipvaultd and places it where Tauri's bundler expects sidecars
// (src-tauri/binaries/clipvaultd-<target-triple>).
import { execFileSync } from "node:child_process";
import { copyFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const profile = process.argv[2] === "release" ? "release" : "debug";
const root = join(dirname(fileURLToPath(import.meta.url)), "..", "src-tauri");
const triple = execFileSync("rustc", ["-vV"], { encoding: "utf8" }).match(/host: (\S+)/)[1];
const args = ["build", "-p", "clipvault-core", "--bin", "clipvaultd"];
if (profile === "release") args.push("--release");
execFileSync("cargo", args, { cwd: root, stdio: "inherit" });
mkdirSync(join(root, "binaries"), { recursive: true });
copyFileSync(join(root, "target", profile, "clipvaultd"), join(root, "binaries", `clipvaultd-${triple}`));
console.log(`clipvaultd (${profile}) -> binaries/clipvaultd-${triple}`);
