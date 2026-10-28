# Airlock: build plan

> **Airlock** (working name) is a local "broadcast delay with a brain" for streamers. OBS streams
> into Airlock on localhost; Airlock holds the stream for a few seconds, uses AI to catch what
> shouldn't air (swearing, slurs, self-doxxing, secrets on screen, ToS-risk speech), redacts it,
> and forwards the clean stream to Twitch/YouTube/Kick.
>
> Differentiator vs. existing tools (CleanStream, Bleep Censor AI, RISM): those are word-list
> bleepers. Airlock adds **context** (LLM judge), **on-screen secret/PII detection**, **private
> terms**, and a **streamer review window** with veto/approve inside the delay.

Read `docs/ARCHITECTURE.md` before starting any task.

---

## How agents use this file

1. Pick the **first unchecked task** in the **lowest unfinished phase** whose dependencies are done
   (unless the human assigned a specific task).
2. Read the task's *Done when*. That is the acceptance test. Don't expand scope; add new tasks
   to the right phase instead.
3. Before marking done, run:
   - `cargo test` (from repo root; engine crates)
   - `cargo check -p airlock-desktop` (if you touched the desktop crate)
   - `npm run build` in `apps/desktop` (if you touched the UI)
4. Mark it done by changing `- [ ]` to `- [x]` and appending ` — done YYYY-MM-DD: <one-line note>`.
5. If you made an architectural choice, add a line to **Decisions log** at the bottom.
6. If you're blocked, leave the box unchecked and add ` — BLOCKED: <reason>` so the next agent sees it.

Conventions live in `AGENTS.md`.

---

## Phase 0: Foundations

- [x] 0.1 Cargo workspace with engine crates + Tauri app as a non-default member — done 2026-10-28
- [x] 0.2 `airlock-core`: media time (`Ts`, `Dur`, `Span`), `Detection`, `Rect`, evidence masking — done 2026-10-28
- [x] 0.3 `Policy` with per-category mode/action/threshold/padding; TOML load/save; `config/policy.default.toml` kept in sync by a test — done 2026-10-28
- [x] 0.4 `DelayLine<T>` (ordered insert, drain by playhead) — done 2026-10-28
- [x] 0.5 `DecisionEngine`: policy application, padding, merging (audio by time, video by time+region), approve/veto, retire to incident log — done 2026-10-28
- [x] 0.6 `airlock-detect`: token wordlist (leetspeak, elongation, prefix entries, allowlist), spoken PII regex, private terms, on-screen secret scanner (stream keys, cloud keys, JWTs, private keys, entropy fallback) — done 2026-10-28
- [x] 0.7 Provider traits: `SpeechRecognizer`, `ContextJudge`, `TextRecognizer`; `ScriptedRecognizer` for tests/demo — done 2026-10-28
- [x] 0.8 LLM judge prompt (numbered words → index ranges), tolerant JSON parser, deadline budget — done 2026-10-28
- [x] 0.9 Audio bleep/mute with click-free fades; CPU pixelate/blackout — done 2026-10-28
- [x] 0.10 `Session` orchestrator + `UiEvent` stream + incident CSV; end-to-end test with scripted ASR, fake judge, fake OCR — done 2026-10-28
- [x] 0.11 Tauri 2 + Angular shell; demo mode replays a scripted stream through the real engine; review UI (delay meter, airlock queue with veto/approve, transcript, incident log, CSV export) — done 2026-10-28
- [ ] 0.12 Set up git repo + CI (GitHub Actions, Windows + Linux): `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check`, `npm run build`
  - *Done when:* a PR runs all four checks green.
- [ ] 0.13 Generate TS types from Rust (`ts-rs` or `specta` + `tauri-specta`) and replace hand-written `airlock.types.ts`
  - *Done when:* changing a Rust field breaks the Angular build until the UI is updated.
- [ ] 0.14 Browser preview mode: when not running inside Tauri, `AirlockService` replays a recorded `UiEvent` JSON fixture so UI work can happen in `ng serve`
  - *Done when:* `npm start` shows the dashboard animating with fixture data.

## Phase 1: Audio MVP (OBS → Airlock → Twitch, bleeping swear words)

Goal: a streamer can actually go live through Airlock with wordlist bleeping and a dump button.
Video is passed through untouched (no re-encode).

