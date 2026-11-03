# Second Brain Hub — Backlog

Single source of truth for tech debt and enhancements. Maintained by Claude: finish an item → move it to **Done** with the release version; find something new → add it with a score. See the `hub-release` skill (step 3b).

**Scoring:** Value 1-5 (user pain / risk removed) · Effort 1-5 (1 = under an hour, 5 = days) · Score = Value × (6 − Effort). Higher first. Ties: user-visible beats internal.

Created 2026-11-01 from a read of the code, ARCHITECTURE.md §12, CI config and the app-data folder. Effort/value are estimates, not measurements.

## Open (prioritized)

Rows marked **new** were proposed 2026-11-02 from a read of `ARCHITECTURE.md`, `lib.rs`, `render.rs`, `Cargo.toml`, the CI workflows and `main.js`. Scores are estimates; prune what you don't want.

| # | ID | Type | Item | V | E | Score | Why / evidence |
|---|----|------|------|---|---|-------|----------------|
| 1 | F-8 | Feature (new) | **Staleness marker and filter.** Badge wikis not updated in 90+ days and add a "Stale" filter, using the `updated` field already stored in the index. | 3 | 2 | 12 | Shows what `wiki-cleanup` would flag without running it; pairs with the overdue banner (F-6). |
| 2 | F-11 | Feature (new) | **Broken-wikilink and orphan report on the Map page.** `graph.rs` already resolves wikilinks and builds the graph; list unresolved targets and pages with no inbound links. | 3 | 2 | 12 | Gives collaborators the same signal `wiki-cleanup` dimension 8 produces, without a token run. |
| 3 | TD-14 | Debt (new) | **Make the frontend testable.** `main.js` is checked only by `node --check`; the Move bug (0.8.2) lived in that gap. Pull pure logic (`fuzzyScore`, `escapeHtml`, auto-rule matching, date formatting) into a module and add `node --test` to CI. Do it with TD-7, not before. | 3 | 3 | 9 | 1,917 lines, zero automated tests; `SMOKE-CHECKLIST.md` is manual. |
| 4 | F-12 | Feature (new) | **Result filters and sort:** filter by frontmatter `type` and `topics`, sort by updated date. Data is indexed; family filter (0.11.0) is the pattern. | 3 | 3 | 9 | Useful once the vault passes ~300 pages. |
| 5 | TD-6 | Debt | **Verify on Windows**: updater cycle, file watcher with OneDrive, `claude://` deep link, in-app editing (§12 items 0, 1, 2, 5). Needs a Windows tester; builds already ship for it. | 4 | 4 | 8 | Listed as untested across §12; shipping unverified to Windows users. Two teammates were asked on 2026-11-01. |
| 6 | F-13 | Feature (new) | **Vault health page (owner-facing):** read `_status/pipelines.json` and the queue folders to show pipeline freshness and queue backlogs. Zero tokens. | 2 | 2 | 8 | Same data `sb_inventory.py` prints; the dashboard doctor covers Zac's machine, not the shared view. |
| 7 | TD-7 | Debt | **Split the big files.** `main.js` is 1,917 lines (one scope, all pages), `transcripts.rs` 1,773, and `lib.rs` 1,099 with ~60 commands in one file. Split by page/feature once a change forces it; do not refactor speculatively. | 2 | 4 | 4 | Maintainability only; no user pain today. Counts re-measured 2026-11-02 (were 1,673 / 1,642). |

## Not doing (decided)
- Incremental indexing (TD-9): a full rebuild of the real vault takes 594 ms (2026-11-01). Revisit if the `indexed N docs ... in M ms` log line shows seconds.
- Apple notarization / paid developer account — internal tool, cost not justified (ARCHITECTURE §9).

