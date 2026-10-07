const { invoke, convertFileSrc } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { open: openDialog, confirm: confirmDialog, message: messageDialog } = window.__TAURI__.dialog;
const { openPath, openUrl, revealItemInDir } = window.__TAURI__.opener;
const { check: checkForUpdate } = window.__TAURI__.updater;
const { relaunch } = window.__TAURI__.process;
const { getVersion } = window.__TAURI__.app;

const onboarding = document.querySelector("#onboarding");
const searchView = document.querySelector("#search-view");
const viewerView = document.querySelector("#viewer-view");
const mapView = document.querySelector("#map-view");
const skillsView = document.querySelector("#skills-view");
const transcriptsView = document.querySelector("#transcripts-view");
const onboardingError = document.querySelector("#onboarding-error");
const settingsBackBtn = document.querySelector("#settings-back-btn");
const searchInput = document.querySelector("#search-input");
const statusEl = document.querySelector("#status");
const resultsEl = document.querySelector("#results");
const emptyStateEl = document.querySelector("#empty-state");
const viewerTitleEl = document.querySelector("#viewer-title");
const viewerContentEl = document.querySelector("#viewer-content");
const viewerOpenExternalBtn = document.querySelector("#viewer-open-external-btn");

let debounceTimer = null;

// One of these per settings-section (vault/raw/inbox) — same autodetect →
// confirm-or-browse → persist shape, so it's a factory instead of copy-pasted
// three times.
function makePathSection({ label, statusEl, useBtn, browseBtn, autodetectCmd, setCmd, configKey, required, onRefresh }) {
  let detected = null;

  async function refresh() {
    await refreshStatus();
    onRefresh?.();
  }

  async function refreshStatus() {
    const config = await invoke("get_config");
    const current = config[configKey];
    if (current) {
      statusEl.textContent = `Using: ${current}`;
      useBtn.classList.add("hidden");
      updateBackButtonVisibility();
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

const stopTranscriptsBtn = document.querySelector("#stop-transcripts-btn");
const showTranscriptsPageBox = document.querySelector("#show-transcripts-page");
const transcriptsSection = makePathSection({
  label: "a transcripts folder",
  statusEl: document.querySelector("#transcripts-status-msg"),
  useBtn: document.querySelector("#use-transcripts-autodetect"),
  browseBtn: document.querySelector("#browse-transcripts-folder"),
  autodetectCmd: "autodetect_transcripts",
  setCmd: "set_transcripts_path",
  configKey: "transcripts_path",
  required: false,
  onRefresh: async () => {
    const config = await invoke("get_config");
    stopTranscriptsBtn.classList.toggle("hidden", !config.transcripts_path);
    showTranscriptsPageBox.checked = !config.hide_transcripts_page;
    autostartBox.checked = await invoke("get_autostart");
    autoRules = config.auto_rules ?? [];
  vocConsent = !!config.voc_consent;
    renderAutoRules();
    document.querySelector("#add-rule-btn").disabled = !config.transcripts_path;
    applyTranscriptsNav(config);
  },
});

const autostartBox = document.querySelector("#autostart-toggle");
autostartBox.addEventListener("change", async () => {
  try {
    await invoke("set_autostart", { on: autostartBox.checked });
  } catch (e) {
    autostartBox.checked = !autostartBox.checked;
    messageDialog(`${e}`, { title: "Couldn't change start at login", kind: "error" });
  }
});

stopTranscriptsBtn.addEventListener("click", async () => {
  const ok = await confirmDialog("Stop watching the transcripts folder? Converted files and the index stay on this machine, and turning it back on reuses them.", {
    title: "Stop syncing transcripts",
    okLabel: "Stop syncing",
    cancelLabel: "Keep syncing",
  });
  if (!ok) return;
  await invoke("stop_transcripts");
  transcriptsSection.refresh();
});

showTranscriptsPageBox.addEventListener("change", async () => {
  await invoke("set_show_transcripts_page", { show: showTranscriptsPageBox.checked });
  applyTranscriptsNav(await invoke("get_config"));
});

async function updateBackButtonVisibility() {
  const config = await invoke("get_config");
  settingsBackBtn.classList.toggle("hidden", !config.vault_path);
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

// Every top-level section toggles through this one helper instead of each
// show*View() hand-repeating a `classList` line per sibling section.
const VIEWS = { onboarding, search: searchView, viewer: viewerView, map: mapView, skills: skillsView, transcripts: transcriptsView };
function showView(name) {
  for (const [key, el] of Object.entries(VIEWS)) el.classList.toggle("hidden", key !== name);
}

async function showSettingsView() {
  showView("onboarding");
  onboardingError.textContent = "";
  vaultSection.refresh();
  rawSection.refresh();
  inboxSection.refresh();
  sharedRawSection.refresh();
  transcriptsSection.refresh();
  refreshDefaultHandlerSection();
  updateBackButtonVisibility();
  document.querySelector("#app-version-value").textContent = await getVersion();
}

function showSearchView() {
  showView("search");
  searchInput.value = "";
  resultsEl.innerHTML = "";
  emptyStateEl.textContent = "";
  invoke("get_index_status").then(renderStatus);
  refreshRawBadge();
  refreshInboxBadge();
  refreshSharedRawBadge();
  searchInput.focus();
}


// --- Transcripts: Teams .vtt files converted to markdown by the Rust side
// (zero tokens). Opt-in: with no folder set this page is just an empty state. ---

const transcriptsList = document.querySelector("#transcripts-list");
const transcriptsEmpty = document.querySelector("#transcripts-empty");
const transcriptsStatus = document.querySelector("#transcripts-status");
const transcriptsShowDups = document.querySelector("#transcripts-show-dups");
const transcriptsDupsLabel = document.querySelector("#transcripts-dups-label");
const transcriptsShowHidden = document.querySelector("#transcripts-show-hidden");
const transcriptsNotNowBtn = document.querySelector("#transcripts-notnow-btn");
let transcriptRows = [];
let transcriptsNoticeTimer = null;

function applyTranscriptsNav(config) {
  document.querySelector("#transcripts-btn").classList.toggle("hidden", !!config.hide_transcripts_page);
}

function transcriptsNotice(text) {
  transcriptsStatus.textContent = text;
  clearTimeout(transcriptsNoticeTimer);
  transcriptsNoticeTimer = setTimeout(renderTranscripts, 4000);
}

let transcriptCategories = [];
let auditStatus = { enabled: false, pending: 0, stuck_days: 0 };
const selectedTranscripts = new Set();
const transcriptsBar = document.querySelector("#transcripts-bar");
const bulkCatSelect = document.querySelector("#transcripts-bulk-cat");

const categoryOptions = (current) =>
  transcriptCategories.map((c) => `<option${c === current ? " selected" : ""}>${escapeHtml(c)}</option>`).join("");

// VoC actions only apply to transcripts in the VoC category (the one the categorizer's clear-signal gate fills).
let vocCategory = "";
const isVoc = (r) => r.category === vocCategory;

function renderSelectionBar() {
  const n = selectedTranscripts.size;
  document.querySelector("#transcripts-voc-sel").classList.toggle("hidden", !transcriptRows.some((r) => selectedTranscripts.has(r.vtt) && isVoc(r)));
  transcriptsBar.classList.toggle("hidden", n === 0);
  document.querySelector("#transcripts-selected").textContent = `${n} selected`;
  // All selected already hidden -> the button unhides; any visible one in the mix -> it hides.
  const hideBtn = document.querySelector("#transcripts-hide-sel");
  hideBtn.textContent = selectionAllHidden() ? "Unhide" : "Hide";
}

const selectionAllHidden = () => {
  const rows = transcriptRows.filter((r) => selectedTranscripts.has(r.vtt));
  return rows.length > 0 && rows.every((r) => r.hidden);
};

function renderTranscripts() {
  const scroll = transcriptsList.scrollTop;
  const dups = transcriptRows.filter((r) => r.duplicate_of);
  const hiddenCount = transcriptRows.filter((r) => r.hidden && !r.duplicate_of).length;
  document.querySelector("#transcripts-hidden-label").textContent = `Show hidden (${hiddenCount})`;
  const shown = transcriptRows.filter(
    (r) => (transcriptsShowDups.checked || !r.duplicate_of) && (transcriptsShowHidden.checked || !r.hidden || r.duplicate_of),
  );
  // Identical content on a later date means Teams saved the wrong transcript, so the real one is missing.
  const meetings = new Set(dups.map((r) => r.duplicate_of)).size;
  transcriptsDupsLabel.textContent = `Suspect downloads (${dups.length})`;
  transcriptsDupsLabel.parentElement.title = dups.length ? `${dups.length} files across ${meetings} meetings are identical to an earlier one — Teams likely saved the wrong transcript, so the real one is missing.` : "";
  transcriptsStatus.textContent =
    `${transcriptRows.length - dups.length - hiddenCount} transcripts` +
    moveErrorText +
    (auditStatus.stuck_days >= 3 ? ` · ⚠ category log not reaching the shared drive for ${auditStatus.stuck_days} days` : "");
  const labelOf = (vtt) => {
    const o = transcriptRows.find((r) => r.vtt === vtt);
    return o ? `${o.title} (${o.date ?? "undated"})` : vtt;
  };
  for (const v of [...selectedTranscripts]) if (!shown.some((r) => r.vtt === v)) selectedTranscripts.delete(v);
  transcriptsList.innerHTML = "";
  for (const r of shown) {
    const li = document.createElement("li");
    li.className = `skill-row transcript-row${r.duplicate_of ? " is-dup" : ""}`;
    li.tabIndex = 0;
    li.dataset.vtt = r.vtt;
    const meta = [r.date, `${r.speakers} speaker${r.speakers === 1 ? "" : "s"}`, `${r.duration_min} min`, `${r.words.toLocaleString()} words`];
    li.innerHTML = `
      <div class="transcript-head">
        <input type="checkbox" class="t-sel" aria-label="Select"${selectedTranscripts.has(r.vtt) ? " checked" : ""}${r.duplicate_of ? " disabled" : ""} />
        <span class="skill-name">${escapeHtml(r.title)}</span>
        ${r.hidden && !r.duplicate_of ? `<span class="transcript-cat suggested">${r.hidden_reason === "logged" ? "Logged" : "Hidden"}</span>` : ""}
        ${r.voc_ingested && !r.duplicate_of ? '<span class="transcript-cat suggested" title="Sent to voc-library-ingest">Sent to VoC</span>' : ""}
        ${r.voc_skip && !r.duplicate_of ? '<span class="transcript-cat suggested" title="Excluded from automatic VoC">Skip Auto VoC</span>' : ""}
        ${r.confidence === "low" ? '<span class="transcript-cat suggested">suggested</span>' : ""}
        <select class="t-cat" title="${escapeHtml(r.category_source)}"${r.duplicate_of ? " disabled" : ""}>${categoryOptions(r.category)}</select>
        <button class="t-actions topbar-btn" aria-label="Actions" aria-haspopup="menu" title="Actions">⋯</button>
      </div>
      <span class="skill-plugin">${escapeHtml(meta.filter(Boolean).join(" · "))}${r.duplicate_of ? ` · ${r.near_duplicate ? "nearly identical" : "identical"} to ${escapeHtml(labelOf(r.duplicate_of))}: likely a bad download, real transcript missing` : ""}</span>
    `;
    transcriptsList.appendChild(li);
  }
  transcriptsList.scrollTop = scroll;
  renderSelectionBar();
}

async function loadTranscripts() {
  const config = await invoke("get_config");
  const configured = !!config.transcripts_path;
  autoRules = config.auto_rules ?? [];
  vocConsent = !!config.voc_consent;
  transcriptsEmpty.classList.toggle("hidden", configured);
  transcriptsList.classList.toggle("hidden", !configured);
  transcriptsNotNowBtn.classList.toggle("hidden", !!config.transcripts_dismissed);
  document.querySelectorAll(".transcripts-toggle").forEach((el) => el.classList.toggle("hidden", !configured));
  if (!configured) {
    transcriptsStatus.textContent = "";
    transcriptsBar.classList.add("hidden");
    document.querySelector("#transcripts-privacy").classList.add("hidden");
    return;
  }
  [transcriptRows, transcriptCategories, auditStatus, vocCategory] = await Promise.all([
    invoke("get_transcripts"),
    invoke("get_transcript_categories"),
    invoke("get_audit_status"),
    invoke("get_voc_category"),
  ]);
  bulkCatSelect.innerHTML = categoryOptions("");
  document.querySelector("#transcripts-privacy").classList.toggle("hidden", !auditStatus.enabled);
  renderTranscripts();
  moveError();
}

async function applyCategory(vtts, category) {
  try {
    await invoke("set_transcript_category", { vtts, category });
  } catch (e) {
    messageDialog(`${e}`, { title: "Couldn't change category", kind: "error" });
  }
  await loadTranscripts();
}

transcriptsList.addEventListener("change", (e) => {
  const li = e.target.closest(".transcript-row");
  if (!li) return;
  if (e.target.matches(".t-sel")) {
    e.target.checked ? selectedTranscripts.add(li.dataset.vtt) : selectedTranscripts.delete(li.dataset.vtt);
    renderSelectionBar();
  } else if (e.target.matches(".t-cat")) {
    applyCategory([li.dataset.vtt], e.target.value);
  }
});
document.querySelector("#transcripts-bulk-apply").addEventListener("click", async () => {
  const vtts = [...selectedTranscripts];
  selectedTranscripts.clear();
  await applyCategory(vtts, bulkCatSelect.value);
});
let moveErrorText = "";
const moveError = () =>
  invoke("get_move_error").then((err) => {
    moveErrorText = err ? ` · ⚠ Auto rules paused: ${err}` : "";
    renderTranscripts();
  });

// Move / hide / log-it work on a list of transcripts, so the selection bar and the
// per-row "⋯" menu share one code path.
async function moveTranscripts(vtts) {
  const dest = await openDialog({ directory: true, multiple: false });
  if (!dest) return;
  try {
    const r = await invoke("move_transcripts", { vtts, dest });
    vtts.forEach((v) => selectedTranscripts.delete(v));
    await loadTranscripts();
    transcriptsNotice(`Moved ${r.moved}${r.skipped ? `, ${r.skipped} failed` : ""}. Duplicates and missing files are skipped.`);
  } catch (e) {
    messageDialog(`${e}`, { title: "Couldn't move", kind: "error" });
  }
}

async function hideTranscripts(vtts, hidden) {
  vtts.forEach((v) => selectedTranscripts.delete(v));
  await invoke("set_transcripts_hidden", { vtts, hidden });
  await loadTranscripts();
}

// One launch per click, capped so the deep-link URL stays short. Duplicates and
// not-yet-converted rows are skipped. Marked logged (and hidden) optimistically:
// the deep link cannot confirm completion; Unhide makes a transcript eligible again.
const LOGIT_MAX = 10;
async function logitTranscripts(vtts) {
  const rows = transcriptRows.filter((r) => vtts.includes(r.vtt) && !r.duplicate_of && r.md_exists && r.open_path);
  if (!rows.length) return transcriptsNotice("Nothing to log: select converted, non-duplicate transcripts.");
  const batch = rows.slice(0, LOGIT_MAX);
  const files = batch.map((r) => r.open_path);
  const prompt =
    files.length === 1
      ? `/log-it ${files[0]}`
      : `Run /log-it in file-argument mode on each of these transcript files, one after another:\n${files.join("\n")}`;
  try {
    await openUrl(`claude://code/new?q=${encodeURIComponent(prompt)}`);
  } catch (err) {
    return messageDialog(`Couldn't open Claude Desktop:\n${err}`, { title: "Run log-it", kind: "error" });
  }
  batch.forEach((r) => selectedTranscripts.delete(r.vtt));
  await invoke("mark_transcripts_logged", { vtts: batch.map((r) => r.vtt) });
  await loadTranscripts();
  transcriptsNotice(rows.length > LOGIT_MAX ? `Sent ${LOGIT_MAX} of ${rows.length} to log-it; select the rest and run again.` : `Sent ${batch.length} to log-it.`);
}

// Interactive: voc-library-ingest shows its batch summary and waits for approval before writing to the shared
// vault. Badge only (the deep link can't confirm completion); the row stays visible and eligible for log-it.
async function vocTranscripts(vtts) {
  const rows = transcriptRows.filter((r) => vtts.includes(r.vtt) && isVoc(r) && !r.duplicate_of && r.md_exists && r.open_path);
  if (!rows.length) return transcriptsNotice("Nothing to send: select converted, non-duplicate transcripts in the VoC category.");
  const batch = rows.slice(0, LOGIT_MAX);
  const files = batch.map((r) => r.open_path);
  const prompt =
    files.length === 1
      ? `/voc-library-ingest ${files[0]}`
      : `Run /voc-library-ingest on these transcript files as one batch (these are Teams transcripts already converted to markdown):\n${files.join("\n")}`;
  try {
    await openUrl(`claude://code/new?q=${encodeURIComponent(prompt)}`);
  } catch (err) {
    return messageDialog(`Couldn't open Claude Desktop:\n${err}`, { title: "Process VoC", kind: "error" });
  }
  batch.forEach((r) => selectedTranscripts.delete(r.vtt));
  await invoke("mark_transcripts_voc", { vtts: batch.map((r) => r.vtt) });
  await loadTranscripts();
  transcriptsNotice(rows.length > LOGIT_MAX ? `Sent ${LOGIT_MAX} of ${rows.length} to VoC; select the rest and run again.` : `Sent ${batch.length} to VoC.`);
}

const selectedList = () => [...selectedTranscripts];
document.querySelector("#transcripts-voc-sel").addEventListener("click", () => vocTranscripts(selectedList()));
document.querySelector("#transcripts-move-sel").addEventListener("click", () => moveTranscripts(selectedList()));
document.querySelector("#transcripts-hide-sel").addEventListener("click", () => hideTranscripts(selectedList(), !selectionAllHidden()));
document.querySelector("#transcripts-logit-sel").addEventListener("click", () => logitTranscripts(selectedList()));

// Per-row "⋯" menu: Open, Move…, Run log-it, Hide/Unhide for that one transcript.
const rowMenu = document.querySelector("#row-menu");
const closeRowMenu = () => rowMenu.classList.add("hidden");
transcriptsList.addEventListener("click", (e) => {
  const btn = e.target.closest(".t-actions");
  if (!btn) return;
  e.stopPropagation();
  const vtt = btn.closest(".transcript-row").dataset.vtt;
  const r = transcriptRows.find((x) => x.vtt === vtt);
  if (!r) return;
  const items = [["open", "Open"]];
  if (!r.duplicate_of) {
    items.push(["move", "Move…"], ["logit", "Run log-it"]);
    if (isVoc(r)) items.push(["voc", "Process VoC"]);
    // Skip/Queue only mean something once an automatic VoC rule is on.
    if (isVoc(r) && autoRules.some((x) => x.enabled && x.action === "voc")) items.push(r.voc_skip ? ["vocallow", "Queue for Auto VoC"] : ["vocskip", "Skip Auto VoC"]);
    items.push(r.hidden ? ["unhide", "Unhide"] : ["hide", "Hide"]);
  }
  rowMenu.innerHTML = items.map(([act, label]) => `<button role="menuitem" data-act="${act}">${label}</button>`).join("");
  rowMenu.dataset.vtt = vtt;
  const box = btn.getBoundingClientRect();
  rowMenu.style.top = `${box.bottom + 4}px`;
  rowMenu.style.left = `${Math.max(8, box.right - 150)}px`;
  rowMenu.classList.remove("hidden");
});
rowMenu.addEventListener("click", (e) => {
  const act = e.target.dataset.act;
  if (!act) return;
  const vtt = rowMenu.dataset.vtt;
  closeRowMenu();
  if (act === "open") openTranscript(vtt);
  else if (act === "move") moveTranscripts([vtt]);
  else if (act === "logit") logitTranscripts([vtt]);
  else if (act === "voc") vocTranscripts([vtt]);
  else if (act === "vocskip" || act === "vocallow") invoke("set_transcripts_voc_skip", { vtts: [vtt], skip: act === "vocskip" }).then(loadTranscripts);
  else hideTranscripts([vtt], act === "hide");
});
document.addEventListener("click", closeRowMenu);
document.addEventListener("keydown", (e) => e.key === "Escape" && closeRowMenu());

// --- Auto rules: listed in Settings, edited in a modal. Saved as one ordered list. ---
let autoRules = [];
const ruleModal = document.querySelector("#rule-modal");
const rf = {
  action: document.querySelector("#rule-action"),
  destField: document.querySelector("#rule-dest-field"),
  dest: document.querySelector("#rule-dest"),
  mode: document.querySelector("#rule-mode"),
  days: document.querySelector("#rule-days"),
  cats: document.querySelector("#rule-cats"),
  preview: document.querySelector("#rule-preview"),
  destPath: null,
  editing: -1, // index in autoRules, or -1 for a new rule
  enabled: true,
  id: "",
};
let vocConsent = false;
const saveRules = async (rules) => {
  if (!vocConsent && rules.some((r) => r.enabled && r.action === "voc")) {
    const ok = await confirmDialog(
      "Automatic VoC copies matching transcripts, including attendee names, into the shared VoC library that your team can read. Nothing outside the VoC category is processed, and up to 5 run per pass.\n\nTurn it on?",
      { title: "Automatic VoC", okLabel: "Turn on", cancelLabel: "Cancel" },
    );
    if (!ok) return;
    await invoke("set_voc_consent", { on: true });
    vocConsent = true;
  }
  await invoke("set_auto_rules", { rules });
  autoRules = rules;
  renderAutoRules();
};
const ruleFromForm = () => ({
  id: rf.id,
  action: rf.action.value,
  enabled: rf.enabled,
  dest: rf.action.value === "move" ? rf.destPath : null,
  mode: rf.mode.value,
  categories: [...rf.cats.querySelectorAll("input:checked")].map((i) => i.value),
  after_days: Math.max(0, parseInt(rf.days.value, 10) || 0),
});
async function refreshRulePreview() {
  const rule = ruleFromForm();
  const idx = rf.editing >= 0 ? rf.editing : autoRules.length;
  const list = rf.editing >= 0 ? autoRules.map((r, i) => (i === idx ? rule : r)) : [...autoRules, rule];
  const n = rule.action === "move" && !rule.dest ? null : await invoke("preview_auto_rule", { rules: list, idx });
  rf.preview.textContent =
    n === null ? "Choose a folder first." : `${n} existing file(s) would ${{ move: "move", voc: "be queued for VoC" }[rule.action] ?? "be queued for log-it"} when this rule is on.`;
}
async function openRuleModal(index) {
  const cats = await invoke("get_transcript_categories");
  const r = index >= 0 ? autoRules[index] : { id: crypto.randomUUID().slice(0, 8), enabled: true, dest: null, mode: "include", categories: [], after_days: 0 };
  Object.assign(rf, { editing: index, id: r.id, enabled: r.enabled, destPath: r.dest });
  rf.action.value = r.action || "move";
  rf.destField.classList.toggle("hidden", rf.action.value !== "move");
  rf.dest.textContent = r.dest ?? "no folder chosen";
  rf.mode.value = r.mode;
  rf.days.value = r.after_days;
  rf.allCats = cats;
  rf.vocCat = await invoke("get_voc_category");
  renderRuleCats(r.categories);
  ruleModal.classList.remove("hidden");
  refreshRulePreview();
}
// A VoC rule can only target the VoC category, so there is nothing to choose and the whole picker is hidden.
function renderRuleCats(checked) {
  const voc = rf.action.value === "voc";
  if (voc) rf.mode.value = "include";
  rf.mode.parentElement.classList.toggle("hidden", voc);
  rf.cats.classList.toggle("hidden", voc); // one VoC category: nothing to choose (checked box stays in the DOM for the save)
  rf.cats.innerHTML = (voc ? [rf.vocCat] : rf.allCats)
    .map((c) => `<label><input type="checkbox" value="${escapeHtml(c)}"${voc || checked.includes(c) ? " checked" : ""} /> ${escapeHtml(c)}</label>`)
    .join("");
}
const closeRuleModal = () => ruleModal.classList.add("hidden");
document.querySelector("#add-rule-btn").addEventListener("click", () => openRuleModal(-1));
document.querySelector("#rule-cancel").addEventListener("click", closeRuleModal);
ruleModal.addEventListener("click", (e) => e.target === ruleModal && closeRuleModal());
document.addEventListener("keydown", (e) => e.key === "Escape" && closeRuleModal());
document.querySelector("#rule-dest-btn").addEventListener("click", async () => {
  const dest = await openDialog({ directory: true, multiple: false });
  if (!dest) return;
  rf.destPath = dest;
  rf.dest.textContent = dest;
  refreshRulePreview();
});
rf.action.addEventListener("change", () => {
  renderRuleCats([...rf.cats.querySelectorAll("input:checked")].map((i) => i.value));
  rf.destField.classList.toggle("hidden", rf.action.value !== "move");
  refreshRulePreview();
});
[rf.mode, rf.days, rf.cats].forEach((el) => el.addEventListener("change", refreshRulePreview));
document.querySelector("#rule-save").addEventListener("click", async () => {
  const rule = ruleFromForm();
  const warn = (m) => messageDialog(m, { title: "Can't save rule", kind: "warning" });
  if (rule.action === "move" && !rule.dest) return warn("Choose a destination folder first.");
  if (!rule.categories.length && rule.mode === "include") return warn("Tick at least one category, or switch to \"Everything except these categories\".");
  const rules = [...autoRules];
  if (rf.editing >= 0) rules[rf.editing] = rule;
  else rules.push(rule);
  try {
    await saveRules(rules);
    closeRuleModal();
  } catch (e) {
    warn(`${e}`);
  }
});

function renderAutoRules() {
  const list = document.querySelector("#auto-rules-list");
  list.innerHTML = autoRules.length ? "" : '<p class="hint muted">No rules yet.</p>';
  autoRules.forEach((r, i) => {
    const what = r.mode === "exclude" ? `everything except ${r.categories.join(", ") || "nothing"}` : r.categories.join(", ");
    const row = document.createElement("div");
    row.className = "auto-rule-row";
    row.innerHTML = `
      <input type="checkbox" class="rule-on" aria-label="Enabled"${r.enabled ? " checked" : ""} />
      <span class="auto-rule-text">${r.action === "voc" ? `<strong>Process VoC</strong> ${escapeHtml(what)}` : r.action === "log-it" ? `<strong>Log-it</strong> ${escapeHtml(what)}` : `<strong>Move</strong> ${escapeHtml(what)} → <span class="muted">${escapeHtml(r.dest ?? "")}</span>`}${r.after_days ? ` · after ${r.after_days} d` : ""}</span>
      <button class="topbar-btn rule-edit">Edit</button>
      <button class="topbar-btn rule-del">Delete</button>`;
    row.querySelector(".rule-on").addEventListener("change", (e) => saveRules(autoRules.map((x, j) => (j === i ? { ...x, enabled: e.target.checked } : x))));
    row.querySelector(".rule-edit").addEventListener("click", () => openRuleModal(i));
    row.querySelector(".rule-del").addEventListener("click", async () => {
      if (await confirmDialog("Delete this rule? Files it already moved stay where they are.", { title: "Delete rule", okLabel: "Delete", cancelLabel: "Keep" })) {
        saveRules(autoRules.filter((_, j) => j !== i));
      }
    });
    list.appendChild(row);
  });
}

// Settings hosts the rules; the Transcripts page just links there.
document.querySelector("#transcripts-rules-btn").addEventListener("click", async () => {
  await showSettingsView();
  document.querySelector("#auto-rules-anchor").scrollIntoView({ block: "start" });
});

document.querySelector("#transcripts-clear-sel").addEventListener("click", () => {
  selectedTranscripts.clear();
  renderTranscripts();
});
document.querySelector("#transcripts-view-shared").addEventListener("click", async () => {
  const status = await invoke("get_audit_status");
  if (status.events_file) openPath(status.events_file).catch((e) => messageDialog(`${e}`, { title: "Couldn't open file", kind: "error" }));
  else messageDialog("Nothing has been shared yet — no category fixes or unmatched titles so far.", { title: "What's shared" });
});

function showTranscriptsView() {
  showView("transcripts");
  loadTranscripts();
}

let transcriptsScroll = 0;

// Double-click (or Enter) opens the converted markdown in the viewer; a duplicate
// opens its original (the Rust side resolves open_path). Back restores scroll.
function openTranscript(vtt) {
  const row = transcriptRows.find((r) => r.vtt === vtt);
  if (!row) return;
  if (!row.md_exists) {
    transcriptsNotice(row.open_path ? "That markdown file is missing — it was moved or deleted." : "Not converted yet — try again in a moment.");
    return;
  }
  transcriptsScroll = transcriptsList.scrollTop;
  openInViewer(row.open_path, "transcripts");
}

const onTranscriptRow = (e) => (e.target.closest("button, input, select, label") ? null : e.target.closest(".transcript-row"));
transcriptsList.addEventListener("dblclick", (e) => {
  const li = onTranscriptRow(e);
  if (li) openTranscript(li.dataset.vtt);
});
transcriptsList.addEventListener("keydown", (e) => {
  if (e.key !== "Enter") return;
  const li = onTranscriptRow(e);
  if (li) openTranscript(li.dataset.vtt);
});
transcriptsShowDups.addEventListener("change", renderTranscripts);
transcriptsShowHidden.addEventListener("change", renderTranscripts);

document.querySelector("#transcripts-setup-btn").addEventListener("click", async () => {
  const found = await invoke("autodetect_transcripts");
  let path = null;
  if (found && (await confirmDialog(`Use this folder?\n\n${found}`, { title: "Transcripts folder", okLabel: "Use this folder", cancelLabel: "Choose another…" }))) {
    path = found;
  } else {
    path = await openDialog({ directory: true, multiple: false });
  }
  if (!path) return;
  try {
    await invoke("set_transcripts_path", { path });
  } catch (e) {
    messageDialog(`${e}`, { title: "Couldn't use that folder", kind: "error" });
    return;
  }
  await loadTranscripts();
  transcriptsNotice("Converting transcripts…");
});
transcriptsNotNowBtn.addEventListener("click", async () => {
  await invoke("dismiss_transcripts");
  transcriptsNotNowBtn.classList.add("hidden");
});

// --- Markdown viewer (read-only) ---

let currentViewerPath = null;
let viewerReturnTo = "search"; // "search" | "map" | "transcripts" — which view the viewer's Back button returns to

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

async function openInViewer(path, returnTo = "search") {
  viewerReturnTo = returnTo;
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

  // Opening it externally would just bounce back here (this app is the OS
  // default handler), so hide the button rather than have it look inert.
  // `is_default_md_handler` returns null on Windows (no query available) —
  // keep the button in that case, matching the settings section's fallback.
  const isDefault = await invoke("is_default_md_handler");
  viewerOpenExternalBtn.classList.toggle("hidden", isDefault === true);

  const sourceDir = doc.source_path.slice(0, doc.source_path.lastIndexOf("/"));
  for (const img of viewerContentEl.querySelectorAll("img")) {
    const src = img.getAttribute("src");
    if (src) img.src = convertFileSrc(resolveImageSrc(src, sourceDir));
  }

  exitEdit();
  viewerEditBtn.classList.toggle("hidden", !(await invoke("can_edit_markdown", { path: doc.source_path })));
  showViewer();
}

function showViewer() {
  showView("viewer");
}

// --- Source editing: plain textarea of the raw body (frontmatter held aside and
// written back verbatim, so [[wikilinks]] and metadata round-trip untouched).
// Explicit Save only (button or Cmd/Ctrl+S) — no autosave/undo, per v0.3.0. ---

const viewerEditBtn = document.querySelector("#viewer-edit-btn");
const viewerSaveBtn = document.querySelector("#viewer-save-btn");
const viewerCancelBtn = document.querySelector("#viewer-cancel-btn");
const viewerEditor = document.querySelector("#viewer-editor");
let editHeader = "";
let editOriginal = "";

const isEditing = () => !viewerEditor.classList.contains("hidden");
const isDirty = () => isEditing() && viewerEditor.value !== editOriginal;

function exitEdit() {
  viewerEditor.classList.add("hidden");
  viewerContentEl.classList.remove("hidden");
  viewerSaveBtn.classList.add("hidden");
  viewerCancelBtn.classList.add("hidden");
  viewerEditBtn.classList.remove("hidden");
}

async function startEdit() {
  let doc;
  try {
    doc = await invoke("read_for_edit", { path: currentViewerPath });
  } catch (e) {
    messageDialog(`${e}`, { title: "Couldn't open for editing", kind: "error" });
    return;
  }
  editHeader = doc.header;
  editOriginal = doc.body;
  viewerEditor.value = doc.body;
  viewerContentEl.classList.add("hidden");
  viewerEditor.classList.remove("hidden");
  viewerEditBtn.classList.add("hidden");
  viewerSaveBtn.classList.remove("hidden");
  viewerCancelBtn.classList.remove("hidden");
  viewerEditor.focus();
}

async function saveEdit() {
  if (!isEditing()) return;
  const body = viewerEditor.value;
  try {
    await invoke("write_for_edit", { path: currentViewerPath, header: editHeader, body });
  } catch (e) {
    // Keep the buffer open so nothing typed is lost (e.g. read-only wikis/ for non-owners).
    messageDialog(`${e}`, { title: "Couldn't save", kind: "error" });
    return;
  }
  await openInViewer(currentViewerPath, viewerReturnTo); // re-render; also leaves edit mode
}

// Resolves false if the user chose to keep editing. Native dialog: the webview's
// window.confirm() is silently ignored (returns false) in Tauri.
async function confirmLeaveEdit() {
  return !isDirty() || confirmDialog("Discard unsaved changes?", { title: "Unsaved changes", okLabel: "Discard", cancelLabel: "Keep editing" });
}

// --- Vault Neural Map: force-directed graph of [[wikilink]] cross-references,
// node radius by word count. Physics tuned against this vault's real link
// density (denser than a single-vault slice usually is) — see graph.rs /
// get_vault_graph for the node/edge source. ---

const mapCanvas = document.querySelector("#map-canvas");
const mapCtx = mapCanvas.getContext("2d");
const mapTooltip = document.querySelector("#map-tooltip");
const mapStatsEl = document.querySelector("#map-stats");

const MAP_ACCENT = "#396cd8";
const MAP_REPULSION_K = 7500,
  MAP_SPRING_K = 0.015,
  MAP_REST_LENGTH = 160,
  MAP_CENTER_K = 0.006,
  MAP_VELOCITY_DECAY = 0.6,
  MAP_ALPHA_DECAY = 0.0175,
  MAP_ALPHA_MIN = 0.001,
  MAP_MAX_SPEED = 40,
  MAP_COLLIDE_PAD = 14,
  MAP_COLLIDE_K = 0.9,
  MAP_RMIN = 4,
  MAP_RMAX = 18;

const mapState = {
  nodes: [],
  edges: [],
  adjacency: new Map(),
  view: { x: 0, y: 0, k: 1 },
  alpha: 1,
  dragNode: null,
  panning: false,
  panStart: null,
  viewStart: null,
  hovered: null,
  selected: null,
  dpr: 1,
  running: false,
};

function mapRadiusFor(words, wMin, wMax) {
  if (wMax === wMin) return (MAP_RMIN + MAP_RMAX) / 2;
  const t = Math.sqrt((words - wMin) / (wMax - wMin));
  return MAP_RMIN + t * (MAP_RMAX - MAP_RMIN);
}

function mapTick() {
  const s = mapState;
  s.alpha = Math.max(MAP_ALPHA_MIN, s.alpha * (1 - MAP_ALPHA_DECAY));
  for (let i = 0; i < s.nodes.length; i++) {
    for (let j = i + 1; j < s.nodes.length; j++) {
      const a = s.nodes[i],
        b = s.nodes[j];
      const dx = a.x - b.x,
        dy = a.y - b.y;
      const distSq = dx * dx + dy * dy || 0.01,
        dist = Math.sqrt(distSq);
      const f = (MAP_REPULSION_K / distSq) * s.alpha;
      const fx = (dx / dist) * f,
        fy = (dy / dist) * f;
      a.vx += fx;
      a.vy += fy;
      b.vx -= fx;
      b.vy -= fy;
      const minDist = a.r + b.r + MAP_COLLIDE_PAD;
      if (dist < minDist) {
        const push = (minDist - dist) * MAP_COLLIDE_K;
        const cfx = (dx / dist) * push,
          cfy = (dy / dist) * push;
        a.vx += cfx;
        a.vy += cfy;
        b.vx -= cfx;
        b.vy -= cfy;
      }
    }
  }
  for (const e of s.edges) {
    const dx = e.t.x - e.s.x,
      dy = e.t.y - e.s.y;
    const dist = Math.sqrt(dx * dx + dy * dy) || 0.01;
    const f = MAP_SPRING_K * (dist - MAP_REST_LENGTH) * s.alpha;
    const fx = (dx / dist) * f,
      fy = (dy / dist) * f;
    e.s.vx += fx;
    e.s.vy += fy;
    e.t.vx -= fx;
    e.t.vy -= fy;
  }
  for (const n of s.nodes) {
    if (n.fx != null) {
      n.x = n.fx;
      n.y = n.fy;
      n.vx = 0;
      n.vy = 0;
      continue;
    }
    n.vx += -n.x * MAP_CENTER_K * s.alpha;
    n.vy += -n.y * MAP_CENTER_K * s.alpha;
    n.vx *= MAP_VELOCITY_DECAY;
    n.vy *= MAP_VELOCITY_DECAY;
    const speed = Math.hypot(n.vx, n.vy);
    if (speed > MAP_MAX_SPEED) {
      n.vx *= MAP_MAX_SPEED / speed;
      n.vy *= MAP_MAX_SPEED / speed;
    }
    n.x += n.vx;
    n.y += n.vy;
  }
}

function mapScreenToWorld(sx, sy) {
  const v = mapState.view;
  return [(sx - v.x) / v.k, (sy - v.y) / v.k];
}

function mapHitTest(sx, sy) {
  const [wx, wy] = mapScreenToWorld(sx, sy);
  let best = null,
    bestD = Infinity;
  for (const n of mapState.nodes) {
    const d = (wx - n.x) ** 2 + (wy - n.y) ** 2;
    const rr = (n.r + 3) ** 2;
    if (d <= rr && d < bestD) {
      best = n;
      bestD = d;
    }
  }
  return best;
}

function mapFitToView() {
  const rect = mapCanvas.parentElement.getBoundingClientRect();
  let minX = Infinity,
    minY = Infinity,
    maxX = -Infinity,
    maxY = -Infinity;
  for (const n of mapState.nodes) {
    minX = Math.min(minX, n.x - n.r);
    maxX = Math.max(maxX, n.x + n.r);
    minY = Math.min(minY, n.y - n.r);
    maxY = Math.max(maxY, n.y + n.r);
  }
  if (!isFinite(minX)) return;
  const w = Math.max(1, maxX - minX),
    h = Math.max(1, maxY - minY);
  const k = Math.min(4, Math.max(0.2, Math.min(rect.width / w, rect.height / h) * 0.82));
  mapState.view.k = k;
  mapState.view.x = rect.width / 2 - ((minX + maxX) / 2) * k;
  mapState.view.y = rect.height / 2 - ((minY + maxY) / 2) * k;
}

function mapDraw() {
  const rect = mapCanvas.parentElement.getBoundingClientRect();
  const dpr = mapState.dpr,
    v = mapState.view;
  const isDark = window.matchMedia("(prefers-color-scheme: dark)").matches;
  const ink = isDark ? "#f0f0f0" : "#0f0f0f";
  const inkMuted = isDark ? "#aaa" : "#777";
  const border = isDark ? "rgba(240,240,240,0.14)" : "rgba(15,15,15,0.12)";

  mapCtx.setTransform(dpr, 0, 0, dpr, 0, 0);
  mapCtx.clearRect(0, 0, rect.width, rect.height);
  mapCtx.setTransform(v.k * dpr, 0, 0, v.k * dpr, v.x * dpr, v.y * dpr);

  const selected = mapState.selected,
    hovered = mapState.hovered;
  const dimming = !!selected;
  const activeSet = selected ? mapState.adjacency.get(selected.id) : null;

  mapCtx.lineWidth = 1 / v.k;
  for (const e of mapState.edges) {
    const involved = selected && (e.s === selected || e.t === selected);
    mapCtx.strokeStyle = dimming ? (involved ? MAP_ACCENT : border) : border;
    mapCtx.globalAlpha = dimming && !involved ? 0.15 : involved ? 0.65 : 1;
    mapCtx.beginPath();
    mapCtx.moveTo(e.s.x, e.s.y);
    mapCtx.lineTo(e.t.x, e.t.y);
    mapCtx.stroke();
  }
  mapCtx.globalAlpha = 1;

  for (const n of mapState.nodes) {
    const dim = dimming && n !== selected && !(activeSet && activeSet.has(n.id));
    mapCtx.globalAlpha = dim ? 0.3 : 1;
    mapCtx.beginPath();
    mapCtx.arc(n.x, n.y, n.r, 0, Math.PI * 2);
    mapCtx.fillStyle = MAP_ACCENT;
    mapCtx.fill();
    if (n === selected || n === hovered) {
      mapCtx.lineWidth = 1.5 / v.k;
      mapCtx.strokeStyle = ink;
      mapCtx.stroke();
    }
    mapCtx.globalAlpha = 1;
  }

  const showLabels = v.k > 0.9;
  mapCtx.font = 11 / v.k + "px " + getComputedStyle(document.body).fontFamily;
  mapCtx.textBaseline = "top";
  for (const n of mapState.nodes) {
    const isFocus = n === hovered || n === selected;
    if (!showLabels && !isFocus) continue;
    const dim = dimming && n !== selected && !(activeSet && activeSet.has(n.id));
    if (dim && !isFocus) continue;
    mapCtx.fillStyle = isFocus ? ink : inkMuted;
    mapCtx.fillText(n.id, n.x + n.r + 4 / v.k, n.y - 5 / v.k);
  }
}

function mapFrame() {
  if (!mapState.running) return;
  const active = mapState.dragNode || mapState.alpha > MAP_ALPHA_MIN;
  if (active) mapTick();
  mapDraw();
  requestAnimationFrame(mapFrame);
}

function mapResize() {
  mapState.dpr = Math.max(1, window.devicePixelRatio || 1);
  const rect = mapCanvas.parentElement.getBoundingClientRect();
  mapCanvas.width = rect.width * mapState.dpr;
  mapCanvas.height = rect.height * mapState.dpr;
  mapCanvas.style.width = rect.width + "px";
  mapCanvas.style.height = rect.height + "px";
}

function rebuildMapSim(graphData) {
  const byId = new Map();
  const words = graphData.nodes.map((n) => n.words);
  const wMin = Math.min(...words, 0),
    wMax = Math.max(...words, 0);
  const nodes = graphData.nodes.map((n) => {
    const angle = Math.random() * Math.PI * 2,
      r = Math.random() * 500;
    const node = {
      id: n.id,
      words: n.words,
      path: n.path,
      r: mapRadiusFor(n.words, wMin, wMax),
      x: Math.cos(angle) * r,
      y: Math.sin(angle) * r,
      vx: 0,
      vy: 0,
      fx: null,
      fy: null,
      degree: 0,
    };
    byId.set(n.id, node);
    return node;
  });
  const edges = graphData.edges
    .filter((e) => byId.has(e.source) && byId.has(e.target))
    .map((e) => ({ s: byId.get(e.source), t: byId.get(e.target) }));
  edges.forEach((e) => {
    e.s.degree++;
    e.t.degree++;
  });
  const adjacency = new Map();
  nodes.forEach((n) => adjacency.set(n.id, new Set()));
  edges.forEach((e) => {
    adjacency.get(e.s.id).add(e.t.id);
    adjacency.get(e.t.id).add(e.s.id);
  });

  mapState.nodes = nodes;
  mapState.edges = edges;
  mapState.adjacency = adjacency;
  mapState.alpha = 1;
  mapState.selected = null;
  mapState.hovered = null;
  mapResize();
  for (let i = 0; i < 700; i++) mapTick();
  mapFitToView();
  mapStatsEl.textContent = `${nodes.length} wikis · ${edges.length} links`;
  mapDraw();
}

async function showMapView() {
  showView("map");
  const graphData = await invoke("get_vault_graph");
  rebuildMapSim(graphData);
  if (!mapState.running) {
    mapState.running = true;
    requestAnimationFrame(mapFrame);
  }
}

new ResizeObserver(() => {
  if (!mapView.classList.contains("hidden")) {
    mapResize();
    mapDraw();
  }
}).observe(mapCanvas.parentElement);

mapCanvas.addEventListener("mousedown", (e) => {
  const rect = mapCanvas.getBoundingClientRect();
  const sx = e.clientX - rect.left,
    sy = e.clientY - rect.top;
  const hit = mapHitTest(sx, sy);
  if (hit) {
    mapState.dragNode = hit;
    const [wx, wy] = mapScreenToWorld(sx, sy);
    hit.fx = wx;
    hit.fy = wy;
    mapState.alpha = Math.max(mapState.alpha, 0.3);
  } else {
    mapState.panning = true;
    mapState.panStart = [sx, sy];
    mapState.viewStart = [mapState.view.x, mapState.view.y];
  }
  mapCanvas.classList.add("dragging");
});
window.addEventListener("mousemove", (e) => {
  if (mapView.classList.contains("hidden")) return;
  const rect = mapCanvas.getBoundingClientRect();
  const sx = e.clientX - rect.left,
    sy = e.clientY - rect.top;
  if (mapState.dragNode) {
    const [wx, wy] = mapScreenToWorld(sx, sy);
    mapState.dragNode.fx = wx;
    mapState.dragNode.fy = wy;
    return;
  }
  if (mapState.panning) {
    mapState.view.x = mapState.viewStart[0] + (sx - mapState.panStart[0]);
    mapState.view.y = mapState.viewStart[1] + (sy - mapState.panStart[1]);
    return;
  }
  if (sx < 0 || sy < 0 || sx > rect.width || sy > rect.height) {
    if (mapState.hovered) {
      mapState.hovered = null;
      mapTooltip.hidden = true;
    }
    return;
  }
  const hit = mapHitTest(sx, sy);
  mapState.hovered = hit;
  if (hit) {
    mapTooltip.hidden = false;
    mapTooltip.style.left = sx + 14 + "px";
    mapTooltip.style.top = sy + 14 + "px";
    mapTooltip.innerHTML =
      `<div class="tt-name">${escapeHtml(hit.id)}</div>` +
      `<div class="tt-meta">${hit.words.toLocaleString()} words · ${hit.degree} link${hit.degree === 1 ? "" : "s"}</div>`;
  } else {
    mapTooltip.hidden = true;
  }
});
window.addEventListener("mouseup", () => {
  if (mapState.dragNode) {
    mapState.dragNode.fx = null;
    mapState.dragNode.fy = null;
    mapState.dragNode = null;
  }
  mapState.panning = false;
  mapCanvas.classList.remove("dragging");
});
mapCanvas.addEventListener("click", (e) => {
  const rect = mapCanvas.getBoundingClientRect();
  const hit = mapHitTest(e.clientX - rect.left, e.clientY - rect.top);
  mapState.selected = hit && hit === mapState.selected ? null : hit;
});
mapCanvas.addEventListener("dblclick", (e) => {
  const rect = mapCanvas.getBoundingClientRect();
  const hit = mapHitTest(e.clientX - rect.left, e.clientY - rect.top);
  if (hit) openInViewer(hit.path, "map");
});
mapCanvas.addEventListener(
  "wheel",
  (e) => {
    e.preventDefault();
    const rect = mapCanvas.getBoundingClientRect();
    const sx = e.clientX - rect.left,
      sy = e.clientY - rect.top;
    const [wx, wy] = mapScreenToWorld(sx, sy);
    const factor = Math.exp(-e.deltaY * 0.001);
    mapState.view.k = Math.min(4, Math.max(0.2, mapState.view.k * factor));
    mapState.view.x = sx - wx * mapState.view.k;
    mapState.view.y = sy - wy * mapState.view.k;
  },
  { passive: false },
);

// --- Skills catalog: every skill in the shared vault's skills-repo,
// searchable, one click to run in Claude Code. ---

const skillsList = document.querySelector("#skills-list");
const skillsSearchInput = document.querySelector("#skills-search");
const skillsUpdateBanner = document.querySelector("#update-banner");
const skillsUpdateBannerCount = document.querySelector("#update-banner-count");
const SKILLS_RECENT_COUNT = 8;
let allSkills = [];

// Subsequence match: every query char appears in order in the candidate
// text; score = matched span length (lower = tighter match). No library
// needed for ~150 short strings.
function fuzzyScore(query, text) {
  query = query.toLowerCase();
  text = text.toLowerCase();
  let cursor = 0,
    first = -1,
    last = -1;
  for (const ch of query) {
    const idx = text.indexOf(ch, cursor);
    if (idx === -1) return null;
    if (first === -1) first = idx;
    last = idx;
    cursor = idx + 1;
  }
  return last - first;
}

function renderSkillRows(rows, showMoreBtn = false) {
  skillsList.innerHTML = "";
  if (rows.length === 0) {
    skillsList.innerHTML = '<li class="no-results">No skills match that search.</li>';
    return;
  }
  for (const skill of rows) {
    const li = document.createElement("li");
    li.className = "skill-row";
    li.innerHTML = `
      <div class="skill-row-head">
        <span class="skill-name">${escapeHtml(skill.name)}</span>
        <button class="skill-run-btn" data-skill="${escapeHtml(skill.name)}">Run</button>
      </div>
      <span class="skill-plugin">${escapeHtml(skill.plugin)}</span>
      <p class="skill-desc">${escapeHtml(skill.description)}</p>
    `;
    skillsList.appendChild(li);
  }
  if (showMoreBtn) {
    const li = document.createElement("li");
    li.className = "show-all-row";
    li.innerHTML = `<button class="show-all-btn">Show all skills (${allSkills.length - SKILLS_RECENT_COUNT} more)</button>`;
    skillsList.appendChild(li);
  }
}

// Default view: 8 most recent + "Show all" button (if there are more).
function renderSkillsDefault() {
  renderSkillRows(allSkills.slice(0, SKILLS_RECENT_COUNT), allSkills.length > SKILLS_RECENT_COUNT);
}

function applySkillsSearch() {
  const q = skillsSearchInput.value.trim();
  if (!q) {
    renderSkillsDefault();
    return;
  }
  const scored = allSkills
    .map((s) => ({ s, score: fuzzyScore(q, `${s.name} ${s.plugin} ${s.description}`) }))
    .filter((m) => m.score !== null)
    .sort((a, b) => a.score - b.score)
    .map((m) => m.s);
  renderSkillRows(scored);
}

async function showSkillsView() {
  allSkills = await invoke("get_skills"); // already sorted newest-first
  skillsSearchInput.value = "";
  renderSkillsDefault(); // re-entering the page always resets to the default 8
  showView("skills");
  skillsSearchInput.focus();
}

skillsSearchInput.addEventListener("input", applySkillsSearch);

skillsList.addEventListener("click", (e) => {
  if (e.target.closest(".show-all-btn")) {
    renderSkillRows(allSkills);
    skillsList.children[SKILLS_RECENT_COUNT]?.scrollIntoView({ behavior: "smooth", block: "start" });
    return;
  }
  const btn = e.target.closest(".skill-run-btn");
  if (btn) launchClaudeCommand(`/${btn.dataset.skill}`);
});

// Manual-refresh loading overlay — gated behind the click so it only covers
// the window for a user-initiated reindex, not the sub-300ms background
// reindexes a file-watcher event triggers on its own.
const loadingOverlay = document.querySelector("#loading-overlay");
const reindexBtn = document.querySelector("#reindex-btn");
let awaitingManualRefresh = false;

function renderStatus(status) {
  if (status.state === "indexing") {
    statusEl.textContent = "Indexing…";
  } else if (status.state === "error") {
    statusEl.textContent = "Index error — check Logs";
  } else {
    statusEl.textContent = `${status.count} docs indexed`;
  }

  if (awaitingManualRefresh && status.state !== "indexing") {
    awaitingManualRefresh = false;
    loadingOverlay.classList.add("hidden");
    reindexBtn.disabled = false;
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

const skillsBadge = document.querySelector("#skills-badge");
let pluginDrift = [];

// Always visible, always opens the Skills view — icon/tooltip and the
// Skills-page banner are the only things that change with drift state.
function renderSkillsBadge(drift) {
  pluginDrift = drift;
  const isDrifted = drift.length > 0;
  skillsBadge.textContent = isDrifted ? "✨ Skills" : "🧩 Skills";
  const tooltip = isDrifted
    ? `${drift.length} skill plugin(s) are behind the shared vault — update from the Skills page.`
    : "See all skills";
  skillsBadge.title = tooltip;
  skillsBadge.dataset.tooltip = tooltip;
  skillsUpdateBannerCount.textContent = drift.length;
  skillsUpdateBanner.classList.toggle("hidden", !isDrifted);
}

async function checkPluginDrift() {
  renderSkillsBadge(await invoke("get_plugin_drift"));
}

skillsBadge.addEventListener("click", showSkillsView);
document.querySelector("#skills-back-btn").addEventListener("click", showSearchView);
document.querySelector("#update-banner-btn").addEventListener("click", () => {
  launchClaudeCommand(`Update these outdated Second Brain plugins: ${pluginDrift.map((d) => d.key).join(", ")}`);
});

// --- Auto-update (checks on launch, install is user-initiated) ---

const appUpdateBanner = document.querySelector("#app-update-banner");
const appUpdateBtn = document.querySelector("#app-update-btn");
let pendingUpdate = null;

async function checkForAppUpdate() {
  try {
    pendingUpdate = await checkForUpdate();
  } catch (e) {
    console.log("[update] check failed:", e);
    return;
  }
  if (pendingUpdate) appUpdateBanner.classList.remove("hidden");
}

appUpdateBtn.addEventListener("click", async () => {
  if (!pendingUpdate || appUpdateBtn.disabled) return;
  appUpdateBtn.disabled = true;
  appUpdateBtn.textContent = "Installing…";
  try {
    await pendingUpdate.downloadAndInstall();
    await relaunch();
  } catch (e) {
    appUpdateBtn.disabled = false;
    appUpdateBtn.textContent = "Update & Restart";
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
    showSettingsView();
  }

  checkForAppUpdate();
  checkPluginDrift();

  listen("index-status", (event) => renderStatus(event.payload));
  listen("raw-count-status", (event) => renderRawCount(event.payload));
  listen("inbox-count-status", (event) => renderInboxCount(event.payload));
  listen("shared-raw-count-status", (event) => renderSharedRawCount(event.payload));
  // Pushed by the Rust side after every reindex (launch, manual, or file-watcher
  // triggered) — live-updates the map only while it's actually the visible view.
  listen("graph-updated", (event) => {
    if (!mapView.classList.contains("hidden")) rebuildMapSim(event.payload);
  });

  // The OS handed this launch a file to open (double-click / "Open With",
  // once this app is a registered .md handler) — covers the already-running
  // case (single-instance forwarding, or a later macOS open-file event).
  listen("open-file-request", (event) => openInViewer(event.payload));
  // Covers a cold Windows/Linux launch: the frontend wasn't listening yet
  // when the Rust side first saw the file, so it's stashed for one pickup.
  invoke("get_launch_file_path").then((path) => {
    if (path) openInViewer(path);
  });

  settingsBackBtn.addEventListener("click", showSearchView);

  document.querySelector("#settings-badge").addEventListener("click", showSettingsView);

  document.querySelector("#reindex-btn").addEventListener("click", () => {
    awaitingManualRefresh = true;
    loadingOverlay.classList.remove("hidden");
    reindexBtn.disabled = true;
    invoke("reindex_now");
    invoke("refresh_plugin_drift").then(renderSkillsBadge);
  });

  document.querySelector("#transcripts-btn").addEventListener("click", showTranscriptsView);
  document.querySelector("#transcripts-back-btn").addEventListener("click", showSearchView);
  invoke("get_config").then(applyTranscriptsNav);
  listen("transcripts-updated", () => {
    if (!transcriptsView.classList.contains("hidden")) loadTranscripts();
  });

  document.querySelector("#map-btn").addEventListener("click", showMapView);
  document.querySelector("#map-back-btn").addEventListener("click", showSearchView);

  document.querySelector("#export-logs-btn").addEventListener("click", async () => {
    const dir = await invoke("log_dir_path");
    revealItemInDir(dir);
  });

  searchInput.addEventListener("input", () => {
    clearTimeout(debounceTimer);
    debounceTimer = setTimeout(runSearch, 200);
  });

  viewerEditBtn.addEventListener("click", startEdit);
  viewerSaveBtn.addEventListener("click", saveEdit);
  viewerCancelBtn.addEventListener("click", async () => (await confirmLeaveEdit()) && exitEdit());
  document.addEventListener("keydown", (e) => {
    if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "s" && isEditing()) {
      e.preventDefault();
      saveEdit();
    }
  });
  document.querySelector("#viewer-back-btn").addEventListener("click", async () => {
    if (!(await confirmLeaveEdit())) return;
    exitEdit();
    if (viewerReturnTo === "map") showMapView();
    else if (viewerReturnTo === "transcripts") {
      // List DOM is untouched while the viewer is up, so selection/filters survive; only scroll resets.
      showView("transcripts");
      transcriptsList.scrollTop = transcriptsScroll;
    } else showSearchView();
  });
  document.querySelector("#viewer-open-external-btn").addEventListener("click", () => {
    if (currentViewerPath) {
      openPath(currentViewerPath).catch((e) => alert(`Couldn't open file:\n${currentViewerPath}\n\n${e}`));
    }
  });
});
