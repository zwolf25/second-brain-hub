# Second Brain Hub — Backlog

Single source of truth for tech debt and enhancements. Maintained by Claude: finish an item → move it to **Done** with the release version; find something new → add it with a score. See the `hub-release` skill (step 3b).

**Scoring:** Value 1-5 (user pain / risk removed) · Effort 1-5 (1 = under an hour, 5 = days) · Score = Value × (6 − Effort). Higher first. Ties: user-visible beats internal.

Created 2026-11-01 from a read of the code, ARCHITECTURE.md §12, CI config and the app-data folder. Effort/value are estimates, not measurements.

## Open (prioritized)

| # | ID | Type | Item | V | E | Score | Why / evidence |
|---|----|------|------|---|---|-------|----------------|
| 1 | TD-1 | Debt | **Gate releases on tests.** `build.yml` publishes on every push to `main`; `test.yml` is manual-only (`workflow_dispatch`). Make the release job `needs:` a test job (`cargo test` + `node --check src/main.js`), run it on PRs too, and add `paths-ignore` for docs-only pushes (a push of `BACKLOG.md` alone would currently trigger a build at the same version tag). | 5 | 2 | 20 | Four releases (0.8.2-0.8.5) went to every user with no test run in CI. 40 Rust tests exist but nothing enforces them. |
| 2 | F-1 | Feature | **Consistent, visible feedback for actions.** Move now has a dialog + badge; Hide/Unhide, Add to Second Brain, category change and bulk actions still use the 4 s status-bar note (`transcriptsNotice`). Replace with one toast helper that persists until dismissed or ~8 s, and shows failures. | 4 | 2 | 16 | Same "nothing happened" bug the user hit on Move. Pattern is shared (`transcriptsNotice`, line ~268). |
| 3 | F-2 | Feature | **Update visibility.** Settings shows current version + "Check for updates" button; check failures show a message (today only `console.log("[update] check failed")`); `latest.json` `notes` says "See the repo's commit history" so the banner can't say what changed — set the release body from the ARCHITECTURE paragraph. | 4 | 2 | 16 | User saw no banner on 0.8.2 and had no way to tell why. Hourly check shipped in 0.8.4 but failures are still silent. |
| 4 | TD-2 | Debt | **Delete orphaned model files.** `~/Library/Application Support/com.zackwolf.second-brain-search/models/` (chat + embedding) is 430 MB and no source file references `models`; the AI chat was removed in v3. One-time cleanup on launch (only that known folder). | 3 | 1 | 15 | Verified: `grep models src-tauri/src` finds nothing; folder is 430 MB on this machine. |
| 5 | TD-3 | Debt | **Set a Content-Security-Policy.** `tauri.conf.json` has `"csp": null`; `main.js` builds HTML with `innerHTML` in 18 places and relies on `escapeHtml` at each. A CSP (default-src 'self') is the backstop for a missed escape on transcript titles/file names. | 3 | 2 | 12 | Titles come from files other people can drop in shared folders. Needs a click-through test since inline handlers/styles can break. |
| 6 | F-3 | Feature | **Move dialog starts at the last destination**, and an "Undo" for the last manual move (moved_by + old path are not stored today; add `moved_from`). | 3 | 2 | 12 | Repeated moves to the same folder mean re-browsing OneDrive every time. |
| 7 | TD-4 | Debt | **Click-through smoke checklist for the UI.** ARCHITECTURE says the Phase 3 UI, stub buttons, rule modal and VoC banner were "not clicked through before release". Write a 10-step manual checklist and run it once per release (or add a Playwright/tauri-driver smoke test later). | 4 | 3 | 12 | Move bug survived to production because only Rust is tested. |
| 8 | F-4 | Feature | **Filter / sort transcripts by "Moved"** and a "Show in Finder/Explorer" row action for moved files. | 2 | 2 | 8 | Badge exists (0.8.5); no way to list only moved items or jump to them. |
| 9 | TD-5 | Debt | **Intel Mac build.** `macos-latest` is arm64, so the `.dmg` and updater only serve Apple Silicon. Add an `x86_64-apple-darwin` matrix entry if any teammate is on Intel. First ask who is. | 3 | 3 | 9 | ARCHITECTURE §9 caveat; unknown how many users affected. |
| 10 | TD-6 | Debt | **Verify on Windows**: updater cycle, file watcher with OneDrive, `claude://` deep link, in-app editing (§12 items 0, 1, 2, 5). Needs a Windows tester; builds already ship for it. | 4 | 4 | 8 | Listed as untested across §12; shipping unverified to Windows users. |
| 11 | TD-7 | Debt | **Split the big files.** `main.js` is 1,673 lines (one scope, all pages) and `transcripts.rs` is 1,642. Split by page/feature once a change forces it; do not refactor speculatively. | 2 | 4 | 4 | Maintainability only; no user pain today. |
| 12 | TD-8 | Debt | **Trim ARCHITECTURE.md** (70 KB; sections numbered 15, 14, 13, then 1-12; some §12 items now stale, e.g. updater "verified on Mac" while 3.x/4.x cycles have run). Move version history to a CHANGELOG and keep the doc current-state. | 2 | 3 | 6 | Slows every release (hub-release step 3 edits this file). |
| 13 | TD-9 | Debt | **Incremental indexing.** Index re-walks the whole vault on any change (§3, §12 item 4). Only worth doing when it measurably slows. | 2 | 4 | 4 | No measurement yet; add timing to the log first. |

## Not doing (decided)
- Apple notarization / paid developer account — internal tool, cost not justified (ARCHITECTURE §9).

## Done
| ID | Item | Release |
|----|------|---------|
| — | Confirmation dialog after Transcripts Move | 0.8.3 |
| — | Hourly app-update check (was launch-only) | 0.8.4 |
| — | "Moved to <folder>" badge on moved transcripts | 0.8.5 |
