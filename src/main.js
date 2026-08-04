const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { open: openDialog } = window.__TAURI__.dialog;
const { openPath, openUrl, revealItemInDir } = window.__TAURI__.opener;
const { check: checkForUpdate } = window.__TAURI__.updater;
const { relaunch } = window.__TAURI__.process;

const onboarding = document.querySelector("#onboarding");
const searchView = document.querySelector("#search-view");
const onboardingError = document.querySelector("#onboarding-error");
const doneSettingsBtn = document.querySelector("#done-settings-btn");
const searchInput = document.querySelector("#search-input");
const statusEl = document.querySelector("#status");
const resultsEl = document.querySelector("#results");
const emptyStateEl = document.querySelector("#empty-state");

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

function showOnboarding() {
  onboarding.classList.remove("hidden");
  searchView.classList.add("hidden");
  onboardingError.textContent = "";
  vaultSection.refresh();
  rawSection.refresh();
  inboxSection.refresh();
  sharedRawSection.refresh();
  updateDoneVisibility();
}

function showSearchView() {
  onboarding.classList.add("hidden");
  searchView.classList.remove("hidden");
  searchInput.value = "";
  resultsEl.innerHTML = "";
  emptyStateEl.textContent = "";
  invoke("get_index_status").then(renderStatus);
  refreshRawBadge();
  refreshInboxBadge();
  refreshSharedRawBadge();
  searchInput.focus();
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
    li.addEventListener("dblclick", () => openResultPath(r.path));
    resultsEl.appendChild(li);
  }
}

function openResultPath(path) {
  openPath(path).catch((e) => alert(`Couldn't open file:\n${path}\n\n${e}`));
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
});
