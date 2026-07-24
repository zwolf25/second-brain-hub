# Architecture — Second Brain Hub

Local desktop app that indexes the shared SC PM "Second Brain" wiki vault (OneDrive) and gives fast fuzzy full-text search over it, plus local semantic "chat" search. Runs entirely on the collaborator's machine — no server, no cloud calls, no API tokens. Written for a small internal team, not a public product; scope decisions below favor simplicity over generality.

(Product name is "Second Brain Hub"; the underlying project folder, npm package, and Rust crate/binary are still named `second-brain-search` — internal plumbing, not user-visible, left as-is rather than doing a full rename.)

Status: **v1 (search), v2 (local chat), and the Raw/Inbox heads-up badges are all implemented and verified against the real shared vault**, including live GUI testing that surfaced and fixed several real bugs (§11). Windows builds now ship via GitHub Actions CI (§9) — the earlier "no Windows machine" blocker is resolved.

## 1. Stack

- **Tauri 2** (Rust backend + OS-native webview). Chosen over Electron for installer size (~10-20MB vs ~150-200MB) and idle resource footprint — this machine had no Rust toolchain when the project started, installed via `rustup`.
- **tantivy** — embedded Lucene/BM25-style full-text engine, runs in-process, no server. Gives genuine fuzzy (edit-distance) search, which tantivy's default `QueryParser` does not do on its own — the query layer builds `FuzzyTermQuery` clauses manually (see §4).
- **candle** (Hugging Face's pure-Rust ML framework) for local chat — an embedding model (semantic search) plus an optional tiny instruct model (prose synthesis), both CPU-only, both downloaded on first use rather than bundled. See §8.
- **Plain HTML/CSS/JS frontend**, no framework. The UI is a search box, a results list, a chat tab, topbar heads-up badges, and a settings/onboarding screen — a framework buys nothing at this scope.
- `tracing` + `tracing-appender` for structured local logging; `notify` + `notify-debouncer-mini` for vault/raw/inbox file-watching; `walkdir` for directory traversal; `serde_yaml` for frontmatter parsing; `anyhow` for error plumbing; `reqwest` (native-tls, blocking) for model downloads.

## 2. Module layout (`src-tauri/src/`)

| File | Responsibility |
|---|---|
| `config.rs` | `AppConfig` (`vault_path`, `raw_path`, `inbox_path`), load/save as plain JSON in `app_config_dir()`, path validation, OneDrive path auto-detection (shared `autodetect_second_brain_root` helper, reused for all three paths) |
| `frontmatter.rs` | Splits `---`-delimited YAML frontmatter from markdown body (hand-rolled, not a crate — simple enough and avoids depending on an unfamiliar crate's exact API), extracts title (first `# ` heading, fallback filename), strips `[[wikilink]]` brackets for clean display |
| `index.rs` | tantivy schema definition, full vault walk + rebuild, index open. Includes a fuzzy-search round-trip test |
| `search.rs` | Fuzzy query construction (title/topics/body, title and topics boosted), snippet generation, `SearchResult` shape returned to the frontend |
| `counts.rs` | `count_unprocessed_raw` (missing `<!-- wiki-processed -->` marker) and `count_inbox_items` (`*.share.md` count) for the topbar heads-up badges. Includes a fixture-based unit test |
| `watcher.rs` | Debounced (400ms) filesystem watch, triggers a full reindex (vault) or recount (raw/inbox) on any change — generic over which folder it's watching |
| `logging.rs` | Rolling daily log file, 7-day retention pruning, panic hook that logs crashes to the same file |
| `llm.rs` | Model download (with timeout + progress callback), embedding model wrapper, chunking, chunk-embedding build, cosine retrieval, chat-model generation. Includes an `#[ignore]`d real-vault integration test (§8) |
| `lib.rs` | Tauri app wiring: `AppState`, command registration, startup sequence |

## 3. Indexing pipeline

On startup (if a vault is configured) and on every debounced file-watcher event, `index::rebuild()` does a **full re-walk-and-reindex** — deletes the on-disk tantivy index directory and rebuilds it from scratch by walking every `.md` file in the vault root. This is simpler than incremental per-file diffing and, at real-world scale (73 files, ~1.8MB), completes in well under a second — confirmed by the log timestamps during verification (sub-300ms `Preparing commit` → `Garbage collect` cycles). Revisit only if a much larger vault makes this measurably slow.

**tantivy schema:** `path` (STRING, STORED — canonical doc id, also the click-through target), `title` (TEXT, STORED), `body` (TEXT, STORED, wikilink brackets stripped), `topics` (TEXT, STORED, from frontmatter, wikilink brackets stripped), `updated` (STRING, STORED, from frontmatter).

**Frontmatter handling:** every wiki file's frontmatter has `type`, `topics` (a list), `created`, `updated` — no `title` field, confirmed by reading real files, hence the first-`# `-heading fallback. OneDrive transient files (filenames starting `~$`) are skipped; unreadable files are skipped rather than aborting the whole rebuild.

**Scope note:** this app indexes the **shared vault only**. Zac's separate personal ZacAI vault and its `second-brain/` pointer-stub subfolder are explicitly out of scope — dropped early to keep indexing/onboarding simple (no stub-dedup logic, no vault-source filter UI). If chat is enabled, `llm::build_chunk_embeddings` re-walks the same vault independently (see §8) — two walks of the same small tree, not shared code, judged not worth a shared-walker abstraction at this scale.

## 4. Search (Search tab)

Query flow: debounced (~200ms) keystroke in the frontend → `search_vault` Tauri command → `search::search()`:
1. Tokenize the query on whitespace, lowercase each token.
2. For each token, build `FuzzyTermQuery` clauses (edit distance 1 for short tokens ≤5 chars, 2 for longer ones) against `title` (boosted 2.0×), `topics` (boosted 1.5×), and `body` (unboosted), OR'd together in a `BooleanQuery`.
3. Run `TopDocs::with_limit(30)`.
4. Generate highlighted snippets via tantivy's `SnippetGenerator` on the `body` field; falls back to a plain 220-char excerpt if snippet generation yields nothing.

This query-construction logic is the one place in the v1 codebase judged worth an explicit test (`index.rs`'s `rebuild_and_fuzzy_search_round_trip`), which also asserts typo tolerance (`"refrigirant"` still finds `"Refrigerant Tracking"`).

**Opening a result:** double-click (not single-click — deliberate, so scrolling/scanning results doesn't accidentally open files; a "Double-click a result to open the file" hint is shown under the search box). Calls `tauri-plugin-opener`'s `openPath` directly from the frontend (`window.__TAURI__.opener` global). See §11 for a real permission bug this hit and how it was fixed.

## 5. Onboarding & config

First run: no `config.json` in `app_config_dir()` (or no `vault_path` in it) → onboarding screen. `autodetect_vault` command probes for a OneDrive folder under `~/Library/CloudStorage` (macOS) or the user's home directory (Windows-style layout) containing `SVC-Product_Management - Second Brain/wikis` with at least one `.md` file. If found, the user gets a one-click "Use detected folder" button; otherwise a native folder-picker (`tauri-plugin-dialog`) is the fallback — every collaborator's OneDrive path differs by username/OS, so this can never be hardcoded.

Config persists as plain `serde_json`-written JSON — no config-management plugin, three fields doesn't warrant one. As of §6 below, `AppConfig` actually has three path fields (`vault_path`, `raw_path`, `inbox_path`), so "three fields doesn't warrant a plugin" now describes the real shape, not just the vault.

## 6. Raw & Inbox heads-up badges + new-session button (topbar, omnipresent)

Three always-visible controls in the topbar (not inside either search mode's panel, so they're on-screen regardless of which mode is active): a 📄 badge for unprocessed raw notes (`wiki-builder`'s job), a 📥 badge for pending inbox items (`inbox-review`'s job), and a Claude-icon button that opens a blank new Claude Desktop session (`claude://code/new`, no `q` param — nothing pre-filled, unlike the badges below). The Claude icon itself is copied from the locally-installed `/Applications/Claude.app` bundle (`ion-dist/images/claude_app_icon.png`) rather than recreated, so it matches the real app icon exactly.

Clicking either heads-up badge jumps straight into Claude Desktop with the matching slash command pre-filled.

**Paths & auto-detect:** `config::autodetect_second_brain_root()` is the OneDrive-probing logic originally written just for the vault, reused two ways — `autodetect_vault` = root + `wikis`, `autodetect_inbox` = root + `inbox/<slug>`. The inbox path needs one more piece the others don't: a local, per-machine slug read from `~/.claude/second-brain-inbox/inbox-slug` (written by the shared Second Brain's own onboarding, not by this app) — if that file doesn't exist, auto-detect simply returns `None`, matching the `inbox-review` skill's own "not set up yet" fallback rather than erroring.

**`autodetect_raw` is deliberately *not* built on the shared-root helper — a real bug, caught in live use.** The raw badge triggers `/wiki-builder`, the **personal** skill, which scans a hardcoded personal path (`~/Documents/Claude/Projects/Second Brain/Second Brain Obsidian/raw/`) — not the shared OneDrive vault's own `raw/` folder, even though both are named `raw/` and the shared vault is even symlinked *into* the personal one for browsing (easy to conflate; `wiki-builder`'s own `SKILL.md` explicitly warns about this exact mixup). The first version of this function wrongly reused `autodetect_second_brain_root()` and confidently auto-selected the *shared* team `raw/` folder for every user — caught when Zac noticed the wrong folder had been auto-picked on his own machine. Fixed by pointing `autodetect_raw` at `wiki-builder`'s own hardcoded scan path directly, independent of the OneDrive probe entirely. That literal path isn't a Zac-specific guess — it's the same path the skill itself expects for anyone who completed the standard Second Brain onboarding, even though Zac's happens to resolve there via a symlink into his personal ZacAI project. The counting logic (`<!-- wiki-processed -->` marker) needed no change — confirmed identical between `wiki-builder` and `sc-wiki-builder`.

**Counting (`counts.rs`):**
- `count_unprocessed_raw` — a raw `.md` file counts as unprocessed if it does **not** contain the literal marker `<!-- wiki-processed -->`, confirmed by reading `sc-wiki-builder`'s own `SKILL.md` rather than assumed. No frontmatter parsing — the marker is the sole authoritative signal per that skill, since raw files arrive from any teammate with inconsistent or missing frontmatter.
- `count_inbox_items` — counts `*.share.md` files, not total file count, confirmed from `inbox-review`'s own skill doc: each item has exactly one companion note (optionally paired with a payload file), so counting notes avoids double-counting pairs.
- Both have a fixture-based unit test (`counts.rs`'s `counts_raw_and_inbox_correctly`).

**Refresh:** same watcher-driven pattern as the vault/tantivy index (§3) — `start_watching_raw`/`start_watching_inbox` debounce filesystem events on each folder and call `recount_raw`/`recount_inbox`, which re-run the count and emit `raw-count-status`/`inbox-count-status` events the frontend listens for. No polling. Verified live: writing a synthetic unprocessed file into the real `raw/` folder moved the badge 1→2 within the debounce window, deleting it moved it back 1→2→1, with no app restart.

**Launch mechanism — `claude://code/new?q=<command>`, no `folder=` param.** This isn't guessed: it's the exact pattern already working in Zac's own `~/Documents/Claude/Projects/ZacAI/tools/dashboard-app-src/` (a separate Swift/WKWebView app). Two things carried over directly from reading that code:
1. `claude://` is the **general Claude Desktop app's** own registered URL scheme (`/Applications/Claude.app`'s `Info.plist` lists `CFBundleURLSchemes: ["claude"]`) — distinct from Claude Code's own `claude-cli://` terminal handler and the VS Code extension's `vscode://anthropic.claude-code` handler, both of which are real and documented but not what was wanted here.
2. **Deliberately no `folder=`/`cwd=` param** — the dashboard's own code comment explains why: a link-supplied folder is always treated as untrusted by Claude Desktop and re-prompts "Trust this workspace?" on every single click. `/wiki-builder` and `/inbox-review` are global skills operating on absolute paths, so no working directory is needed anyway.

Implemented entirely in the frontend (`main.js`'s `launchClaudeCommand`) via `openUrl` from `window.__TAURI__.opener` — no Rust command needed, same as how file-opening already works (§4). Verified live end-to-end: clicking the 📥 badge in the running app opened Claude Desktop with `/wiki-builder` pre-filled, unsent, exactly as documented.

**Settings screen:** the onboarding screen (§5) is no longer single-purpose — it now has three sections (Wikis, required; Raw notes, optional; Inbox, optional), each with the same auto-detect-then-confirm-or-browse UX, built from one `makePathSection` factory in `main.js` rather than tripled by hand. Vault still gates the app (a "Done" button only appears once it's set); raw/inbox are purely optional and can be left unset or configured later via the same ⚙ button (now labeled "Settings" instead of "Change vault folder").

**Not yet verified on Windows** — same category of gap as the rest of this doc's Windows caveats. `claude://` almost certainly works identically (Claude Desktop ships for both platforms and URL-scheme registration is standard cross-platform practice for Electron-style apps), but hasn't been tested on an actual Windows machine.

## 7. Logging & crash capture

`tracing` writes to a daily-rotating file under `app_log_dir()/logs/second-brain-search.log.<date>` (confirmed real location on macOS: `~/Library/Logs/com.zackwolf.second-brain-search/logs/`). Files older than 7 days are pruned on each startup. A `std::panic::set_hook` logs any panic (message + location) to the same file before the process dies, so crashes aren't silently lost.

There is no automatic upload/telemetry — deliberately out of scope. The "Logs" button in the search view's topbar calls `revealItemInDir` to open the log folder in Finder/Explorer; a collaborator experiencing an issue attaches the relevant log file manually (Slack/email) for Zac to read. This is a manual, user-initiated export, not a phone-home pipeline.

**Fixed:** `chat_availability` and `enable_chat` now call `tracing::info!` at each stage (called, downloading/loading, done + chunk count) — this gap (originally flagged here as "not yet fixed") turned out to matter in practice: it's what let the mutex-contention bug in §11 get diagnosed instead of staying a mystery.

**Devtools enabled deliberately, including in release builds** (`tauri = { features = ["devtools"] }` in `Cargo.toml`) — right-click → Inspect Element works even in the shipped `.app`/`.exe`. For a small internal tool, being able to ask a collaborator to open the console and paste what they see is worth more than whatever minor security/polish cost comes from devtools being reachable. This is what actually cracked the render-freeze bug in §11 — `tracing` alone wasn't enough since that bug was a *frontend* rendering issue, not a Rust-side one.

## 8. v2 — local chat ("AI Search", implemented)

**UI, as of the second redesign:** what started as a "Search"/"Chat" tab pair is now a single iOS-style slide switch in the topbar labeled "Keyword Search" / "AI Search" — a literal toggle (`<input type="checkbox">` styled as a track+knob), not two separate controls, because a two-tab or two-radio layout reads as "pick one of two screens" rather than "flip between two modes of the same search box," which is the actual mental model. Flipping the switch to AI Search **auto-triggers `enable_chat` immediately** — there's no separate "Enable AI Chat" button/warning screen anymore; selecting the mode *is* the enable action, exactly like pressing a button used to be. Already-enabled just shows the AI Search bar instantly. Hovering "AI Search" shows a tooltip ("Find related markdowns locally — no tokens.") via a custom CSS `::after` tooltip, not the native `title` attribute — the OS-level hover delay on `title` was noticeably slow and wasn't fixable from CSS/HTML alone.

**Why not Ollama:** it's a separate app collaborators would have to install and manage themselves — real friction for a non-technical PM team, and explicitly ruled out.

**Why not a persistent generative model:** flagged as a real performance concern (RAM/CPU on every collaborator's laptop). Resolved by defaulting to **extractive retrieval**, not standing generation:
- Default (`related_docs` command): the embedding model (`all-MiniLM-L6-v2`, ~90MB, via `candle-transformers`' BERT support) embeds the query, cosine-similarity ranks against pre-computed chunk embeddings, and results render as a ranked, highlighted list — no prose generation, near-instant.
- Opt-in ("Write me an answer" button → `write_answer` command): loads a tiny instruct model on demand (`Qwen2-0.5B-Instruct`, Q4_0 GGUF, ~350MB) for that one query only, generates a short answer citing the retrieved chunks, then **drops the model** (Rust scope-exit) — no standing background cost. Uses greedy sampling + a repeat penalty (1.15, last-64-tokens window) — omitting the repeat penalty was an early bug that produced looping/duplicated output (§11).

**Why `candle`** over binding to llama.cpp: pure Rust, no C/C++ toolchain needed at build time — relevant because the Windows build (§9) already has its own toolchain friction.

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

**Caveat — only validated on this machine.** This is a capable Mac; a representative slower Windows laptop (older Intel CPU, no Apple-Silicon-class SIMD) hasn't been tested and could be meaningfully slower. Same category of gap as the Windows build blocker in §9.

**Dev-mode note:** `npm run tauri dev` builds in **debug** profile, and candle's tensor math is dramatically slower unoptimized (a chunk-embedding build that takes seconds in release can peg the CPU for minutes in debug — this looked exactly like a hang during interactive testing and wasn't). Always test chat-related changes against a `cargo build --release` binary, not `tauri dev`.

## 9. Packaging & distribution

**Mac:** `cargo tauri build` → unsigned `.app`/`.dmg`. Xcode Command Line Tools are sufficient for this; full notarization/Apple Developer Program enrollment is explicitly skipped as unnecessary cost for a handful-of-teammates internal tool. First launch requires a Gatekeeper right-click → Open (or `xattr -cr` the `.app`).

**Windows — resolved via GitHub Actions.** Tauri's Windows target can't be cross-compiled from macOS (needs the MSVC linker, WebView2, NSIS/WiX bundling), so the actual build runs on a `windows-latest` GitHub Actions runner instead. Repo: `https://github.com/zwolf25/second-brain-hub` (private). Workflow: `.github/workflows/build-windows.yml`, triggers on push to `main` or manually (`workflow_dispatch`), builds both `.msi` and `.exe` (NSIS) installers and publishes them as a numbered GitHub Release (`windows-build-N`) rather than as an Actions artifact — Actions' native artifact storage (`productionresultssa8.blob.core.windows.net`) gets connection-reset on this network in both the CLI and a real browser, almost certainly a Zscaler-blocked Azure endpoint; Release assets are served from a different domain and download cleanly.

**Current update/distribution model — fully manual, deliberately.** There is no auto-updater (see below). Every push to `main` produces a new numbered Release with a fresh installer; each collaborator (Mac or Windows) re-downloads and reinstalls by hand to pick up changes. Fine for occasional low-volume updates to a handful of people; would get annoying fast with frequent releases.

**Explicitly out of scope (for now):** auto-updater, telemetry, multi-user sync/accounts/backend, a real vector DB (brute-force cosine is enough at this corpus size), a wikilink cross-reference graph UI (brackets are just stripped for display).

**Future consideration — auto-updater.** If manual re-download/reinstall becomes painful (frequent releases, more collaborators), Tauri has a built-in `tauri-plugin-updater` that checks a hosted `latest.json` manifest on launch and downloads+installs updates automatically. Real setup cost beyond what exists today: generate a signing keypair (updates must be signed), host `latest.json` somewhere the app can reach (could live in the same GitHub Release, updated by the CI workflow each build), and add the plugin + update-check UI. Not built now — explicitly deferred, not forgotten.

## 10. Capabilities / permissions (`src-tauri/capabilities/default.json`)

Tauri's ACL is deny-by-default per-command **and** per-scope — granting a command permission (e.g. `opener:allow-open-path`) is not sufficient if that permission also requires a path scope and none is configured (see §11 for the bug this caused). Current grants: `core:default`, `opener:default` (URL opening + reveal-in-Finder), `opener:allow-open-path` scoped to `$HOME/**` (covers the OneDrive vault at any username; broad but low-risk since it only permits "open with OS default handler," not raw file read/write), `opener:allow-open-url` scoped to `claude://*` (powers the Raw/Inbox badge launch, §6 — `opener:default`'s bundled URL scope only covers `mailto:`/`tel:`/`http:`/`https:`, not custom schemes), `dialog:default` (folder picker).

## 11. Real bugs found and fixed during live testing

Live GUI testing (not just automated tests) surfaced several real issues automated tests didn't catch — worth recording since they're the kind of thing that'll recur if this pattern (Tauri + candle + reqwest) gets reused elsewhere:

1. **Download hang risk:** `reqwest::blocking::get` (the free function) has no timeout. A real network failure would hang the download — and the UI waiting on it — forever with no error. **Fixed:** build an explicit `reqwest::blocking::Client` with a 15s connect timeout and 20-minute overall timeout.
2. **UI looked frozen during the compute-only phase:** once a model finishes downloading, `enable_chat` still has real work left (loading the model, embedding ~1132 chunks) with no download-progress bytes to report — the progress bar/label just sat stale, indistinguishable from actually being stuck. **Fixed:** added a `chat-status` event emitted before each compute phase, decoupled from the byte-progress `model-download-progress` event.
3. **Silent permission failure on file-open:** `opener:allow-open-path` was granted but with no `scope` — Tauri's ACL denies-by-default even with the permission listed, so every open attempt failed with "Not allowed to open path X," silently, because the JS click handler had no `.catch()`. **Fixed both ends:** added `"allow": [{ "path": "$HOME/**" }]` to the capability, and added error surfacing (`alert(...)` on rejection) so any future permission/IO failure is visible instead of silent.
4. **Debug-build slowness looked like a hang:** see §8's dev-mode note — a legitimate ~minutes-long debug-profile compute phase was initially mistaken for a stuck process. Diagnosed via `ps`/`lsof` (99% CPU, zero open network connections — ruled out a network hang, pointed at unoptimized compute instead) before concluding it needed `--release`, not a code fix.
5. **Mutex held across expensive computation → concurrent calls block, reads as "stuck loading forever":** `enable_chat`, `related_docs`, and `write_answer` all locked `embedding_model`/`chunk_embeddings` and held the lock for the *entire* duration of the embedding computation (`build_chunk_embeddings`, `top_related`'s `embed()` call), not just the brief moment of reading the value out. A second call touching the same lock (e.g. re-selecting AI Search, or `chat_availability` firing while `enable_chat` was still running) blocked waiting on it — with a 27-second `enable_chat` call observed live, this read as a permanent hang rather than a normal wait. **Fixed:** wrapped both fields in `Arc` (`Mutex<Option<Arc<EmbeddingModel>>>`, `Mutex<Arc<Vec<ChunkEmbedding>>>`) so every call site clones the `Arc` and drops the lock immediately, before doing any real work — the lock is now only ever held for a pointer clone, never across computation. General lesson for this codebase: never hold a `Mutex` guard across an `await`-equivalent or a slow synchronous call — clone what you need out from under the lock first.
6. **Real UI freeze during embedding computation, silently swallowing every loading-indicator design tried:** three different loading-UI implementations (a progress bar nested in the panel, a `position: fixed` overlay, a spinner nested in the panel) all failed identically — the indicator's `classList` change demonstrably executed (confirmed via `console.log` and `tracing::info!` on both sides, ruling out a JS/Rust logic bug) but nothing ever appeared on screen. Root cause, per Zac's own diagnosis: the CPU-heavy embedding computation starves the webview's render thread badly enough that a DOM mutation made *immediately before* the blocking `invoke()` call never gets painted — the freeze hits before the browser's next paint cycle fires. This wasn't a CSS/layout bug at all; every DOM-structure change made while chasing it was solving a non-problem. **Fixed:** force a real paint to commit before starting the heavy work — `await new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)))` between showing the loading state and calling `invoke("enable_chat")`. Double-rAF (not a single rAF, not `setTimeout`) is the standard technique for "block until the browser has definitely painted the current frame." General lesson: if a DOM update provably runs (via logging) but never renders, suspect a starved render thread before suspecting the DOM/CSS — and diagnose via devtools console + `tracing`, not by redesigning the markup.
7. **Raw-folder auto-detect confidently found the wrong folder** (§6) — built on the same shared-OneDrive-root helper used for the vault and inbox, so it always found the *shared team* `raw/` folder, even though the raw badge is meant to trigger `/wiki-builder`, the *personal* skill scanning a completely different path. Caught live: Zac noticed the auto-selected folder didn't match his actual personal raw notes. The two paths are genuinely easy to conflate — same folder name, and the shared vault is even symlinked into the personal one for browsing — which is exactly why `wiki-builder`'s own `SKILL.md` calls out the mixup explicitly. **Fixed:** `autodetect_raw` now checks `wiki-builder`'s own hardcoded scan path directly, with no dependency on the OneDrive/shared-root probe at all. General lesson: two features that both auto-detect "a `raw/` folder" aren't necessarily looking for the same folder — check what the *downstream skill* actually scans, not just what the feature is named.

## 12. Known risks / open items

1. File-watcher robustness against OneDrive sync behavior (partial writes, transient lock files) is defensive-by-design (skip-on-error, filter `~$*`) but was only exercised by real ambient vault activity during verification on Mac, not a deliberate edit-while-running stress test, and not yet exercised on Windows at all — worth a dedicated smoke test there.
2. No auto-updater (§9) — every update is a manual re-download/reinstall. Deferred, not forgotten; Tauri's `tauri-plugin-updater` is the concrete path if this becomes painful.
3. Chat retrieval quality on technical/identifier-dense queries is a documented, evidenced gap (§8) — recommended fix is hybrid tantivy+embedding retrieval, not yet built.
4. The embedding computation blocking the render thread (§11 #6) is only worked around at the one call site that triggers it interactively (`activateAiSearch`'s double-rAF yield) — if another future UI path ever calls `enable_chat` (or similarly heavy work) without that same yield, the freeze-swallows-the-loading-UI bug will resurface there. Worth factoring the yield into a shared helper if a third call site appears.
5. Full-rewalk-on-any-change indexing (§3) assumes the vault stays small; revisit if it ever measurably slows down.
6. `spike-candle/` at the project root is throwaway validation code from before the real `llm.rs` implementation landed — safe to delete, or keep as a quick benchmark harness for future model-swap decisions.
7. Speed/quality numbers throughout §8 are only validated on this Mac — a representative Windows/lower-spec machine may behave meaningfully differently, same open question as the Windows build blocker.
8. The `claude://code/new?q=...` badge launch (§6) is verified working end-to-end on Mac (both the raw OS-level `open` call and the app's own permission-scoped `openUrl`) but not yet on Windows — same category of gap as the rest of this list, not expected to differ (Claude Desktop ships for both platforms and URL-scheme registration is standard cross-platform practice) but genuinely untested.
9. `autodetect_raw` (§6, §11 #7) assumes every collaborator's personal Second Brain vault lives at the literal `~/Documents/Claude/Projects/Second Brain/Second Brain Obsidian/raw/` path — true for anyone who followed the standard onboarding (it's `wiki-builder`'s own hardcoded scan target), but if a collaborator's personal setup deviates from that convention, auto-detect will just find nothing and fall back to manual browse, same as today, not silently wrong. Worth confirming this holds across a few real collaborator machines, not just Zac's.
