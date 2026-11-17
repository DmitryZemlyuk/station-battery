#!/usr/bin/env node
// Station Battery updater: downloads the latest GitHub release and replaces the app.
//
//   node update.mjs [folder] [--force] [--dry-run] [--no-start]
//
// --force     reinstall even if the latest version is already there
// --dry-run   only show what would be downloaded
// --no-start  don't start the app afterwards
//
// Files downloaded by Node don't get the "downloaded from the internet" mark
// (Mark of the Web on Windows, quarantine on macOS), so the new version starts
// without the SmartScreen / Gatekeeper prompt.
//
// Windows: keeps the app as <folder>\StationBattery.exe. The name never changes, so
//          the "Start with the system" entry keeps pointing at the right file.
// macOS:   keeps the app as <folder>/Station Battery.app (default: ~/Applications).
// Needs Node 18+ (built-in fetch). No packages.

import { createWriteStream, existsSync, readFileSync, writeFileSync, renameSync, rmSync, mkdirSync, statSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { homedir, tmpdir } from "node:os";
import { fileURLToPath } from "node:url";
import { spawn, spawnSync } from "node:child_process";
import { Readable } from "node:stream";
import { pipeline } from "node:stream/promises";

const REPO = "DmitryZemlyuk/station-battery";
const args = process.argv.slice(2);
const force = args.includes("--force");
const dryRun = args.includes("--dry-run");
const platform = process.env.SB_PLATFORM || process.platform; // override for testing only
const isWin = platform === "win32";
const isMac = platform === "darwin";

if (!isWin && !isMac) {
  console.error(`Unsupported platform: ${platform}. Station Battery runs on Windows and macOS.`);
  process.exit(1);
}

const here = dirname(fileURLToPath(import.meta.url));
const folderArg = args.find((a) => !a.startsWith("--"));
const folder = resolve(folderArg || (isMac ? join(homedir(), "Applications") : here));
const target = isWin ? join(folder, "StationBattery.exe") : join(folder, "Station Battery.app");
const stamp = join(folder, ".station-battery-version");

function sh(cmd, cmdArgs) {
  return spawnSync(cmd, cmdArgs, { encoding: "utf8", windowsHide: true });
}

function isRunning() {
  if (isWin) {
    // Any copy: also one started from a downloaded file like StationBattery-1.2.6-windows-x64.exe
    // (with only one instance allowed, the new version would otherwise just exit).
    const r = sh("tasklist", ["/FI", "IMAGENAME eq StationBattery*", "/NH"]);
    return /StationBattery[^\s]*\.exe/i.test(r.stdout || "");
  }
  return sh("pgrep", ["-f", "Station Battery.app/Contents/MacOS/"]).status === 0;
}

async function stopApp() {
  if (!isRunning()) return false;
  console.log("Closing the running app…");
  if (isWin) sh("taskkill", ["/IM", "StationBattery*", "/F"]);
  else sh("osascript", ["-e", 'quit app "Station Battery"']);
  for (let i = 0; i < 50 && isRunning(); i++) await new Promise((r) => setTimeout(r, 100));
  if (isRunning() && isMac) sh("pkill", ["-f", "Station Battery.app/Contents/MacOS/"]);
  return true;
}

function startApp() {
  const child = isWin
    ? spawn(target, [], { detached: true, stdio: "ignore", windowsHide: false })
    : spawn("open", [target], { detached: true, stdio: "ignore" });
  child.on("error", (e) => console.error(`Couldn't start the app: ${e.message}`));
  child.unref();
}

async function main() {
  // The latest version comes from the redirect of /releases/latest, so there is no
  // GitHub API call (and no API rate limit).
  const res = await fetch(`https://github.com/${REPO}/releases/latest`, {
    headers: { "User-Agent": "station-battery-updater" },
    redirect: "manual",
  });
  const location = res.headers.get("location") || "";
  const tag = (location.match(/\/releases\/tag\/([^/?#]+)/) || [])[1];
  if (!tag) throw new Error(`Couldn't find the latest release (GitHub answered ${res.status}).`);
  const version = decodeURIComponent(tag).replace(/^v/, "");
  const name = `StationBattery-${version}${isWin ? "-windows-x64.exe" : "-macos-universal.zip"}`;
  const asset = { name, url: `https://github.com/${REPO}/releases/download/${tag}/${name}` };

  const installed = existsSync(stamp) && existsSync(target) ? readFileSync(stamp, "utf8").trim() : "";
  console.log(`Latest: ${version}. Installed: ${installed || "none"}. Folder: ${folder}`);
  if (installed === version && !force) {
    console.log("Already up to date. Use --force to reinstall.");
    return;
  }
  if (dryRun) {
    console.log(`Would download ${asset.url} to ${target}`);
    return;
  }

  mkdirSync(folder, { recursive: true });
  const tmp = isWin ? join(folder, "StationBattery.exe.download") : join(tmpdir(), asset.name);
  console.log(`Downloading ${asset.name}…`);
  const dl = await fetch(asset.url, { headers: { "User-Agent": "station-battery-updater" } });
  if (!dl.ok || !dl.body) throw new Error(`Download failed: ${dl.status} ${dl.statusText}`);
  const size = Number(dl.headers.get("content-length") || 0);
  await pipeline(Readable.fromWeb(dl.body), createWriteStream(tmp));
  // A truncated download must never replace a working app.
  const got = statSync(tmp).size;
  if (size && got !== size) {
    rmSync(tmp, { force: true });
    throw new Error(`Download incomplete (${got} of ${size} bytes).`);
  }

  const wasRunning = await stopApp();
  if (isWin) {
    // A running exe can't be overwritten; it was stopped above. Retry briefly in case
    // Windows still holds the file for a moment after the process exits.
    for (let i = 0; ; i++) {
      try {
        renameSync(tmp, target);
        break;
      } catch (e) {
        if (i >= 20) throw new Error(`Couldn't replace ${target}: ${e.message}`);
        await new Promise((r) => setTimeout(r, 250));
      }
    }
  } else {
    rmSync(target, { recursive: true, force: true });
    const r = sh("ditto", ["-x", "-k", tmp, folder]);
    rmSync(tmp, { force: true });
    if (r.status !== 0) throw new Error(`Unpacking failed: ${r.stderr}`);
  }
  writeFileSync(stamp, version + "\n");
  console.log(`Installed ${version}: ${target}`);

  if (!args.includes("--no-start")) {
    startApp();
    console.log(wasRunning ? "Restarted." : "Started.");
  }
}

main().catch((e) => {
  console.error(`Update failed: ${e.message}`);
  process.exit(1);
});
