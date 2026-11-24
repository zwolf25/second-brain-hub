// Pure helpers, split out of main.js so `node --test` (src/lib.test.js) can cover them without a DOM or Tauri.

export const STALE_DAYS = 90;

// Subsequence match: every query char appears in order in the candidate
// text; score = matched span length (lower = tighter match). No library
// needed for ~150 short strings.
export function fuzzyScore(query, text) {
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

// Header/banner/settings wording from a Rust onedrive::Status. Null status = not checked yet.
export function onedriveSummary(status) {
  if (!status) return { worst: "ok", pill: "", banner: "", setting: "Checking…" };
  const top = status.issues.find((i) => i.severity === status.worst) ?? status.issues[0];
  const pill = { red: "OneDrive: problem", yellow: "OneDrive: check" }[status.worst] ?? "";
  const info = status.limited ? "Healthy (limited checks on Windows)" : "Healthy";
  return {
    worst: status.worst,
    pill,
    banner: status.worst === "red" && top ? `OneDrive needs attention: ${top.title.toLowerCase()}.` : "",
    setting: status.worst === "ok" ? info : top ? `${status.worst === "red" ? "Problem" : "Check"}: ${top.title}` : info,
  };
}

// Plain text a collaborator can paste to Zac when a fix doesn't work.
export function onedriveDetails(status, version = "") {
  if (!status) return "OneDrive status not checked yet.";
  const lines = [`Second Brain Hub ${version} OneDrive details (${status.platform}), ${new Date(status.checked_at * 1000).toISOString()}`];
  for (const c of status.checks) lines.push(`${{ g: "OK", y: "WARN", r: "FAIL", x: "n/a" }[c.state]}: ${c.label}`);
  for (const i of status.issues) lines.push(`[${i.severity}] ${i.code}: ${i.title}`);
  return lines.join("\n");
}

// Escapes the three characters that matter inside element text (same set the old DOM-based version did).
export function escapeHtml(s) {
  return String(s ?? "")
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;");
}

// A date-only frontmatter value is a local day, not UTC midnight.
const parseUpdated = (iso) => new Date(iso.length > 10 ? iso : `${iso}T00:00:00`);

// "Today" / "Yesterday" / "Oct 28" / "Oct 28, 2025" from a frontmatter `updated` value.
export function formatUpdated(iso, now = new Date()) {
  if (!iso) return "";
  const d = parseUpdated(iso);
  if (isNaN(d)) return iso;
  const days = Math.round((new Date(now).setHours(0, 0, 0, 0) - new Date(d).setHours(0, 0, 0, 0)) / 864e5);
  if (days === 0) return "Today";
  if (days === 1) return "Yesterday";
  const sameYear = d.getFullYear() === now.getFullYear();
  return d.toLocaleDateString(undefined, { month: "short", day: "numeric", ...(sameYear ? {} : { year: "numeric" }) });
}

export const isStale = (updated, now = Date.now()) => !!updated && now - parseUpdated(updated).getTime() > STALE_DAYS * 864e5;

// --- F-12 result filter / sort. Results carry `topics` (string[]). ---

// Distinct topics across the results, sorted.
export function topicValues(results) {
  return [...new Set(results.flatMap((r) => r.topics ?? []))].sort((a, b) => a.localeCompare(b));
}

export const filterByTopic = (results, topic) => (topic ? results.filter((r) => (r.topics ?? []).includes(topic)) : results);

// "relevance" keeps the backend order; "newest"/"oldest" sort by `updated` (ISO strings sort lexically), undated pages last either way.
export function sortResults(results, sort) {
  if (sort !== "newest" && sort !== "oldest") return results;
  const dir = sort === "newest" ? -1 : 1;
  return [...results].sort((a, b) => {
    if (!a.updated || !b.updated) return !a.updated - !b.updated; // undated last
    return dir * a.updated.localeCompare(b.updated);
  });
}

// Pipeline page: split the staleness outputs into the three flow-order tabs (stale rows first, most new evidence first).
export function pipelineTabs(outputs = []) {
  const tabs = { wl: [], pmd: [], comp: [] };
  for (const o of outputs) {
    const t = o.name.startsWith("Win/Loss") ? "wl" : o.name.startsWith("PMD") ? "pmd" : "comp";
    tabs[t].push(o);
  }
  for (const k of Object.keys(tabs)) tabs[k].sort((a, b) => Number(b.stale) - Number(a.stale) || b.new_upstream - a.new_upstream || a.name.localeCompare(b.name));
  return tabs;
}

// VoC priority from the deal size in the "$1,234" text: High from 150K, Med from 50K.
export function vocPriority(amount) {
  const n = Number(String(amount).replace(/[^0-9.]/g, "")) || 0;
  return { n, label: n >= 150000 ? "High" : n >= 50000 ? "Med" : "Low" };
}

// Declined rows hide unless that chip is chosen; other chips filter by kind.
export function vocVisible(rows, filter) {
  if (filter === "declined") return rows.filter((r) => r.declined);
  return rows.filter((r) => !r.declined && (filter === "all" || r.kind === filter));
}

// ---- VoC ordering (0.20.0): type first (new-logo loss > churn > new-logo win > expansion > renewal > services), then deal size, then recency
export const VOC_TYPES = { loss: "New-logo loss", churn: "Churn", win: "New-logo win", expansion: "Expansion", renewal: "Renewal", services: "Services" };
const TYPE_RANK = { loss: 100, churn: 80, win: 60, expansion: 40, renewal: 20, services: 5 };
export function vocType(r) {
  if (r.kind === "churn") return "churn";
  const t = r.type || "new_logo";
  if (t === "new_logo") return r.kind === "loss" ? "loss" : "win";
  return t in TYPE_RANK ? t : "expansion";
}
export function vocValue(r) {
  return typeof r.acv === "number" ? r.acv : vocPriority(r.amount).n;
}
export function vocShown(r) {
  return r.amount_flag && typeof r.acv !== "number" ? "value unverified" : vocMoney(vocValue(r));
}
export function vocMoney(n) {
  if (!n) return "no amount";
  return n >= 1e6 ? `$${(n / 1e6).toFixed(1)}M` : n >= 1e3 ? `$${Math.round(n / 1e3)}K` : `$${Math.round(n)}`;
}
export function vocDaysLeft(r, today = Date.now(), windowDays = 90) {
  return Math.max(0, Math.round(windowDays - (today - Date.parse(r.date)) / 864e5));
}
export function vocScore(r, today = Date.now(), windowDays = 90) {
  const v = Math.min(Math.max(vocValue(r), 1000), 1e6);
  const value = ((Math.log10(v) - 3) / 3) * 30;
  const age = (today - Date.parse(r.date)) / 864e5;
  const left = windowDays - (Number.isFinite(age) ? age : 0);
  const recency = left <= 30 ? Math.min(10, ((30 - Math.max(left, 0)) / 30) * 10) : 0;
  return TYPE_RANK[vocType(r)] + value + recency;
}
function vocHay(r) {
  const c = r.contact || {};
  return [r.account, r.opp_name, VOC_TYPES[vocType(r)], r.why, r.reason_text, r.competitor, r.sales_rep, r.csm, c.name, c.title, c.email, r.date, r.amount, vocShown(r), r.claimed_by, r.requested_by, r.status]
    .filter(Boolean).join(" ").toLowerCase();
}
/** One entry per account: best candidate in `best`, the account's other deals in `others`. */
export function vocBuild(rows, { type = "all", query = "", services = false, sort = "priority", mine = "", today = Date.now() } = {}) {
  const terms = query.toLowerCase().split(/\s+/).filter(Boolean);
  const pool = rows.filter((r) => (!mine || r.claimed_by === mine || r.requested_by === mine)
    && (type === "declined" ? r.declined : !r.declined && !/^(done|completed)/i.test(r.status || ""))
    && (services || type === "services" || vocType(r) !== "services")
    && (type === "all" || type === "declined" || (type === "claimed" ? r.claimed_by || r.requested_by : vocType(r) === type)));
  const byAcct = new Map();
  for (const r of pool) {
    const k = r.account.toLowerCase();
    if (!byAcct.has(k)) byAcct.set(k, []);
    byAcct.get(k).push(r);
  }
  const groups = [];
  for (const list of byAcct.values()) {
    list.sort((a, b) => vocScore(b, today) - vocScore(a, today));
    const hay = list.map(vocHay).join(" ");
    if (terms.every((t) => hay.includes(t))) groups.push({ best: list[0], others: list.slice(1), score: vocScore(list[0], today) });
  }
  const parked = (g) => (g.best.claimed_by || g.best.requested_by ? 1 : 0);
  const by = sort === "days" ? (a, b) => vocDaysLeft(a.best, today) - vocDaysLeft(b.best, today) : (a, b) => b.score - a.score;
  return groups.sort((a, b) => parked(a) - parked(b) || by(a, b));
}

/** Plain-language text for an updater failure; the raw error stays in the console. */
export function updateErrorText(e) {
  const m = String(e?.message ?? e);
  if (/fallback platforms|platforms.*object/i.test(m)) return "A new version is still being built for your computer. Wait a few minutes and try again.";
  if (/error sending request|dns|timed out|timeout|network|connect|could not fetch|failed to fetch/i.test(m)) return "Couldn't reach the update server. Check your internet or VPN, then try again.";
  if (/signature/i.test(m)) return "The update didn't pass its security check, so it wasn't installed. Tell the owner.";
  return `Something went wrong while checking. Try again in a few minutes; if it keeps happening, tell the owner.\n(${m.slice(0, 160)})`;
}

// ⌘/Ctrl + key -> page. The `<kbd>` hints in index.html must list exactly these keys (lib.test.js checks it).
export const NAV_KEYS = { 1: "search", 2: "transcripts", 3: "skills", 4: "map", 5: "onedrive", 6: "pipeline", ",": "settings" };

// Number on the Market Intel menu row: items waiting in queues plus stale outputs.
export function pipelineBadgeCount(status) {
  if (!status) return 0;
  return (status.queues ?? []).reduce((n, q) => n + (q.pending || 0), 0) + (status.outputs ?? []).filter((o) => o.stale).length;
}

// Market Intel tab search: every word must match. Competitive matches the name only; PMDs also match PM, PMM, modules and summary.
export function planSearch(outputs, query, tab, assignments = {}) {
  const words = (query ?? "").trim().toLowerCase().split(/\s+/).filter(Boolean);
  if (!words.length) return outputs;
  const hay = (o) => {
    if (tab !== "pmd") return o.name.toLowerCase();
    const a = assignments[o.name.replace(/^PMD:\s*/, "")];
    return [o.name, a?.pm, a?.pmm, a?.modules, a?.summary].flat().filter(Boolean).join(" ").toLowerCase();
  };
  return outputs.filter((o) => words.every((w) => hay(o).includes(w)));
}
