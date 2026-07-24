const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { open: openDialog } = window.__TAURI__.dialog;
const { openPath, openUrl, revealItemInDir } = window.__TAURI__.opener;

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

// --- AI Search (formerly the "Chat" tab) ---

const modeAiSwitch = document.querySelector("#mode-ai-switch");
const searchPanel = document.querySelector("#search-panel");
const chatPanel = document.querySelector("#chat-panel");
const chatLoading = document.querySelector("#chat-loading");
const chatAskSection = document.querySelector("#chat-ask");
const chatProgressLabel = document.querySelector("#chat-progress-label");
const chatEnableError = document.querySelector("#chat-enable-error");
const chatRetryBtn = document.querySelector("#chat-retry-btn");
const chatInput = document.querySelector("#chat-input");
const relatedResultsEl = document.querySelector("#related-results");
const askBtn = document.querySelector("#ask-btn");
const chatThinking = document.querySelector("#chat-thinking");
const answerBox = document.querySelector("#answer-box");
const answerTextEl = document.querySelector("#answer-text");

let chatDebounceTimer = null;
let lastChatQuery = "";

function showMode(mode) {
  console.log("[ai-search] showMode(", mode, ")");
  const isKeyword = mode === "keyword";
  searchPanel.classList.toggle("hidden", !isKeyword);
  chatPanel.classList.toggle("hidden", isKeyword);
  if (!isKeyword) activateAiSearch();
}

// Selecting the AI Search radio goes straight for it — no separate "Enable"
// button/warning screen. Already-ready just shows the search bar; otherwise
// this is exactly what the old Enable-AI-Chat button used to do.
async function activateAiSearch() {
  console.log("[ai-search] activateAiSearch() called");
  const availability = await invoke("chat_availability");
  console.log("[ai-search] chat_availability ->", availability);
  if (availability.embedding_ready) {
    console.log("[ai-search] already ready, skipping loading state");
    chatLoading.classList.add("hidden");
    chatAskSection.classList.remove("hidden");
    chatInput.focus();
    return;
  }

  console.log("[ai-search] showing loading state, calling enable_chat");
  chatAskSection.classList.add("hidden");
  chatLoading.classList.remove("hidden");
  chatEnableError.textContent = "";
  chatRetryBtn.classList.add("hidden");
  chatProgressLabel.textContent = "Downloading embedding model…";

  // The embedding computation is CPU-heavy enough to starve the webview's
  // render thread — without yielding here, the spinner's DOM update never
  // gets painted before the freeze hits, so it silently never appears.
  // Force a real paint to commit before starting the heavy work.
  await new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)));

  const start = Date.now();
  try {
    await invoke("enable_chat");
    console.log("[ai-search] enable_chat resolved after", Date.now() - start, "ms");
    // Models are often already cached locally, so this can finish in well
    // under a second — force the loading state to stay visible for at least
    // this long so it isn't just an imperceptible flash.
    await minRemainingDelay(start, 500);
    chatLoading.classList.add("hidden");
    chatAskSection.classList.remove("hidden");
    chatInput.focus();
  } catch (e) {
    console.log("[ai-search] enable_chat rejected:", e);
    chatEnableError.textContent = String(e);
    chatRetryBtn.classList.remove("hidden");
  }
}

function minRemainingDelay(startTime, minMs) {
  const elapsed = Date.now() - startTime;
  return elapsed < minMs ? new Promise((r) => setTimeout(r, minMs - elapsed)) : Promise.resolve();
}

chatRetryBtn.addEventListener("click", activateAiSearch);

let lastRelatedResults = [];

function renderRelated(results) {
  relatedResultsEl.innerHTML = "";
  for (const r of results) {
    const li = document.createElement("li");
    li.className = "result";
    li.innerHTML = `
      <div class="result-title">${escapeHtml(r.title)}</div>
      <div class="result-snippet">${escapeHtml(r.snippet)}</div>
    `;
    li.addEventListener("dblclick", () => openResultPath(r.path));
    relatedResultsEl.appendChild(li);
  }
}

