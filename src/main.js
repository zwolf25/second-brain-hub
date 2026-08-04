const { invoke, convertFileSrc } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { open: openDialog } = window.__TAURI__.dialog;
const { openPath, openUrl, revealItemInDir } = window.__TAURI__.opener;
const { check: checkForUpdate } = window.__TAURI__.updater;
const { relaunch } = window.__TAURI__.process;

const onboarding = document.querySelector("#onboarding");
const searchView = document.querySelector("#search-view");
const viewerView = document.querySelector("#viewer-view");
const onboardingError = document.querySelector("#onboarding-error");
const doneSettingsBtn = document.querySelector("#done-settings-btn");
const searchInput = document.querySelector("#search-input");
const statusEl = document.querySelector("#status");
const resultsEl = document.querySelector("#results");
const emptyStateEl = document.querySelector("#empty-state");
const viewerTitleEl = document.querySelector("#viewer-title");
const viewerContentEl = document.querySelector("#viewer-content");

let debounceTimer = null;

// One of these per settings-section (vault/raw/inbox) — same autodetect →
// confirm-or-browse → persist shape, so it's a factory instead of copy-pasted
// three times.
function makePathSection({ label, statusEl, useBtn, browseBtn, autodetectCmd, setCmd, configKey, required }) {
  let detected = null;

  async function refresh() {
    const config = await invoke("get_config");
    const current = config[configKey];
    if (current) {
      statusEl.textContent = `Using: ${current}`;
      useBtn.classList.add("hidden");
      updateDoneVisibility();
      return;
    }
    detected = await invoke(autodetectCmd);
    if (detected) {
      statusEl.textContent = `Found: ${detected}`;
      useBtn.classList.remove("hidden");
    } else {
      statusEl.textContent = required
        ? "Couldn't auto-detect it — browse for the folder instead."
        : `Couldn't auto-detect ${label} — browse for it if you have one, or leave it unset.`;
      useBtn.classList.add("hidden");
    }
  }

  async function trySet(path) {
    if (required) onboardingError.textContent = "";
    try {
      await invoke(setCmd, { path });
      await refresh();
    } catch (e) {
      if (required) onboardingError.textContent = String(e);
      else statusEl.textContent = `Error: ${e}`;
    }
  }

  useBtn.addEventListener("click", () => detected && trySet(detected));
  browseBtn.addEventListener("click", async () => {
    const selected = await openDialog({ directory: true, multiple: false });
    if (selected) trySet(selected);
  });

  return { refresh };
}

const vaultSection = makePathSection({
  label: "the wikis folder",
  statusEl: document.querySelector("#vault-status-msg"),
  useBtn: document.querySelector("#use-vault-autodetect"),
  browseBtn: document.querySelector("#browse-vault-folder"),
  autodetectCmd: "autodetect_vault",
  setCmd: "set_vault_path",
  configKey: "vault_path",
  required: true,
});
const rawSection = makePathSection({
  label: "the raw folder",
  statusEl: document.querySelector("#raw-status-msg"),
  useBtn: document.querySelector("#use-raw-autodetect"),
  browseBtn: document.querySelector("#browse-raw-folder"),
  autodetectCmd: "autodetect_raw",
  setCmd: "set_raw_path",
  configKey: "raw_path",
  required: false,
});
const inboxSection = makePathSection({
  label: "your inbox folder",
  statusEl: document.querySelector("#inbox-status-msg"),
  useBtn: document.querySelector("#use-inbox-autodetect"),
  browseBtn: document.querySelector("#browse-inbox-folder"),
  autodetectCmd: "autodetect_inbox",
  setCmd: "set_inbox_path",
  configKey: "inbox_path",
  required: false,
});
const sharedRawSection = makePathSection({
  label: "the shared vault's raw folder",
  statusEl: document.querySelector("#shared-raw-status-msg"),
  useBtn: document.querySelector("#use-shared-raw-autodetect"),
  browseBtn: document.querySelector("#browse-shared-raw-folder"),
  autodetectCmd: "autodetect_shared_raw",
  setCmd: "set_shared_raw_path",
  configKey: "shared_raw_path",
  required: false,
});

