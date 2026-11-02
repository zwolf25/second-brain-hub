# Second Brain Hub — Backlog

Single source of truth for tech debt and enhancements. Maintained by Claude: finish an item → move it to **Done** with the release version; find something new → add it with a score. See the `hub-release` skill (step 3b).

**Scoring:** Value 1-5 (user pain / risk removed) · Effort 1-5 (1 = under an hour, 5 = days) · Score = Value × (6 − Effort). Higher first. Ties: user-visible beats internal.

Created 2026-11-01 from a read of the code, ARCHITECTURE.md §12, CI config and the app-data folder. Effort/value are estimates, not measurements.

## Open (prioritized)

| # | ID | Type | Item | V | E | Score | Why / evidence |
|---|----|------|------|---|---|-------|----------------|
| 1 | TD-6 | Debt | **Verify on Windows**: updater cycle, file watcher with OneDrive, `claude://` deep link, in-app editing (§12 items 0, 1, 2, 5). Needs a Windows tester; builds already ship for it. | 4 | 4 | 8 | Listed as untested across §12; shipping unverified to Windows users. |
| 2 | TD-7 | Debt | **Split the big files.** `main.js` is 1,673 lines (one scope, all pages) and `transcripts.rs` is 1,642. Split by page/feature once a change forces it; do not refactor speculatively. | 2 | 4 | 4 | Maintainability only; no user pain today. |

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
