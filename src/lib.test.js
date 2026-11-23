import test from "node:test";
import assert from "node:assert/strict";
import { fuzzyScore, escapeHtml, formatUpdated, isStale, topicValues, filterByTopic, sortResults, onedriveSummary, onedriveDetails, pipelineTabs, vocPriority, vocVisible } from "./lib.js";

test("fuzzyScore: subsequence match, tighter span scores lower, miss is null", () => {
  assert.equal(fuzzyScore("wo", "Work Orders"), 1);
  assert.ok(fuzzyScore("wo", "Work Orders") < fuzzyScore("wo", "w and o far apart"));
  assert.equal(fuzzyScore("zz", "Work Orders"), null);
  assert.equal(fuzzyScore("", "anything"), 0);
});

test("escapeHtml escapes & < > and tolerates null", () => {
  assert.equal(escapeHtml(`<a href="x">&</a>`), `&lt;a href="x"&gt;&amp;&lt;/a&gt;`);
  assert.equal(escapeHtml(null), "");
});

test("formatUpdated: today, yesterday, empty, unparseable", () => {
  const now = new Date(2026, 10, 11, 15, 0); // local Nov 11 2026
  assert.equal(formatUpdated("2026-11-11", now), "Today");
  assert.equal(formatUpdated("2026-11-10", now), "Yesterday");
  assert.equal(formatUpdated("", now), "");
  assert.equal(formatUpdated("not a date", now), "not a date");
});

test("isStale: 90 day boundary, date-only and full ISO, missing", () => {
  const now = new Date(2026, 10, 11).getTime();
  assert.equal(isStale("2026-11-01", now), false);
  assert.equal(isStale("2026-01-01", now), true);
  assert.equal(isStale("2026-01-01T00:00:00Z", now), true);
  assert.equal(isStale("", now), false);
});

const docs = [
  { title: "A", topics: ["pricing", "ai"], updated: "2026-10-01" },
  { title: "B", topics: ["pricing"], updated: "2026-11-01T10:00:00Z" },
  { title: "C", topics: [], updated: "" },
  { title: "D", topics: ["ai"], updated: "2026-09-01" },
];

test("topicValues: distinct and sorted", () => {
  assert.deepEqual(topicValues(docs), ["ai", "pricing"]);
});

test("filterByTopic: match, no topic = unchanged", () => {
  assert.deepEqual(filterByTopic(docs, "pricing").map((r) => r.title), ["A", "B"]);
  assert.equal(filterByTopic(docs, ""), docs);
});

test("sortResults: newest/oldest put undated last; relevance leaves order and input untouched", () => {
  assert.deepEqual(sortResults(docs, "newest").map((r) => r.title), ["B", "A", "D", "C"]);
  assert.deepEqual(sortResults(docs, "oldest").map((r) => r.title), ["D", "A", "B", "C"]);
  assert.equal(sortResults(docs, "relevance"), docs);
  assert.deepEqual(docs.map((r) => r.title), ["A", "B", "C", "D"]);
});

const odIssue = (severity, title) => ({ code: "x", severity, title, detail: "", steps: [], can_open: true });
const odStatus = (worst, issues, extra = {}) => ({ worst, issues, checks: [{ label: "App", state: "g" }], limited: false, checked_at: 0, platform: "macos", ...extra });

test("onedriveSummary: ok is quiet, red gets pill + banner, yellow only a pill", () => {
  assert.equal(onedriveSummary(odStatus("ok", [])).pill, "");
  assert.equal(onedriveSummary(null).setting, "Checking…");
  const red = onedriveSummary(odStatus("red", [odIssue("yellow", "Items"), odIssue("red", "OneDrive is signed out")]));
  assert.equal(red.pill, "OneDrive: problem");
  assert.match(red.banner, /signed out/);
  const yellow = onedriveSummary(odStatus("yellow", [odIssue("yellow", "Items")]));
  assert.equal(yellow.pill, "OneDrive: check");
  assert.equal(yellow.banner, "");
  assert.match(onedriveSummary(odStatus("ok", [], { limited: true })).setting, /Windows/);
});

test("onedriveDetails: lists checks and issues for pasting", () => {
  const t = onedriveDetails(odStatus("red", [odIssue("red", "Down")]), "0.18.0");
  assert.match(t, /0\.18\.0/);
  assert.match(t, /OK: App/);
  assert.match(t, /\[red\] x: Down/);
});

test("pipelineTabs: three tabs, stale first, most new evidence first", () => {
  const o = [
    { name: "PMD: A", stale: false, new_upstream: 0 }, { name: "PMD: B", stale: true, new_upstream: 2 }, { name: "PMD: C", stale: true, new_upstream: 5 },
    { name: "Battlecard: X", stale: true, new_upstream: 1 }, { name: "Landscape map", stale: false, new_upstream: 0 }, { name: "Win/Loss/Churn report", stale: true, new_upstream: 9 },
  ];
  const t = pipelineTabs(o);
  assert.deepEqual(t.pmd.map((x) => x.name), ["PMD: C", "PMD: B", "PMD: A"]);
  assert.equal(t.comp.length, 2);
  assert.equal(t.wl[0].name, "Win/Loss/Churn report");
  assert.deepEqual(pipelineTabs(undefined), { wl: [], pmd: [], comp: [] });
});

test("vocPriority and vocVisible", () => {
  assert.equal(vocPriority("$380,000").label, "High");
  assert.equal(vocPriority("$95K").label, "Low"); // text without digits-only amount stays Low: sync writes full dollars
  assert.equal(vocPriority("$50,000").label, "Med");
  const rows = [{ kind: "loss", declined: "" }, { kind: "win", declined: "" }, { kind: "loss", declined: "too small" }];
  assert.equal(vocVisible(rows, "all").length, 2);
  assert.equal(vocVisible(rows, "loss").length, 1);
  assert.equal(vocVisible(rows, "declined").length, 1);
});
