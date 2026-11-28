// Builds the updater's latest.json from a release's signed assets.
// Run once, after every platform job has uploaded its installers, so no two
// jobs race to write the same manifest (the 0.21.4 failure, BACKLOG TD-16).
//
// Usage: node scripts/make-updater-manifest.mjs <assets-dir> <owner/repo> <version> <notes-file>
// Prints the manifest JSON to stdout. Exits 1 if any expected platform has no signed asset.
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

// Asset name suffix -> updater platform keys it serves. Names come from tauri-action's output.
export const RULES = [
  [/_aarch64\.app\.tar\.gz$/, ["darwin-aarch64", "darwin-aarch64-app"]],
  [/_x64\.app\.tar\.gz$/, ["darwin-x86_64", "darwin-x86_64-app"]],
  [/_x64-setup\.exe$/, ["windows-x86_64-nsis"]],
  [/_x64_en-US\.msi$/, ["windows-x86_64", "windows-x86_64-msi"]],
];

export const EXPECTED_PLATFORMS = [
  "darwin-aarch64", "darwin-aarch64-app", "darwin-x86_64", "darwin-x86_64-app",
  "windows-x86_64", "windows-x86_64-msi", "windows-x86_64-nsis",
];

// `files` maps each file name in the release to its contents (signatures are read as text).
export function buildManifest({ files, repo, version, notes, pubDate }) {
  const platforms = {};
  for (const [name, contents] of Object.entries(files)) {
    if (!name.endsWith(".sig")) continue;
    const asset = name.slice(0, -".sig".length);
    const rule = RULES.find(([re]) => re.test(asset));
    if (!rule) continue;
    const entry = {
      signature: contents.trim(),
      url: `https://github.com/${repo}/releases/download/v${version}/${asset}`,
    };
    for (const key of rule[1]) platforms[key] = entry;
  }
  const missing = EXPECTED_PLATFORMS.filter((k) => !platforms[k]);
  if (missing.length) throw new Error(`no signed asset for: ${missing.join(", ")}`);
  return { version, notes: notes.trim(), pub_date: pubDate, platforms };
}

function main([dir, repo, version, notesFile]) {
  if (!dir || !repo || !version || !notesFile) {
    console.error("usage: make-updater-manifest.mjs <assets-dir> <owner/repo> <version> <notes-file>");
    process.exit(2);
  }
  const files = {};
  for (const name of readdirSync(dir)) {
    if (name.endsWith(".sig")) files[name] = readFileSync(join(dir, name), "utf8");
  }
  try {
    const manifest = buildManifest({
      files, repo, version,
      notes: readFileSync(notesFile, "utf8"),
      pubDate: new Date().toISOString(),
    });
    process.stdout.write(JSON.stringify(manifest, null, 2) + "\n");
  } catch (e) {
    console.error(`manifest not written: ${e.message}`);
    process.exit(1);
  }
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) main(process.argv.slice(2));
