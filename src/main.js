const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { open: openDialog } = window.__TAURI__.dialog;
const { openPath, revealItemInDir } = window.__TAURI__.opener;

const onboarding = document.querySelector("#onboarding");
const searchView = document.querySelector("#search-view");
const autodetectMsg = document.querySelector("#autodetect-msg");
const useAutodetectBtn = document.querySelector("#use-autodetect");
const browseFolderBtn = document.querySelector("#browse-folder");
const onboardingError = document.querySelector("#onboarding-error");
const searchInput = document.querySelector("#search-input");
const statusEl = document.querySelector("#status");
const resultsEl = document.querySelector("#results");
const emptyStateEl = document.querySelector("#empty-state");

let detectedPath = null;
let debounceTimer = null;

function showOnboarding() {
  onboarding.classList.remove("hidden");
  searchView.classList.add("hidden");
  onboardingError.textContent = "";
  invoke("autodetect_vault").then((path) => {
    detectedPath = path;
    if (path) {
      autodetectMsg.textContent = `Found: ${path}`;
      useAutodetectBtn.classList.remove("hidden");
    } else {
      autodetectMsg.textContent = "Couldn't auto-detect it — browse for the folder instead.";
      useAutodetectBtn.classList.add("hidden");
    }
  });
}

function showSearchView() {
  onboarding.classList.add("hidden");
  searchView.classList.remove("hidden");
  searchInput.value = "";
  resultsEl.innerHTML = "";
  emptyStateEl.textContent = "";
  invoke("get_index_status").then(renderStatus);
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

async function trySetVault(path) {
  onboardingError.textContent = "";
  try {
    await invoke("set_vault_path", { path });
    showSearchView();
  } catch (e) {
    onboardingError.textContent = String(e);
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

// --- Chat tab ---

const tabSearchBtn = document.querySelector("#tab-search");
const tabChatBtn = document.querySelector("#tab-chat");
const searchPanel = document.querySelector("#search-panel");
const chatPanel = document.querySelector("#chat-panel");
const chatEnableSection = document.querySelector("#chat-enable");
const chatAskSection = document.querySelector("#chat-ask");
const enableChatBtn = document.querySelector("#enable-chat-btn");
const chatProgress = document.querySelector("#chat-progress");
const chatProgressFill = document.querySelector("#chat-progress-fill");
const chatProgressLabel = document.querySelector("#chat-progress-label");
const chatEnableError = document.querySelector("#chat-enable-error");
const chatInput = document.querySelector("#chat-input");
const relatedResultsEl = document.querySelector("#related-results");
const writeAnswerRow = document.querySelector("#write-answer-row");
const writeAnswerBtn = document.querySelector("#write-answer-btn");
const answerBox = document.querySelector("#answer-box");
const answerTextEl = document.querySelector("#answer-text");

let chatDebounceTimer = null;
let lastChatQuery = "";

function showTab(name) {
  const isSearch = name === "search";
  tabSearchBtn.classList.toggle("active", isSearch);
  tabChatBtn.classList.toggle("active", !isSearch);
  searchPanel.classList.toggle("hidden", !isSearch);
  chatPanel.classList.toggle("hidden", isSearch);
  if (!isSearch) checkChatAvailability();
}

async function checkChatAvailability() {
  const availability = await invoke("chat_availability");
  chatEnableSection.classList.toggle("hidden", availability.embedding_ready);
  chatAskSection.classList.toggle("hidden", !availability.embedding_ready);
  if (availability.embedding_ready) chatInput.focus();
}

enableChatBtn.addEventListener("click", async () => {
  chatEnableError.textContent = "";
  enableChatBtn.disabled = true;
  chatProgress.classList.remove("hidden");
  chatProgressLabel.textContent = "Downloading embedding model…";
  try {
    await invoke("enable_chat");
    chatEnableSection.classList.add("hidden");
    chatAskSection.classList.remove("hidden");
    chatInput.focus();
  } catch (e) {
    chatEnableError.textContent = String(e);
    enableChatBtn.disabled = false;
  } finally {
    chatProgress.classList.add("hidden");
  }
});

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
  writeAnswerRow.classList.toggle("hidden", results.length === 0);
}

async function runRelatedSearch() {
  const query = chatInput.value.trim();
  lastChatQuery = query;
  answerBox.classList.add("hidden");
  if (!query) {
    relatedResultsEl.innerHTML = "";
    writeAnswerRow.classList.add("hidden");
    return;
  }
  const results = await invoke("related_docs", { query });
  renderRelated(results);
}

writeAnswerBtn.addEventListener("click", async () => {
  const query = lastChatQuery;
  if (!query) return;
  writeAnswerBtn.disabled = true;
  writeAnswerBtn.textContent = "Thinking…";
  answerBox.classList.remove("hidden");
  answerTextEl.textContent = "";
  try {
    const answer = await invoke("write_answer", { query });
    answerTextEl.textContent = answer;
  } catch (e) {
    answerTextEl.textContent = `Error: ${e}`;
  } finally {
    writeAnswerBtn.disabled = false;
    writeAnswerBtn.textContent = "Write me an answer";
  }
});

window.addEventListener("DOMContentLoaded", async () => {
  const config = await invoke("get_config");
  if (config.vault_path) {
    showSearchView();
  } else {
    showOnboarding();
  }

  listen("index-status", (event) => renderStatus(event.payload));

  listen("model-download-progress", (event) => {
    const { model, downloaded, total } = event.payload;
    const pct = total > 0 ? Math.round((downloaded / total) * 100) : 0;
    const mb = (n) => (n / 1_000_000).toFixed(0);
    chatProgressLabel.textContent = `Downloading ${model} model… ${mb(downloaded)}MB / ${mb(total)}MB`;
    chatProgressFill.style.width = `${pct}%`;
  });

  // Covers the compute-only phases (model load, chunk embedding) that have no
  // byte-progress of their own — without this the progress bar looks frozen
  // once any download finishes but real work is still happening.
  listen("chat-status", (event) => {
    chatProgressLabel.textContent = event.payload;
    chatProgressFill.style.width = "100%";
  });

  tabSearchBtn.addEventListener("click", () => showTab("search"));
  tabChatBtn.addEventListener("click", () => showTab("chat"));

  chatInput.addEventListener("input", () => {
    clearTimeout(chatDebounceTimer);
    chatDebounceTimer = setTimeout(runRelatedSearch, 200);
  });

  useAutodetectBtn.addEventListener("click", () => {
    if (detectedPath) trySetVault(detectedPath);
  });

  browseFolderBtn.addEventListener("click", async () => {
    const selected = await openDialog({ directory: true, multiple: false });
    if (selected) trySetVault(selected);
  });

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
