# Second Brain Hub

Desktop app (Tauri, vanilla HTML/CSS/JavaScript) for the shared Second Brain vault.

- **Download:** https://github.com/zwolf25/second-brain-hub/releases/latest
- **Current design:** [ARCHITECTURE.md](ARCHITECTURE.md). **Release history:** [CHANGELOG.md](CHANGELOG.md). **Open work:** [BACKLOG.md](BACKLOG.md).

## Develop

```sh
npm install
npx tauri dev
```

## Test

```sh
node --check src/main.js
node --test src/lib.test.js scripts/*.test.mjs
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml
```

The UI has no automated tests; use [SMOKE-CHECKLIST.md](SMOKE-CHECKLIST.md). For a test build with a separate settings profile:

```sh
npx tauri build --debug --no-bundle --config '{"identifier":"com.zackwolf.sbh-test"}'
```

A separate profile does not isolate everything. The test build still uses whatever vault folders you set in it. On a new profile it can also turn on launch at login and repoint the installed app's login item to the test build.

## Release

Pushing a non-documentation change to `main` publishes a release. Bump the version in `package.json` and `src-tauri/tauri.conf.json` first. The `manifest` job in `.github/workflows/build.yml` writes `latest.json` after all platform builds succeed.
