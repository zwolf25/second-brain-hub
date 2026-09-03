const { invoke, convertFileSrc } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { open: openDialog } = window.__TAURI__.dialog;
const { openPath, openUrl, revealItemInDir } = window.__TAURI__.opener;
const { check: checkForUpdate } = window.__TAURI__.updater;
const { relaunch } = window.__TAURI__.process;

const onboarding = document.querySelector("#onboarding");
const searchView = document.querySelector("#search-view");
const viewerView = document.querySelector("#viewer-view");
const mapView = document.querySelector("#map-view");
const onboardingError = document.querySelector("#onboarding-error");
const doneSettingsBtn = document.querySelector("#done-settings-btn");
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
  mapView.classList.add("hidden");
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
  mapView.classList.add("hidden");
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
let viewerReturnTo = "search"; // "search" | "map" — which view the viewer's Back button returns to

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

  showViewer();
}

function showViewer() {
  onboarding.classList.add("hidden");
  searchView.classList.add("hidden");
  viewerView.classList.remove("hidden");
  mapView.classList.add("hidden");
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
  onboarding.classList.add("hidden");
  searchView.classList.add("hidden");
  viewerView.classList.add("hidden");
  mapView.classList.remove("hidden");
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

  doneSettingsBtn.addEventListener("click", showSearchView);

  document.querySelector("#change-vault-btn").addEventListener("click", showOnboarding);

  document.querySelector("#reindex-btn").addEventListener("click", () => {
    invoke("reindex_now");
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

  document.querySelector("#viewer-back-btn").addEventListener("click", () => {
    if (viewerReturnTo === "map") showMapView();
    else showSearchView();
  });
  document.querySelector("#viewer-open-external-btn").addEventListener("click", () => {
    if (currentViewerPath) {
      openPath(currentViewerPath).catch((e) => alert(`Couldn't open file:\n${currentViewerPath}\n\n${e}`));
    }
  });
});
