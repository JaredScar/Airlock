# Agent guide (Cursor, Claude, Codex, etc.)

Start every task by reading `PLAN.md` (pick the task, follow its *Done when*) and
`docs/ARCHITECTURE.md` (core rules). Update `PLAN.md` checkboxes as described there.

## Repo map

- `crates/airlock-core`: shared types, policy, delay line, decision engine. No I/O.
- `crates/airlock-detect`: deterministic detectors (wordlists, PII, private terms, screen secrets).
- `crates/airlock-asr` / `airlock-llm` / `airlock-vision`: provider traits + implementations.
- `crates/airlock-media`: ingest/egress and audio/video redaction effects.
- `crates/airlock-pipeline`: `Session` orchestrator and `UiEvent`s.
- `apps/desktop`: Tauri 2 (`src-tauri`) + Angular (`src`). Demo mode lives in `src-tauri/src/demo.rs`.
- `config/`: default policy TOML and wordlists.

## Commands

```bash
cargo test                       # engine crates (default workspace members)
cargo clippy --all-targets -- -D warnings
cargo check -p airlock-desktop   # Tauri crate (needs webkit2gtk on Linux)
cd apps/desktop && npm install && npm run tauri dev   # run the app
cd apps/desktop && npm run build                      # UI build only
```

## Rules

- **Keep `airlock-core` pure**: no I/O, threads, or heavy deps. It must stay fast to test.
- **Media time only**: use `Ts`/`Dur`/`Span`. Never mix in `Instant` except in output pacing.
- **Never log or display raw secrets/PII**: use `mask_evidence` for anything user-visible.
  Stream keys and API keys come from the OS keychain; never from config files, env dumps or logs.
- **Private terms never go to a cloud provider.** Add a test if you touch prompt building.
- **Every detector and policy change needs a test.** Fast deterministic tests live next to the
  code; whole-pipeline behavior goes in `crates/airlock-pipeline/tests/`.
- **Policy defaults** live in both `Policy::default()` and `config/policy.default.toml`; a test
  keeps them identical. Change both.
- **New provider = new impl of the existing trait.** Don't add provider-specific branches to `Session`.
- Heavy optional deps (ffmpeg-next, whisper-rs, ort, windows) go behind cargo features so
  `cargo test` keeps working on a bare machine.
- Rust edition 2024; prefer `thiserror` errors in libraries, no `unwrap()` outside tests except
  on invariants with a comment.
- UI: Angular standalone components + signals; global styles in `src/styles.css` using the CSS
  variables there. Rust ⇄ TS types must match (see PLAN 0.13).
- Keep diffs scoped to the task. If you notice other problems, add a task to `PLAN.md`.