- [ ] 1.1 **Spike:** choose the media stack. Prototype ingest of an RTMP stream from OBS with `ffmpeg-next` on Windows (static FFmpeg build, NVENC available). Fallback candidates: GStreamer (`gstreamer-rs`), FFmpeg sidecar processes.
  - *Done when:* a 10-minute OBS → Airlock → local file capture has no A/V drift and the decision is logged in the Decisions log with reasons.
- [ ] 1.2 Ingest server: listen on `rtmp://127.0.0.1:1935/live` (configurable port), accept one publisher, rebase PTS to `Ts`, handle publisher reconnect without restarting the app.
  - *Done when:* OBS can start/stop streaming to Airlock repeatedly; UI shows ingest connected/disconnected.
- [ ] 1.3 Audio path: decode to f32 48 kHz interleaved → `DelayLine` → `apply_audio_redactions` → AAC encode.
- [ ] 1.4 Video passthrough: delay encoded video packets in a `DelayLine` keyed on the same `Ts`; remux unchanged.
  - *Done when:* output keyframes and timestamps are valid (ffprobe clean), A/V sync within 1 frame after 1 hour.
- [ ] 1.5 Output pacing loop: drives the playhead from a monotonic wall clock anchored at first packet; drains both delay lines; muxes FLV; pushes to RTMPS egress.
- [ ] 1.6 Egress: RTMPS to Twitch (then YouTube, Kick) with reconnect/backoff; stream key read from OS keychain (`keyring` crate) via `EgressTarget.stream_key_ref`, never written to disk or logs.
  - *Done when:* a 30-minute test stream to a private/unlisted channel has no drops; killing the network for 10s recovers.
- [ ] 1.7 ASR tap (temporary): feed the live edge to a cloud STT with word timestamps (Deepgram or AssemblyAI streaming) behind `SpeechRecognizer`, so Phase 1 ships before local Whisper. Opt-in, key in keychain.
- [ ] 1.8 **Dump button** (classic broadcast delay feature): one click/hotkey mutes all audio currently in the buffer (and optionally blacks out video) as a `Category`-less manual redaction.
  - *Done when:* pressing dump at time T guarantees nothing captured in (T − delay, T] airs unmuted.
- [ ] 1.9 Fail-safe behavior: if analysis stalls (no ASR words for N seconds while audio has energy), show a red banner and apply the configured fallback (`warn` | `mute` | `extend delay`).
- [ ] 1.10 Replace demo-only wiring in `apps/desktop/src-tauri` with a real session started from the UI ("Go live" / "Stop"), keeping demo mode behind a dev toggle.
- [ ] 1.11 OBS setup guide in the app: what to put in OBS → Settings → Stream (Custom server `rtmp://127.0.0.1:1935/live`, recommended high-bitrate local settings).
- [ ] 1.12 **Milestone:** stream for 1 hour to Twitch through Airlock; every wordlist hit in the test script is bleeped; CPU < 15% on the dev machine beyond OBS's own usage.

## Phase 2: Local speech recognition + spoken PII

- [ ] 2.1 Decide threading/ownership model for live redactions (session thread publishes immutable snapshots vs. shared `RwLock`) and implement channels between media, ASR, judge, OCR and session threads.
- [ ] 2.2 `WhisperRecognizer` via `whisper-rs` (feature `whisper`, sub-feature `cuda`): sliding window (3s/1s hop), word timestamps, LocalAgreement-2 commit policy so words are never emitted twice or retracted.
  - *Done when:* on the eval set (Phase 6), p95 word finalization latency ≤ 1.5s on an RTX 5090 and WER within 2 points of offline Whisper on the same clips.
