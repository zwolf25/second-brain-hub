import test from "node:test";
import assert from "node:assert/strict";
import { fuzzyScore, escapeHtml, formatUpdated, isStale, topicValues, filterByTopic, sortResults } from "./lib.js";

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
