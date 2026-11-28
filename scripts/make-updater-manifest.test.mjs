import { test } from "node:test";
import assert from "node:assert/strict";
import { buildManifest, EXPECTED_PLATFORMS } from "./make-updater-manifest.mjs";

const REPO = "zwolf25/second-brain-hub";
const V = "0.21.5";

// Names as tauri-action uploads them for a full three-job release.
function fullRelease() {
  return {
    "Second.Brain.Hub_0.21.5_aarch64.dmg": "dmg",
    "Second.Brain.Hub_aarch64.app.tar.gz": "tar",
    "Second.Brain.Hub_aarch64.app.tar.gz.sig": "SIG-ARM\n",
    "Second.Brain.Hub_x64.app.tar.gz": "tar",
    "Second.Brain.Hub_x64.app.tar.gz.sig": "SIG-INTEL\n",
    "Second.Brain.Hub_0.21.5_x64-setup.exe": "exe",
    "Second.Brain.Hub_0.21.5_x64-setup.exe.sig": "SIG-NSIS\n",
    "Second.Brain.Hub_0.21.5_x64_en-US.msi": "msi",
    "Second.Brain.Hub_0.21.5_x64_en-US.msi.sig": "SIG-MSI\n",
  };
}

// The signature files are the only input; sig contents are the files' text.
function sigsOnly(files) {
  return Object.fromEntries(Object.entries(files).filter(([n]) => n.endsWith(".sig")));
}

test("maps every expected platform to its signed asset", () => {
  const m = buildManifest({ files: sigsOnly(fullRelease()), repo: REPO, version: V, notes: "Notes", pubDate: "2026-11-27T00:00:00.000Z" });
  assert.deepEqual(Object.keys(m.platforms).sort(), [...EXPECTED_PLATFORMS].sort());
  assert.equal(m.version, V);
  assert.equal(m.pub_date, "2026-11-27T00:00:00.000Z");
  assert.equal(m.notes, "Notes");
});

test("signature is the .sig contents, trimmed; url points at the release asset", () => {
  const m = buildManifest({ files: sigsOnly(fullRelease()), repo: REPO, version: V, notes: "", pubDate: "x" });
  assert.equal(m.platforms["darwin-aarch64"].signature, "SIG-ARM");
  assert.equal(m.platforms["darwin-aarch64-app"].signature, "SIG-ARM");
  assert.equal(m.platforms["darwin-x86_64"].signature, "SIG-INTEL");
  assert.equal(m.platforms["windows-x86_64"].signature, "SIG-MSI");
  assert.equal(m.platforms["windows-x86_64-msi"].signature, "SIG-MSI");
  assert.equal(m.platforms["windows-x86_64-nsis"].signature, "SIG-NSIS");
  assert.equal(m.platforms["darwin-aarch64"].url, `https://github.com/${REPO}/releases/download/v${V}/Second.Brain.Hub_aarch64.app.tar.gz`);
  assert.equal(m.platforms["windows-x86_64"].url, `https://github.com/${REPO}/releases/download/v${V}/Second.Brain.Hub_0.21.5_x64_en-US.msi`);
});

test("throws, naming the missing platforms, when an arm64 signature is absent", () => {
  const files = sigsOnly(fullRelease());
  delete files["Second.Brain.Hub_aarch64.app.tar.gz.sig"];
  assert.throws(
    () => buildManifest({ files, repo: REPO, version: V, notes: "", pubDate: "x" }),
    /darwin-aarch64, darwin-aarch64-app/,
  );
});

test("ignores non-updater signatures", () => {
  const files = { ...sigsOnly(fullRelease()), "Other.Tool.zip.sig": "IGNORED" };
  const m = buildManifest({ files, repo: REPO, version: V, notes: "", pubDate: "x" });
  assert.ok(!Object.values(m.platforms).some((p) => p.signature === "IGNORED"));
});