## Done
| ID | Item | Release |
|----|------|---------|
| — | Confirmation dialog after Transcripts Move | 0.8.3 |
| — | Hourly app-update check (was launch-only) | 0.8.4 |
| — | "Moved to <folder>" badge on moved transcripts | 0.8.5 |
| TD-1 | Release gated on tests (`build.yml` calls `test.yml`; docs-only pushes ignored) | 0.9.0 |
| F-1 | One toast for all Transcripts actions (Move, Hide/Unhide, category, add) | 0.9.0 |
| F-2 | Settings "Check for updates", visible failures, release notes in the update banner (`RELEASE_NOTES.md`) | 0.9.0 |
| TD-2 | Deleted orphaned 430 MB `models/` folder on launch | 0.9.0 |
| TD-3 | Content-Security-Policy set (smoke-tested: Settings page and IPC work; viewer/images not click-tested) | 0.9.0 |
| F-5 | Hub → child wikis on the Neural Map (thick edges, ringed hubs, closer clusters) in the Hub and the ZacAI Dashboard | 0.9.1 |
| F-3 | Move picker starts at the last folder; Undo on the Moved toast | 0.10.0 |
| F-4 | "Only moved" filter and "Show in folder" row action | 0.10.0 |
| TD-4 | `SMOKE-CHECKLIST.md` (10 manual UI checks; hub-release step 6a). Not automated. | 0.10.0 |
| — | Re-link moved transcripts shuffled outside the Hub | 0.10.1 |
| — | "Recently updated" 5 wikis on the empty search page | 0.10.1 |
| — | Skills banner Refresh button | 0.10.1 |
| — | Better first-run folder auto-detect (OneDrive roots, vault override, KFM, inbox by email) | 0.10.1 |
| TD-5 | Intel Mac build added to `build.yml` (matrix `--target x86_64-apple-darwin`); unverified until the next release builds | 0.11.0 |
| TD-8 | ARCHITECTURE.md trimmed 76 KB to 58 KB: sections in order, release-by-release detail and the v2 AI Search archive moved to `CHANGELOG.md`, stale risks fixed; hub-release step 3 now writes CHANGELOG.md | 0.11.0 |
| TD-9 | Index timing logged on every reindex; measured 594 ms for 218 real docs, so incremental indexing is not built (see Not doing) | 0.11.0 |
| — | Clickable wikilinks + child wikis, backlinks, wiki-family search filter, "new since last visit" markers | 0.11.0 |
| F-6 | Wiki-cleanup overdue banner (4+ weeks via `logs/wiki-cleanup-last-run.txt`, one-click `/wiki-cleanup`) | 0.11.1 |
| F-7 | ☰ page menu + shared icon header; "showing N of M" Recently updated with Show all; readable dates | 0.12.0 |
| TD-10 | Poisoned-Mutex recovery: `LockRecover::lock_recover()` replaces the 70 `.lock().unwrap()` calls in `lib.rs`; test poisons a mutex and checks the next lock works. The other non-test `unwrap`s in `transcripts.rs` are guarded by invariants (checked), so left alone. | 0.13.0 |
| TD-11 | Raw HTML in markdown files shows as text in the viewer (only the app's own wikilink anchors render; comments dropped); also fixed an unescaped plain-text fallback search snippet in `search.rs` that went into `innerHTML`. Audited all `innerHTML` template sites in `main.js`: the rest escape or interpolate constants. 3 new tests. | 0.13.0 |
| F-10 | "✦ Ask Claude" button in the viewer header (hidden while editing): opens `claude://code/new?q=Read <path> and answer my question about it: `, pre-filled, not sent, no `folder=`. First real click on Mac still to verify (possible permission prompt). | 0.13.0 |
| F-9 | Keyboard navigation in results (↑/↓/Enter/Esc, Enter only after a row is highlighted). | 0.14.0 |
| TD-12 | CI: clippy `-D warnings` in `test.yml` + Dependabot (cargo, Actions). `cargo fmt --check` skipped (code never rustfmt-formatted: 266 diffs), `cargo audit` replaced by Dependabot. | 0.14.0 |
| TD-13 | ARCHITECTURE §2 lists all 16 modules; §12 item 0 aligned with §9a. | 0.14.0 |
| F-15 | "New" badge on Skills rows changed since the previous launch (file mtime via `SkillInfo.modified_ms`). | 0.14.0 |