async function updateDoneVisibility() {
  const config = await invoke("get_config");
  doneSettingsBtn.classList.toggle("hidden", !config.vault_path);
}

// --- Default app for .md files ---
// `is_default_md_handler` returns `Some(bool)` on macOS (a real Launch
// Services query) or `null` everywhere else — Windows blocks programmatic
// default-app changes entirely (a real OS restriction since Windows 8), so
// there's no equivalent check, and no fake status is shown for it.
const defaultHandlerStatusMsg = document.querySelector("#default-handler-status-msg");
const setDefaultHandlerBtn = document.querySelector("#set-default-handler-btn");
const openOsDefaultSettingsBtn = document.querySelector("#open-os-default-settings-btn");

async function refreshDefaultHandlerSection() {
  const isDefault = await invoke("is_default_md_handler");
  if (isDefault === null || isDefault === undefined) {
    defaultHandlerStatusMsg.textContent =
      "Windows doesn't let apps set themselves as default — click below to open Settings, then choose Second Brain Hub for .md and .markdown.";
    setDefaultHandlerBtn.classList.add("hidden");
    openOsDefaultSettingsBtn.classList.remove("hidden");
    return;
  }
  defaultHandlerStatusMsg.textContent = isDefault
    ? "Second Brain Hub is currently your default .md app."
    : "Not currently your default .md app.";
  openOsDefaultSettingsBtn.classList.add("hidden");
  setDefaultHandlerBtn.classList.remove("hidden");
  setDefaultHandlerBtn.disabled = isDefault;
  setDefaultHandlerBtn.textContent = isDefault ? "Already default" : "Set as default";
}

setDefaultHandlerBtn.addEventListener("click", async () => {
  try {
    await invoke("set_default_md_handler");
  } catch (e) {
    defaultHandlerStatusMsg.textContent = `Couldn't set as default: ${e}`;
    return;
  }
  // Re-check rather than assume success — this OS call has been observed to
  // report success without actually taking effect on some macOS versions.
  await refreshDefaultHandlerSection();
});

openOsDefaultSettingsBtn.addEventListener("click", () => {
  openUrl("ms-settings:defaultapps").catch((e) => alert(`Couldn't open Settings:\n${e}`));
});

function showOnboarding() {
  onboarding.classList.remove("hidden");
  searchView.classList.add("hidden");
  viewerView.classList.add("hidden");
  onboardingError.textContent = "";
  vaultSection.refresh();
  rawSection.refresh();
  inboxSection.refresh();
  sharedRawSection.refresh();
  refreshDefaultHandlerSection();
  updateDoneVisibility();
}

function showSearchView() {
  onboarding.classList.add("hidden");
  searchView.classList.remove("hidden");
  viewerView.classList.add("hidden");
  searchInput.value = "";
  resultsEl.innerHTML = "";
  emptyStateEl.textContent = "";
  invoke("get_index_status").then(renderStatus);
  refreshRawBadge();
  refreshInboxBadge();
  refreshSharedRawBadge();
  searchInput.focus();
}

// --- Markdown viewer (read-only) ---

let currentViewerPath = null;

// Resolves a markdown-relative image src (e.g. "images/foo.png",
// "../assets/x.png") against the source file's own directory, then rewrites
// it through convertFileSrc so the webview's asset protocol can load it.
// Skips anything already absolute or already a URL. POSIX-style path joining
// only (`/` separators) — matches how this vault is actually laid out.
function resolveImageSrc(src, sourceDir) {
  if (/^([a-z]+:|\/)/i.test(src)) return src; // already absolute or a URL scheme
  const parts = sourceDir.split("/").filter(Boolean);
  for (const segment of src.split("/")) {
    if (segment === "." || segment === "") continue;
    if (segment === "..") parts.pop();
    else parts.push(segment);
  }
  return "/" + parts.join("/");
}

