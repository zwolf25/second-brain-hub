# Manual smoke checklist

Run on the installed build (or `npx tauri build --debug --no-bundle --config '{"identifier":"com.zackwolf.sbh-test"}'` for a throwaway profile) before each release that touches the UI. Rust is unit-tested; nothing below is, and the Move bug (0.8.2) lived in this gap. Tick only what you actually looked at; note anything skipped in the release commit.

1. **Launch:** window opens, no blank view; Settings shows version and folder paths ("Found: …" means IPC works).
2. **Search:** type a keyword from a known wiki; results appear; double-click opens the viewer.
3. **Viewer:** markdown renders (headings, table, image); Back returns to the same scroll position; Edit then Cancel leaves the file unchanged.
4. **Neural Map:** nodes drawn, not stacked; thick lines and rings on hub wikis; click a node dims the rest; drag pans.
5. **Transcripts list:** rows load, search narrows them, page 2 works, "Show hidden" and "Only moved" counts match.
6. **Move:** move one row to a folder; toast says where; "Moved to" badge appears; click **Undo** and the badge goes away and the file is back.
7. **Hide / Unhide / category:** each shows a toast; "Show hidden" reveals the hidden row.
8. **Add to Second Brain:** with a raw folder set, a stub appears in `raw/` and the toast says so.
9. **Row menu:** "Show in folder" opens Finder/Explorer on the transcript; Open shows the viewer.
10. **Update:** Settings → "Check for updates" says "latest version" (or offers one); the Search-tab banner shows the release notes when an update exists.
11. **Recently updated:** Search tab with an empty box lists 5 wikis under "Recently updated"; typing replaces them with results, clearing brings them back.
12. **Skills Refresh:** with the update banner showing, click Refresh: toast says still N behind or up to date; banner clears if up to date.
