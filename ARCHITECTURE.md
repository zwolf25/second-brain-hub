# Architecture — Second Brain Hub

Local desktop app that indexes the shared SC PM "Second Brain" wiki vault (OneDrive) and gives fast fuzzy full-text search over it, plus local semantic "chat" search. Runs entirely on the collaborator's machine — no server, no cloud calls, no API tokens. Written for a small internal team, not a public product; scope decisions below favor simplicity over generality.

(Product name is "Second Brain Hub"; the underlying project folder, npm package, and Rust crate/binary are still named `second-brain-search` — internal plumbing, not user-visible, left as-is rather than doing a full rename.)

Status: **v1 (search) and v2 (local chat) are both implemented and verified against the real shared vault**, including live GUI testing that surfaced and fixed several real bugs (§10). Windows packaging is still blocked on machine/CI access (§8).

## 1. Stack

- **Tauri 2** (Rust backend + OS-native webview). Chosen over Electron for installer size (~10-20MB vs ~150-200MB) and idle resource footprint — this machine had no Rust toolchain when the project started, installed via `rustup`.
- **tantivy** — embedded Lucene/BM25-style full-text engine, runs in-process, no server. Gives genuine fuzzy (edit-distance) search, which tantivy's default `QueryParser` does not do on its own — the query layer builds `FuzzyTermQuery` clauses manually (see §4).
- **candle** (Hugging Face's pure-Rust ML framework) for local chat — an embedding model (semantic search) plus an optional tiny instruct model (prose synthesis), both CPU-only, both downloaded on first use rather than bundled. See §7.
- **Plain HTML/CSS/JS frontend**, no framework. The UI is a search box, a results list, a chat tab, and a settings/onboarding screen — a framework buys nothing at this scope.
- `tracing` + `tracing-appender` for structured local logging; `notify` + `notify-debouncer-mini` for vault file-watching; `walkdir` for directory traversal; `serde_yaml` for frontmatter parsing; `anyhow` for error plumbing; `reqwest` (native-tls, blocking) for model downloads.

## 2. Module layout (`src-tauri/src/`)

| File | Responsibility |
|---|---|
| `config.rs` | `AppConfig` (currently just `vault_path`), load/save as plain JSON in `app_config_dir()`, vault validation, OneDrive path auto-detection |
| `frontmatter.rs` | Splits `---`-delimited YAML frontmatter from markdown body (hand-rolled, not a crate — simple enough and avoids depending on an unfamiliar crate's exact API), extracts title (first `# ` heading, fallback filename), strips `[[wikilink]]` brackets for clean display |
| `index.rs` | tantivy schema definition, full vault walk + rebuild, index open. Includes a fuzzy-search round-trip test |
| `search.rs` | Fuzzy query construction (title/topics/body, title and topics boosted), snippet generation, `SearchResult` shape returned to the frontend |
| `watcher.rs` | Debounced (400ms) filesystem watch on the vault root, triggers a full reindex on any change |
| `logging.rs` | Rolling daily log file, 7-day retention pruning, panic hook that logs crashes to the same file |
| `llm.rs` | Model download (with timeout + progress callback), embedding model wrapper, chunking, chunk-embedding build, cosine retrieval, chat-model generation. Includes an `#[ignore]`d real-vault integration test (§7) |
| `lib.rs` | Tauri app wiring: `AppState`, command registration, startup sequence |

## 3. Indexing pipeline

On startup (if a vault is configured) and on every debounced file-watcher event, `index::rebuild()` does a **full re-walk-and-reindex** — deletes the on-disk tantivy index directory and rebuilds it from scratch by walking every `.md` file in the vault root. This is simpler than incremental per-file diffing and, at real-world scale (73 files, ~1.8MB), completes in well under a second — confirmed by the log timestamps during verification (sub-300ms `Preparing commit` → `Garbage collect` cycles). Revisit only if a much larger vault makes this measurably slow.

**tantivy schema:** `path` (STRING, STORED — canonical doc id, also the click-through target), `title` (TEXT, STORED), `body` (TEXT, STORED, wikilink brackets stripped), `topics` (TEXT, STORED, from frontmatter, wikilink brackets stripped), `updated` (STRING, STORED, from frontmatter).

**Frontmatter handling:** every wiki file's frontmatter has `type`, `topics` (a list), `created`, `updated` — no `title` field, confirmed by reading real files, hence the first-`# `-heading fallback. OneDrive transient files (filenames starting `~$`) are skipped; unreadable files are skipped rather than aborting the whole rebuild.

**Scope note:** this app indexes the **shared vault only**. Zac's separate personal ZacAI vault and its `second-brain/` pointer-stub subfolder are explicitly out of scope — dropped early to keep indexing/onboarding simple (no stub-dedup logic, no vault-source filter UI). If chat is enabled, `llm::build_chunk_embeddings` re-walks the same vault independently (see §7) — two walks of the same small tree, not shared code, judged not worth a shared-walker abstraction at this scale.

## 4. Search (Search tab)

Query flow: debounced (~200ms) keystroke in the frontend → `search_vault` Tauri command → `search::search()`:
1. Tokenize the query on whitespace, lowercase each token.
2. For each token, build `FuzzyTermQuery` clauses (edit distance 1 for short tokens ≤5 chars, 2 for longer ones) against `title` (boosted 2.0×), `topics` (boosted 1.5×), and `body` (unboosted), OR'd together in a `BooleanQuery`.
3. Run `TopDocs::with_limit(30)`.
4. Generate highlighted snippets via tantivy's `SnippetGenerator` on the `body` field; falls back to a plain 220-char excerpt if snippet generation yields nothing.

This query-construction logic is the one place in the v1 codebase judged worth an explicit test (`index.rs`'s `rebuild_and_fuzzy_search_round_trip`), which also asserts typo tolerance (`"refrigirant"` still finds `"Refrigerant Tracking"`).

**Opening a result:** double-click (not single-click — deliberate, so scrolling/scanning results doesn't accidentally open files; a "Double-click a result to open the file" hint is shown under the search box). Calls `tauri-plugin-opener`'s `openPath` directly from the frontend (`window.__TAURI__.opener` global). See §10 for a real permission bug this hit and how it was fixed.

## 5. Onboarding & config

First run: no `config.json` in `app_config_dir()` (or no `vault_path` in it) → onboarding screen. `autodetect_vault` command probes for a OneDrive folder under `~/Library/CloudStorage` (macOS) or the user's home directory (Windows-style layout) containing `SVC-Product_Management - Second Brain/wikis` with at least one `.md` file. If found, the user gets a one-click "Use detected folder" button; otherwise a native folder-picker (`tauri-plugin-dialog`) is the fallback — every collaborator's OneDrive path differs by username/OS, so this can never be hardcoded.

Config persists as plain `serde_json`-written JSON — no config-management plugin, three fields doesn't warrant one.

## 6. Logging & crash capture

`tracing` writes to a daily-rotating file under `app_log_dir()/logs/second-brain-search.log.<date>` (confirmed real location on macOS: `~/Library/Logs/com.zackwolf.second-brain-search/logs/`). Files older than 7 days are pruned on each startup. A `std::panic::set_hook` logs any panic (message + location) to the same file before the process dies, so crashes aren't silently lost.

There is no automatic upload/telemetry — deliberately out of scope. The "Logs" button in the search view's topbar calls `revealItemInDir` to open the log folder in Finder/Explorer; a collaborator experiencing an issue attaches the relevant log file manually (Slack/email) for Zac to read. This is a manual, user-initiated export, not a phone-home pipeline.

**Gap, not yet fixed:** `llm.rs`'s download/chunking/generation path logs via `eprintln!` in its own test only — the production `enable_chat`/`write_answer` commands don't call `tracing::info!` anywhere, so the log file currently shows nothing useful if chat-side behavior needs debugging in the field. Worth adding basic `tracing` calls in `lib.rs`'s chat commands (mirroring how `reindex()` already does it) next time this area is touched.

## 7. v2 — local chat (Chat tab, implemented)

**Why not Ollama:** it's a separate app collaborators would have to install and manage themselves — real friction for a non-technical PM team, and explicitly ruled out.

**Why not a persistent generative model:** flagged as a real performance concern (RAM/CPU on every collaborator's laptop). Resolved by defaulting to **extractive retrieval**, not standing generation:
- Default (`related_docs` command): the embedding model (`all-MiniLM-L6-v2`, ~90MB, via `candle-transformers`' BERT support) embeds the query, cosine-similarity ranks against pre-computed chunk embeddings, and results render as a ranked, highlighted list — no prose generation, near-instant.
- Opt-in ("Write me an answer" button → `write_answer` command): loads a tiny instruct model on demand (`Qwen2-0.5B-Instruct`, Q4_0 GGUF, ~350MB) for that one query only, generates a short answer citing the retrieved chunks, then **drops the model** (Rust scope-exit) — no standing background cost. Uses greedy sampling + a repeat penalty (1.15, last-64-tokens window) — omitting the repeat penalty was an early bug that produced looping/duplicated output (§10).

**Why `candle`** over binding to llama.cpp: pure Rust, no C/C++ toolchain needed at build time — relevant because the Windows build (§8) already has its own toolchain friction.

**Chunking:** `llm::chunk_body` splits on `## ` heading boundaries, hard-splitting anything over `CHUNK_MAX_CHARS` (2500, ~500-800 tokens). Chunk embeddings are built by `llm::build_chunk_embeddings`, which re-walks the vault (separately from `index.rs`, see §3) and embeds every chunk in one batched call. This runs once when chat is first enabled (`enable_chat` command) and again on every reindex **only if chat is already enabled** (checked in `lib.rs`'s `reindex()`) — collaborators who never open the Chat tab never pay this cost.

**Retrieval:** brute-force cosine similarity (`llm::top_related`) over the in-memory chunk-embedding list — no vector DB, confirmed sufficient at this corpus size (1132 chunks from the real 73-file vault).

**Models are not bundled** in the installer (keeps it at ~10-20MB); the embedding model downloads on first Chat-tab "Enable AI Chat" click, the synthesis model only on first "Write me an answer" click. Both cache in `app_data_dir/models/{embedding,chat}/` after first download, fully offline afterward.

### Validated: TLS fix for corporate-intercepted networks

`hf-hub`'s built-in downloader (via `native-tls`, through its bundled `ureq` client) fails cert validation against Zscaler TLS interception, even though plain `curl` and the OS trust store handle the same Zscaler root CA fine. Root cause isolated to `ureq`'s specific `native-tls` binding — a standalone test confirmed `reqwest`'s `native-tls` binding (same underlying OS trust store, different crate) works correctly against the same network. **Fix shipped:** `llm.rs` downloads directly via `reqwest::blocking` (not `hf-hub` at all) hitting `https://huggingface.co/{repo}/resolve/main/{filename}` URLs directly. Confirmed working end-to-end against the real network in `llm.rs`'s ignored integration test (downloaded ~443MB for the 0.5B tier via this exact production code path).

### Validated, with a real caveat: retrieval quality on technical/identifier-dense content

The real-vault test (`cargo test -- --ignored`) surfaced a genuine, evidenced limitation, not a code bug:

For a **topical** query ("refrigerant tracking compliance"), retrieval works well — the top-5 results are all genuinely refrigerant-tracking-related content, and a generated answer is coherent.

For a **specific enumeration** query ("what Feature Flags are related to Refrigerant Tracking?"), retrieval struggles: the doc section that actually lists the real flag names (`sc-provider-mobile-and-refrigerant.md`'s "Feature-Flag Reference" section — `RefrigerantTrackingNoYes`, `Show_RT_TC_AssetList`, etc.) did not appear in the top 10 of 1132 chunks at all. A broader, prose-heavy roadmap chunk that merely *mentions* "Refrigerant Tracking" extensively but never discusses feature flags outranked it. Root cause: `all-MiniLM-L6-v2` (a small, general-purpose sentence embedding model) embeds dense bullet-list/code-identifier content weakly relative to conversational prose — a known class of limitation for small embedding models, not something the repeat-penalty or model-size fixes below touch, since the problem is upstream of generation (bad retrieval → the LLM never sees the right content, and correctly declines to invent it once the anti-hallucination prompt/repeat-penalty fixes were in place).

Tested and ruled out as fixes: bumping the chat model from 0.5B to 1.5B-Instruct (no improvement — same retrieval-starved context, just a slower wrong-ish answer); tightening the prompt to demand verbatim quoting (made the model correctly refuse to fabricate, but the answer was then useless since the real content wasn't in its context).

**Recommended fix, not yet built:** hybrid retrieval — blend/rerank the embedding-based top-k with a full-text score for the same query against the **existing tantivy index** (v1's `search.rs` already has this working; it isn't used by chat retrieval today). Exact-term matches like "feature flag" would surface the right chunk even when its embedding similarity is weak. This is the concrete next step if chat answer quality on this kind of query matters more than what's shipped now.

**Given this, chat should be framed to users as "best effort, verify against the cited source docs"** (the UI already shows clickable doc titles under both extractive results and generated answers) rather than as reliably authoritative — it's genuinely useful for topical exploration, weaker on "find me the exact identifier" style questions.

### Model-size ladder tested

| Model | Load time | Notes |
|---|---|---|
| Qwen2-0.5B-Instruct Q4_0 (default, shipped) | ~800ms | Fast; on a clean single-doc context, produced a correct, cited answer with no hallucination |
| Qwen2-1.5B-Instruct Q4_0 (tested, not shipped) | ~3-4x slower to download/load | No quality improvement on the retrieval-starved query above — not worth the extra weight as a default |

**Caveat — only validated on this machine.** This is a capable Mac; a representative slower Windows laptop (older Intel CPU, no Apple-Silicon-class SIMD) hasn't been tested and could be meaningfully slower. Same category of gap as the Windows build blocker in §8.

**Dev-mode note:** `npm run tauri dev` builds in **debug** profile, and candle's tensor math is dramatically slower unoptimized (a chunk-embedding build that takes seconds in release can peg the CPU for minutes in debug — this looked exactly like a hang during interactive testing and wasn't). Always test chat-related changes against a `cargo build --release` binary, not `tauri dev`.

## 8. Packaging & distribution

**Mac:** `cargo tauri build` → unsigned `.app`/`.dmg`. Xcode Command Line Tools are sufficient for this; full notarization/Apple Developer Program enrollment is explicitly skipped as unnecessary cost for a handful-of-teammates internal tool. First launch requires a Gatekeeper right-click → Open (or `xattr -cr` the `.app`).

**Windows — real, unresolved blocker:** cannot be cross-compiled from macOS. Tauri's Windows target needs the MSVC linker, WebView2, and NSIS/WiX bundling — none of which cross-compile from macOS. Needs one of: (1) an actual Windows machine to run `cargo tauri build` locally (cheapest), (2) a GitHub Actions `windows-latest` runner if this repo goes to GitHub, (3) a Windows VM as a fallback. **Blocked on access to one of these** — currently only ships for Mac.

**Explicitly out of scope:** auto-updater, telemetry, multi-user sync/accounts/backend, a real vector DB (brute-force cosine is enough at this corpus size), a wikilink cross-reference graph UI (brackets are just stripped for display).

## 9. Capabilities / permissions (`src-tauri/capabilities/default.json`)

Tauri's ACL is deny-by-default per-command **and** per-scope — granting a command permission (e.g. `opener:allow-open-path`) is not sufficient if that permission also requires a path scope and none is configured (see §10 for the bug this caused). Current grants: `core:default`, `opener:default` (URL opening + reveal-in-Finder), `opener:allow-open-path` scoped to `$HOME/**` (covers the OneDrive vault at any username; broad but low-risk since it only permits "open with OS default handler," not raw file read/write), `dialog:default` (folder picker).

## 10. Real bugs found and fixed during live testing

Live GUI testing (not just automated tests) surfaced several real issues automated tests didn't catch — worth recording since they're the kind of thing that'll recur if this pattern (Tauri + candle + reqwest) gets reused elsewhere:

1. **Download hang risk:** `reqwest::blocking::get` (the free function) has no timeout. A real network failure would hang the download — and the UI waiting on it — forever with no error. **Fixed:** build an explicit `reqwest::blocking::Client` with a 15s connect timeout and 20-minute overall timeout.
2. **UI looked frozen during the compute-only phase:** once a model finishes downloading, `enable_chat` still has real work left (loading the model, embedding ~1132 chunks) with no download-progress bytes to report — the progress bar/label just sat stale, indistinguishable from actually being stuck. **Fixed:** added a `chat-status` event emitted before each compute phase, decoupled from the byte-progress `model-download-progress` event.
3. **Silent permission failure on file-open:** `opener:allow-open-path` was granted but with no `scope` — Tauri's ACL denies-by-default even with the permission listed, so every open attempt failed with "Not allowed to open path X," silently, because the JS click handler had no `.catch()`. **Fixed both ends:** added `"allow": [{ "path": "$HOME/**" }]` to the capability, and added error surfacing (`alert(...)` on rejection) so any future permission/IO failure is visible instead of silent.
4. **Debug-build slowness looked like a hang:** see §7's dev-mode note — a legitimate ~minutes-long debug-profile compute phase was initially mistaken for a stuck process. Diagnosed via `ps`/`lsof` (99% CPU, zero open network connections — ruled out a network hang, pointed at unoptimized compute instead) before concluding it needed `--release`, not a code fix.

## 11. Known risks / open items

1. File-watcher robustness against OneDrive sync behavior (partial writes, transient lock files) is defensive-by-design (skip-on-error, filter `~$*`) but was only exercised by real ambient vault activity during verification, not a deliberate edit-while-running stress test — worth a dedicated smoke test, especially on Windows once that build exists.
2. Windows build is blocked on machine/CI access (§8) — not a code problem.
3. Chat retrieval quality on technical/identifier-dense queries is a documented, evidenced gap (§7) — recommended fix is hybrid tantivy+embedding retrieval, not yet built.
4. Chat-side commands have no `tracing` logging (§6) — hard to debug in the field beyond what a user reports directly.
5. Full-rewalk-on-any-change indexing (§3) assumes the vault stays small; revisit if it ever measurably slows down.
6. `spike-candle/` at the project root is throwaway validation code from before the real `llm.rs` implementation landed — safe to delete, or keep as a quick benchmark harness for future model-swap decisions.
7. Speed/quality numbers throughout §7 are only validated on this Mac — a representative Windows/lower-spec machine may behave meaningfully differently, same open question as the Windows build blocker.