async function openInViewer(path) {
  let doc;
  try {
    doc = await invoke("render_markdown", { path });
  } catch (e) {
    alert(`Couldn't open file:\n${path}\n\n${e}`);
    return;
  }
  currentViewerPath = doc.source_path;
  viewerTitleEl.textContent = doc.title;
  viewerContentEl.innerHTML = doc.html;

  const sourceDir = doc.source_path.slice(0, doc.source_path.lastIndexOf("/"));
  for (const img of viewerContentEl.querySelectorAll("img")) {
    const src = img.getAttribute("src");
    if (src) img.src = convertFileSrc(resolveImageSrc(src, sourceDir));
  }

  showViewer();
}

function showViewer() {
  onboarding.classList.add("hidden");
  searchView.classList.add("hidden");
  viewerView.classList.remove("hidden");
}

function renderStatus(status) {
  if (status.state === "indexing") {
    statusEl.textContent = "Indexing…";
  } else if (status.state === "error") {
    statusEl.textContent = "Index error — check Logs";
  } else {
    statusEl.textContent = `${status.count} docs indexed`;
  }
}

async function runSearch() {
  const query = searchInput.value.trim();
  if (!query) {
    resultsEl.innerHTML = "";
    emptyStateEl.textContent = "";
    return;
  }
  const results = await invoke("search_vault", { query });
  resultsEl.innerHTML = "";
  emptyStateEl.textContent = results.length === 0 ? "No matches." : "";
  for (const r of results) {
    const li = document.createElement("li");
    li.className = "result";
    li.innerHTML = `
      <div class="result-title">${escapeHtml(r.title)}</div>
      <div class="result-snippet">${r.snippet || ""}</div>
      <div class="result-meta">${escapeHtml(r.updated || "")}</div>
    `;
    li.addEventListener("dblclick", () => openInViewer(r.path));
    resultsEl.appendChild(li);
  }
}

function escapeHtml(s) {
  const div = document.createElement("div");
  div.textContent = s;
  return div.innerHTML;
}

// --- Raw / Inbox heads-up badges (topbar, omnipresent across tabs) ---

const rawBadge = document.querySelector("#raw-badge");
const rawCountNum = document.querySelector("#raw-count-num");
const inboxBadge = document.querySelector("#inbox-badge");
const inboxCountNum = document.querySelector("#inbox-count-num");
const sharedRawBadge = document.querySelector("#shared-raw-badge");
const sharedRawCountNum = document.querySelector("#shared-raw-count-num");

function renderRawCount(count) {
  rawBadge.classList.toggle("hidden", count === null || count === undefined);
  rawBadge.classList.toggle("count-alert", Boolean(count));
  if (count != null) rawCountNum.textContent = count;
}

function renderInboxCount(count) {
  inboxBadge.classList.toggle("hidden", count === null || count === undefined);
  inboxBadge.classList.toggle("count-alert", Boolean(count));
  if (count != null) inboxCountNum.textContent = count;
}

function renderSharedRawCount(count) {
  sharedRawBadge.classList.toggle("hidden", count === null || count === undefined);
  sharedRawBadge.classList.toggle("count-alert", Boolean(count));
  if (count != null) sharedRawCountNum.textContent = count;
}

async function refreshRawBadge() {
  renderRawCount(await invoke("get_raw_count"));
}

async function refreshInboxBadge() {
  renderInboxCount(await invoke("get_inbox_count"));
}

async function refreshSharedRawBadge() {
  renderSharedRawCount(await invoke("get_shared_raw_count"));
}

// --- Auto-update (checks on launch, install is user-initiated) ---

const updateBadge = document.querySelector("#update-badge");
let pendingUpdate = null;