- [ ] 2.3 Model manager: download/verify ggml models on first run with progress UI; pick a default by detected GPU/VRAM; CPU fallback (`small.en`).
- [ ] 2.4 VAD gate (Silero VAD or whisper's built-in) so silence doesn't burn GPU or produce hallucinated words.
- [ ] 2.5 Spoken numbers normalization: ASR often writes "six three one five five five…" or "631 555 0123"; extend PII patterns to cover spelled-out and space-separated digits.
- [ ] 2.6 Wordlists: replace seed lists with maintained, licensed lists per language (check licenses); ship `slurs.txt` populated; UI to add/remove custom words and allowlist entries.
- [ ] 2.7 Private terms UI: add your real name, family names, school, employer, town, street. Stored encrypted at rest (OS keychain-derived key). Never included in cloud prompts (add a test).
- [ ] 2.8 Speaker-agnostic caveat: guests/Discord voices are in the same mix. Document it; optionally let the user add a second audio input (mic only) for higher-accuracy ASR on the streamer.

## Phase 3: Context judge (the differentiator)

- [ ] 3.1 `OllamaJudge`: HTTP to `/api/chat` with JSON format, temperature 0, client timeout = `deadline_budget`. Recommended default model chosen by eval (start with qwen3 8B / llama3.1 8B).
- [ ] 3.2 Judge scheduler: every ~1.5s or every N new words, call `Session::next_judge_window()` on the judge thread; drop a request if a newer one supersedes it; record latency.
- [ ] 3.3 Late-result handling: if a flag arrives after its span aired, file it in the incident log as "missed live" (for VOD), never discard silently.
- [ ] 3.4 `AnthropicJudge` (opt-in cloud): look up the current Messages API format and model IDs at docs.claude.com (don't hardcode from memory); explicit consent toggle in settings ("transcript text leaves this PC").
- [ ] 3.5 Per-platform rule packs: short platform-specific guidance injected into the prompt (Twitch / YouTube / Kick), versioned in `config/platform_rules/`.
- [ ] 3.6 Streamer rules: free-text rules from settings ("don't say my ex's name", "no sponsor competitor names") passed to the judge.
- [ ] 3.7 Prompt-injection hardening: chat messages read aloud can contain "ignore previous instructions". Treat transcript strictly as data (delimiters, instruction at end, schema-validated output); add adversarial eval cases.
- [ ] 3.8 **Milestone:** on the eval set, ToS-risk precision ≥ 0.85 at the default threshold with p95 latency inside the budget.

## Phase 4: On-screen secrets and PII

- [ ] 4.1 Video transcode path: decode frames (hardware decode), apply `video_fx`, encode with NVENC/AMF/QSV (fallback x264). Passthrough stays the default until a video category is enabled.
  - *Done when:* 1080p60 transcode adds < 1 frame of latency and < 10% GPU on the dev machine.
- [ ] 4.2 Analysis frame tap: `FrameSampler` at 4 fps, downscale for OCR, keep normalized coordinates.
- [ ] 4.3 `WindowsOcr` via the `windows` crate (`Windows.Media.Ocr`); returns line-level boxes. Evaluate PaddleOCR (ONNX Runtime + CUDA) as a higher-accuracy option behind the same trait.
- [ ] 4.4 Retroactive blur: a detection at sample k covers from sample k−1 (already provided by `FrameSampler::offer`) plus policy padding. Verify with a test video where a key appears for a single frame between samples.
- [ ] 4.5 Region stability: track boxes across samples (IoU matching) so a blur region follows a scrolling terminal/browser instead of flickering.
- [ ] 4.6 Window/scene awareness (optional): read OBS scene/source names via obs-websocket and let users mark sources as "always scan" or "never scan" (e.g., skip the gameplay capture, always scan the browser/IDE capture).
- [ ] 4.7 Vision model pass (optional, later): small local VLM for DMs/notifications/NSFW that OCR can't classify; same deadline rules as the judge.
- [ ] 4.8 **Milestone:** in a scripted test (IDE with `.env` open, Discord DM popup, email client), every secret/PII line is blurred before it airs and nothing else is.

## Phase 5: Streamer experience

- [ ] 5.1 Global hotkeys (`tauri-plugin-global-shortcut`): veto next, approve next, dump, toggle safe mode. Configurable.
- [ ] 5.2 Settings UI: policy editor (per-category mode/action/threshold/padding) backed by the TOML schema, delay length, providers, platforms, private terms, wordlists.
- [ ] 5.3 Onboarding wizard: detect GPU → pick models → connect platform keys → configure OBS automatically via obs-websocket (set custom server) → test stream.
- [ ] 5.4 Compact "on-air" overlay mode: small always-on-top window with the countdown queue for single-monitor streamers.
- [ ] 5.5 Incident log upgrades: export as EDL / chapter markers / YouTube timestamps so VOD cleanup is fast; per-session history.
- [ ] 5.6 Health panel: ingest bitrate, buffer fill, ASR/judge/OCR latency p50/p95, dropped frames, GPU usage.
- [ ] 5.7 Light theme + accessibility pass (keyboard navigation, screen reader labels, color-blind-safe category colors).
- [ ] 5.8 System tray + start with OBS (detect OBS launch).

## Phase 6: Evaluation and quality

- [ ] 6.1 Eval corpus format: audio/video clip + labeled spans (category, start, end, region) in JSONL under `evals/` (large media in Git LFS or external storage).
- [ ] 6.2 Corpus v1: TTS-generated speech for profanity/PII/ToS cases + recorded real speech with gaming background noise; screen recordings with planted fake secrets.
- [ ] 6.3 `airlock-eval` binary: runs the full pipeline offline over the corpus; reports precision/recall per category, span overlap, and latency percentiles; JSON + markdown output.
- [ ] 6.4 CI regression gate on a small fast subset (scripted ASR) so detector/policy changes can't silently regress.
- [ ] 6.5 False-positive review loop: from the incident log, one click "this was wrong" exports an anonymized case into the eval set (opt-in).

## Phase 7: Performance and hardening

- [ ] 7.1 Move blur/pixelate to GPU (wgpu compute or FFmpeg CUDA filters) once 4.1 profiling says so.
- [ ] 7.2 Adjustable delay mid-stream: ramp the delay by time-stretching audio slightly (±5%) instead of jumping.
- [ ] 7.3 Crash safety: if Airlock dies, the stream dies. Document the OBS backup-server setting; add a watchdog that restarts the pipeline and resumes egress within seconds.
- [ ] 7.4 Memory caps on all buffers; soak test 8 hours with no growth.
- [ ] 7.5 SRT ingest option for lower and more stable localhost latency.
- [ ] 7.6 Security review: keychain usage, no secrets in logs/crash dumps, signed model downloads, dependency audit (`cargo audit`, `npm audit`).

## Phase 8: Ship

- [ ] 8.1 Windows installer via Tauri bundler (NSIS/MSI), code signing, auto-update (`tauri-plugin-updater`).
- [ ] 8.2 macOS build (Apple Vision OCR instead of Windows OCR; VideoToolbox encode).
- [ ] 8.3 Licensing: free tier (wordlist + dump + spoken PII) vs. Pro (context judge, screen scanning, multistream, cloud providers). License key check that works offline.
- [ ] 8.4 Privacy policy and in-app data statement: what (if anything) leaves the machine, per provider.
- [ ] 8.5 Landing page + demo video (before/after of a stream key being blurred retroactively).
- [ ] 8.6 Opt-in telemetry: crash reports and latency stats only, never content.

## Later / ideas

- Multistream with per-platform policies (Kick looser than YouTube) — needs one encode per policy.
- Chat-aware judge: read Twitch chat so the judge knows when the streamer is reading a message aloud.
- Music detection (DMCA): fingerprint background music and mute it in the VOD/live.
- Team/agency mode: a moderator approves/vetoes remotely during the delay window.
- OBS plugin variant for users who don't want a separate app.

---

## Decisions log

- D1 2026-10-28: **Local relay app**, not a cloud relay or OBS plugin. Private, no server costs, uses the streamer's GPU.
- D2 2026-10-28: **Tauri 2 + Angular.** Rust core for the media pipeline and low latency; Angular UI.
- D3 2026-10-28: **Pluggable, local-first AI.** Whisper + Ollama by default; cloud providers (STT, Claude) are opt-in behind the same traits.
- D4 2026-10-28: All timing in **media time microseconds** (`Ts`); wall clock only paces output.
- D5 2026-10-28: Detectors report, `DecisionEngine` decides. `Session` is single-threaded and synchronous for testability; other threads talk to it over channels.
- D6 2026-10-28: Context-judge findings (`tos_risk`) and screen PII default to **suggest** mode; profanity, slurs, spoken PII and screen secrets default to **apply**.
- D7 2026-10-28: Default delay **8s**. Long enough for ASR + judge + review, short enough for chat interaction.
- D8 2026-10-28: Secrets are **pixelated** (irreversible) rather than lightly blurred; `blur` in policy should map to a strong blur or pixelation.
- D9 2026-10-28: **Private terms never leave the machine**; stream/API keys live only in the OS keychain.
- D10 2026-10-28: Phase 1 ships with **video passthrough** and a temporary cloud STT tap, so a usable product exists before local Whisper and video transcode.

## Open questions (for Jared)

- Final name and trademark check ("Airlock" is a working name).
- Pricing model: one-time vs. subscription; what's in the free tier.
- Windows-only for v1, or macOS from the start?
- Should the dump button also extend the delay automatically (like broadcast "delay rebuild")?