// The model isn't asked for markdown, but small instruct models default to
// it anyway (**bold**, `code`) — strip it since the answer renders as plain
// text, not through a markdown renderer.
function cleanAnswerText(text) {
  return text.replace(/[*`]+/g, "");
}

async function fetchRelated(query) {
  const results = await invoke("related_docs", { query });
  lastRelatedResults = results;
  askBtn.classList.toggle("hidden", results.length === 0);
  return results;
}

// Extractive only — fires on every debounce tick while typing. Cheap and
// near-instant, safe to run on every pause. Generation (`askForAnswer`) is
// deliberately NOT chained off this: it's heavy enough to freeze the render
// thread for its full duration (same class of issue as ARCHITECTURE.md §11
// #6), so firing it on every mid-sentence typing pause reads as the whole
// app locking up mid-keystroke. Generation only runs on explicit submit
// (Enter key or the Ask button). The related-doc list itself stays hidden
// until an answer has been shown (see `askForAnswer`) — only the Ask button's
// visibility reacts to results while typing.
async function runRelatedSearch() {
  const query = chatInput.value.trim();
  lastChatQuery = query;
  answerBox.classList.add("hidden");
  relatedResultsEl.innerHTML = ""; // clear any list left over from a previous answer
  if (!query) {
    lastRelatedResults = [];
    askBtn.classList.add("hidden");
    return;
  }

  await fetchRelated(query);
}

async function askForAnswer() {
  const query = chatInput.value.trim();
  if (!query) return;
  lastChatQuery = query;
  clearTimeout(chatDebounceTimer);
  answerBox.classList.add("hidden");
  answerTextEl.textContent = "";
  relatedResultsEl.innerHTML = "";
  chatThinking.classList.remove("hidden");
  askBtn.disabled = true;

  // Enter can be pressed before the 200ms debounce fires — make sure the
  // related-doc list (used both for the Ask-button gate and as citations
  // once the answer renders) is fresh for this exact query.
  await fetchRelated(query);

  // Same paint-yield fix as `enable_chat`'s loading spinner (ARCHITECTURE.md
  // §11 #6) — the candle generation call is CPU-heavy enough to starve the
  // render thread before the "Thinking…" indicator gets painted otherwise.
  await new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)));

  try {
    const answer = await invoke("write_answer", { query });
    if (query !== lastChatQuery) return;
    answerTextEl.textContent = cleanAnswerText(answer);
    answerBox.classList.remove("hidden");
  } catch (e) {
    if (query !== lastChatQuery) return;
    answerTextEl.textContent = String(e);
    answerBox.classList.remove("hidden");
  } finally {
    chatThinking.classList.add("hidden");
    askBtn.disabled = false;
    if (query === lastChatQuery) renderRelated(lastRelatedResults);
  }
}

// --- Raw / Inbox heads-up badges (topbar, omnipresent across tabs) ---

const rawBadge = document.querySelector("#raw-badge");
const rawCountNum = document.querySelector("#raw-count-num");
const inboxBadge = document.querySelector("#inbox-badge");
const inboxCountNum = document.querySelector("#inbox-count-num");

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

async function refreshRawBadge() {
  renderRawCount(await invoke("get_raw_count"));
}

async function refreshInboxBadge() {
  renderInboxCount(await invoke("get_inbox_count"));
}

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

  listen("index-status", (event) => renderStatus(event.payload));
  listen("raw-count-status", (event) => renderRawCount(event.payload));
  listen("inbox-count-status", (event) => renderInboxCount(event.payload));

  listen("model-download-progress", (event) => {
    const { model, downloaded, total } = event.payload;
    const mb = (n) => (n / 1_000_000).toFixed(0);
    chatProgressLabel.textContent = `Downloading ${model} model… ${mb(downloaded)}MB / ${mb(total)}MB`;
  });

  listen("chat-status", (event) => {
    chatProgressLabel.textContent = event.payload;
  });

  modeAiSwitch.addEventListener("change", () => showMode(modeAiSwitch.checked ? "ai" : "keyword"));

  chatInput.addEventListener("input", () => {
    clearTimeout(chatDebounceTimer);
    chatDebounceTimer = setTimeout(runRelatedSearch, 200);
  });

  chatInput.addEventListener("keydown", (e) => {
    if (e.key === "Enter") askForAnswer();
  });

  askBtn.addEventListener("click", askForAnswer);

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