async function checkForAppUpdate() {
  try {
    pendingUpdate = await checkForUpdate();
  } catch (e) {
    console.log("[update] check failed:", e);
    return;
  }
  if (pendingUpdate) updateBadge.classList.remove("hidden");
}

updateBadge.addEventListener("click", async () => {
  if (!pendingUpdate || updateBadge.disabled) return;
  updateBadge.disabled = true;
  updateBadge.textContent = "Installing…";
  try {
    await pendingUpdate.downloadAndInstall();
    await relaunch();
  } catch (e) {
    updateBadge.disabled = false;
    updateBadge.textContent = "⬇ Update available";
    alert(`Couldn't install the update:\n${e}`);
  }
});

// No `folder=` param, deliberately — Claude Desktop treats a link-supplied
// folder as untrusted and re-prompts "Trust this workspace?" every time.
// wiki-builder/inbox-review are global skills on absolute paths, so no cwd
// is needed (same fix already proven in the ZacAI Dashboard's own deep links).
function launchClaudeCommand(command) {
  const url = `claude://code/new?q=${encodeURIComponent(command)}`;
  openUrl(url).catch((e) => alert(`Couldn't open Claude Desktop:\n${e}`));
}

rawBadge.addEventListener("click", () => launchClaudeCommand("/wiki-builder"));
inboxBadge.addEventListener("click", () => launchClaudeCommand("/inbox-review"));

// No Claude command to run here — there's nothing the viewer can do about
// someone else's pending files. Just open the folder so it's still useful.
sharedRawBadge.addEventListener("click", async () => {
  const config = await invoke("get_config");
  if (config.shared_raw_path) revealItemInDir(config.shared_raw_path);
});

// Bare `claude://code/new` — no `q`, opens a fresh session with nothing pre-filled.
document.querySelector("#new-session-btn").addEventListener("click", () => {
  openUrl("claude://code/new").catch((e) => alert(`Couldn't open Claude Desktop:\n${e}`));
});

window.addEventListener("DOMContentLoaded", async () => {
  const config = await invoke("get_config");
  if (config.vault_path) {
    showSearchView();
  } else {
    showOnboarding();
  }

  checkForAppUpdate();

  listen("index-status", (event) => renderStatus(event.payload));
  listen("raw-count-status", (event) => renderRawCount(event.payload));
  listen("inbox-count-status", (event) => renderInboxCount(event.payload));
  listen("shared-raw-count-status", (event) => renderSharedRawCount(event.payload));

  // The OS handed this launch a file to open (double-click / "Open With",
  // once this app is a registered .md handler) — covers the already-running
  // case (single-instance forwarding, or a later macOS open-file event).
  listen("open-file-request", (event) => openInViewer(event.payload));
  // Covers a cold Windows/Linux launch: the frontend wasn't listening yet
  // when the Rust side first saw the file, so it's stashed for one pickup.
  invoke("get_launch_file_path").then((path) => {
    if (path) openInViewer(path);
  });

  doneSettingsBtn.addEventListener("click", showSearchView);

  document.querySelector("#change-vault-btn").addEventListener("click", showOnboarding);

  document.querySelector("#reindex-btn").addEventListener("click", () => {
    invoke("reindex_now");
  });

  document.querySelector("#export-logs-btn").addEventListener("click", async () => {
    const dir = await invoke("log_dir_path");
    revealItemInDir(dir);
  });

  searchInput.addEventListener("input", () => {
    clearTimeout(debounceTimer);
    debounceTimer = setTimeout(runSearch, 200);
  });

  document.querySelector("#viewer-back-btn").addEventListener("click", showSearchView);
  document.querySelector("#viewer-open-external-btn").addEventListener("click", () => {
    if (currentViewerPath) {
      openPath(currentViewerPath).catch((e) => alert(`Couldn't open file:\n${currentViewerPath}\n\n${e}`));
    }
  });
});
