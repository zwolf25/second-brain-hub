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
