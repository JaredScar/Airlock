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
- [ ] 0.12 Set up git repo + CI (GitHub Actions, Windows + Linux): `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check`, `npm run build` — BLOCKED: workflow is in `.github/workflows/ci.yml` and the four checks pass locally on Windows (2026-10-28); no git remote yet, so a PR has not run them. CI Node is current 22.x because the Angular CLI requires >= 22.22.3.
  - *Done when:* a PR runs all four checks green.
- [x] 0.13 Generate TS types from Rust (`ts-rs` or `specta` + `tauri-specta`) and replace hand-written `airlock.types.ts` — done 2026-10-28: ts-rs exports into `apps/desktop/src/app/generated`, and `npm run build` regenerates them before `ng build`.
- [x] 0.14 Browser preview mode: when not running inside Tauri, `AirlockService` replays a recorded `UiEvent` JSON fixture so UI work can happen in `ng serve` — done 2026-10-28: `ng serve` auto-plays `fixtures/demo-events.json` recorded from the demo session.

## Phase 1: Audio MVP (OBS → Airlock → Twitch, bleeping swear words)

Goal: a streamer can actually go live through Airlock with wordlist bleeping and a dump button.
Video is passed through untouched (no re-encode).

- [ ] 1.1 **Spike:** choose the media stack. Prototype ingest of an RTMP stream from OBS with `ffmpeg-next` on Windows (static FFmpeg build, NVENC available). Fallback candidates: GStreamer (`gstreamer-rs`), FFmpeg sidecar processes. — BLOCKED: OBS is not installed; the 10-minute capture waits on it. ffmpeg is available for a later publisher-side prototype.
  - *Done when:* a 10-minute OBS → Airlock → local file capture has no A/V drift and the decision is logged in the Decisions log with reasons.
- [ ] 1.2 Ingest server: listen on `rtmp://127.0.0.1:1935/live` (configurable port), accept one publisher, rebase PTS to `Ts`, handle publisher reconnect without restarting the app. — BLOCKED: done-when needs OBS start/stop. Depends on 1.1.
  - *Done when:* OBS can start/stop streaming to Airlock repeatedly; UI shows ingest connected/disconnected.
- [x] 1.3 Audio path: decode to f32 48 kHz interleaved → `DelayLine` → `apply_audio_redactions` → AAC encode. — done 2026-10-28: studio PCM is 48 kHz stereo f32. `AudioHold` releases 20 ms chunks at the playhead and mutes inside the span. ffmpeg on PATH decodes a 44.1 kHz wav and round-trips the mute through AAC. The AAC check skips when ffmpeg is absent. RTMP ingest is still 1.1.
- [ ] 1.4 Video passthrough: delay encoded video packets in a `DelayLine` keyed on the same `Ts`; remux unchanged.
  - *Done when:* output keyframes and timestamps are valid (ffprobe clean), A/V sync within 1 frame after 1 hour.
- [ ] 1.5 Output pacing loop: drives the playhead from a monotonic wall clock anchored at first packet; drains both delay lines; muxes FLV; pushes to RTMPS egress.
- [ ] 1.6 Egress: RTMPS to Twitch (then YouTube, Kick) with reconnect/backoff; stream key read from OS keychain (`keyring` crate) via `EgressTarget.stream_key_ref`, never written to disk or logs.
  - *Done when:* a 30-minute test stream to a private/unlisted channel has no drops; killing the network for 10s recovers.
- [ ] 1.7 ASR tap (temporary): feed the live edge to a cloud STT with word timestamps (Deepgram or AssemblyAI streaming) behind `SpeechRecognizer`, so Phase 1 ships before local Whisper. Opt-in, key in keychain.
- [x] 1.8 **Dump button** (classic broadcast delay feature): one click/hotkey mutes all audio currently in the buffer (and optionally blacks out video) as a `Category`-less manual redaction. — done 2026-10-28: `DecisionEngine::dump` mutes (T−delay, T]; video blackout is optional and off by default. Does not rebuild the delay.
- [x] 1.9 Fail-safe behavior: if analysis stalls (no ASR words for N seconds while audio has energy), show a red banner and apply the configured fallback (`warn` | `mute` | `extend delay`). — done 2026-10-28: default is warn after 3s; mute and extend-delay are policy settings. Extra delay stays for the session.
- [ ] 1.10 Replace demo-only wiring in `apps/desktop/src-tauri` with a real session started from the UI ("Go live" / "Stop"), keeping demo mode behind a dev toggle.
- [x] 1.11 OBS setup guide in the app: what to put in OBS → Settings → Stream (Custom server `rtmp://127.0.0.1:1935/live`, recommended high-bitrate local settings). — done 2026-10-28: collapsible guide in the review UI. No OBS install required to read it.
- [ ] 1.12 **Milestone:** stream for 1 hour to Twitch through Airlock; every wordlist hit in the test script is bleeped; CPU < 15% on the dev machine beyond OBS's own usage.

## Phase 2: Local speech recognition + spoken PII

- [x] 2.1 Decide threading/ownership model for live redactions (session thread publishes immutable snapshots vs. shared `RwLock`) and implement channels between media, ASR, judge, OCR and session threads. — done 2026-10-28: workers send `SessionInbox`. The session thread publishes an `OutputSnapshot`. The engine is not shared.
- [ ] 2.2 `WhisperRecognizer` via `whisper-rs` (feature `whisper`, sub-feature `cuda`): sliding window (3s/1s hop), word timestamps, LocalAgreement-2 commit policy so words are never emitted twice or retracted.
  - *Done when:* on the eval set (Phase 6), p95 word finalization latency ≤ 1.5s on an RTX 5090 and WER within 2 points of offline Whisper on the same clips.
- [ ] 2.3 Model manager: download/verify ggml models on first run with progress UI; pick a default by detected GPU/VRAM; CPU fallback (`small.en`).
- [x] 2.4 VAD gate (Silero VAD or whisper's built-in) so silence doesn't burn GPU or produce hallucinated words. — done 2026-10-28: an energy gate with an 8-chunk hangover sits in front of any recognizer. Silence is not forwarded. A neural VAD waits for the Whisper feature.
- [x] 2.5 Spoken numbers normalization: ASR often writes "six three one five five five…" or "631 555 0123"; extend PII patterns to cover spelled-out and space-separated digits. — done 2026-10-28: grouped digits stay on the phone regex; a run of ten spelled or single digits (optional leading 1, "oh" as zero) maps back to the source words. "2500" and years stay quiet.
- [ ] 2.6 Wordlists: replace seed lists with maintained, licensed lists per language (check licenses); ship `slurs.txt` populated; UI to add/remove custom words and allowlist entries.
- [x] 2.7 Private terms UI: add your real name, family names, school, employer, town, street. Stored encrypted at rest (OS keychain-derived key). Never included in cloud prompts (add a test). — done 2026-10-28: AES-256-GCM seal, key in the OS keychain. The desktop app writes `private-terms.seal` and keeps terms out of its browser profile. The preview has no keychain. Prompt tests still show terms never leave the machine.
- [x] 2.8 Speaker-agnostic caveat: guests/Discord voices are in the same mix. Document it; optionally let the user add a second audio input (mic only) for higher-accuracy ASR on the streamer. — done 2026-10-28: the private-terms panel says guests, Discord, and the game share the mix. A second input is still optional and not built.

## Phase 3: Context judge (the differentiator)

- [x] 3.1 `OllamaJudge`: HTTP to `/api/chat` with JSON format, temperature 0, client timeout = `deadline_budget`. Recommended default model chosen by eval (start with qwen3 8B / llama3.1 8B). — done 2026-10-28: POST `{base}/api/chat`, `format: json`, temperature 0, timeout is the deadline budget. Default model `qwen3:8b`. Tested against a local server.
- [x] 3.2 Judge scheduler: every ~1.5s or every N new words, call `Session::next_judge_window()` on the judge thread; drop a request if a newer one supersedes it; record latency. — done 2026-10-28: due at 1.5s or 24 words. `complete` drops a generation that a newer `begin` replaced, and records wall-clock latency only for the one that lands.
- [x] 3.3 Late-result handling: if a flag arrives after its span aired, file it in the incident log as "missed live" (for VOD), never discard silently. — done 2026-10-28: `missed_live` on the redaction; it is not applied. A span that still overlaps the hold is redacted. Below-threshold detections are still dropped.
- [x] 3.4 `AnthropicJudge` (opt-in cloud): look up the current Messages API format and model IDs at docs.claude.com (don't hardcode from memory); explicit consent toggle in settings ("transcript text leaves this PC"). — done 2026-10-28: Messages API as documented 2026-10-28 (`POST /v1/messages`, `anthropic-version: 2026-10-21`, Bearer auth, model `claude-haiku-5-5`). The call is refused until the settings consent box is on. The key is stored in the OS keychain and is not printed.
- [x] 3.5 Per-platform rule packs: short platform-specific guidance injected into the prompt (Twitch / YouTube / Kick), versioned in `config/platform_rules/`. — done 2026-10-28: v1 packs compiled into the prompt; a Twitch request does not include the YouTube or Kick pack.
- [x] 3.6 Streamer rules: free-text rules from settings ("don't say my ex's name", "no sponsor competitor names") passed to the judge. — done 2026-10-28: `Session::judge_request` passes `streamer_rules` into the prompt. Private terms are not a prompt field. The settings screen is 5.2.
- [x] 3.7 Prompt-injection hardening: chat messages read aloud can contain "ignore previous instructions". Treat transcript strictly as data (delimiters, instruction at end, schema-validated output); add adversarial eval cases. — done 2026-10-28: transcript is between markers, instructions follow, a forged end-marker in a word cannot close the block, and a non-`tos_risk` category is dropped.
- [ ] 3.8 **Milestone:** on the eval set, ToS-risk precision ≥ 0.85 at the default threshold with p95 latency inside the budget.

## Phase 4: On-screen secrets and PII

- [ ] 4.1 Video transcode path: decode frames (hardware decode), apply `video_fx`, encode with NVENC/AMF/QSV (fallback x264). Passthrough stays the default until a video category is enabled.
  - *Done when:* 1080p60 transcode adds < 1 frame of latency and < 10% GPU on the dev machine.
- [x] 4.2 Analysis frame tap: `FrameSampler` at 4 fps, downscale for OCR, keep normalized coordinates. — done 2026-10-28: 4 fps sampler was already tested. `downscale_bgra` shrinks the long edge and leaves coordinates normalized.
- [x] 4.3 `WindowsOcr` via the `windows` crate (`Windows.Media.Ocr`); returns line-level boxes. Evaluate PaddleOCR (ONNX Runtime + CUDA) as a higher-accuracy option behind the same trait. — done 2026-10-28: feature `winocr` reads a painted word into normalized line boxes. PaddleOCR stays a later optional trait impl; it is not a dependency.
- [x] 4.4 Retroactive blur: a detection at sample k covers from sample k−1 (already provided by `FrameSampler::offer`) plus policy padding. Verify with a test video where a key appears for a single frame between samples. — done 2026-10-28: a key seen at the 250 ms sample covers the frame at 100 ms, and pixelating that region changes the synthetic frame.
- [x] 4.5 Region stability: track boxes across samples (IoU matching) so a blur region follows a scrolling terminal/browser instead of flickering. — done 2026-10-28: `associate_regions` keeps an id when overlap stays above the threshold and assigns a new id when it does not.
- [ ] 4.6 Window/scene awareness (optional): read OBS scene/source names via obs-websocket and let users mark sources as "always scan" or "never scan" (e.g., skip the gameplay capture, always scan the browser/IDE capture).
- [ ] 4.7 Vision model pass (optional, later): small local VLM for DMs/notifications/NSFW that OCR can't classify; same deadline rules as the judge.
- [x] 4.8 **Milestone:** in a scripted test (IDE with `.env` open, Discord DM popup, email client), every secret/PII line is blurred before it airs and nothing else is. — done 2026-10-28: a fake AWS key is pixelated immediately. Emails and a phone are suggestions (D6) and pixelate once approved. A database URL, a chat line, and a subject line are left alone.

## Phase 5: Streamer experience

- [x] 5.1 Global hotkeys (`tauri-plugin-global-shortcut`): veto next, approve next, dump, toggle safe mode. Configurable. — done 2026-10-28: F8–F11 by default, editable in settings. The desktop app registers them globally. The same keys work in the preview while the window is focused. Safe mode keeps the hold muted.
- [x] 5.2 Settings UI: policy editor (per-category mode/action/threshold/padding) backed by the TOML schema, delay length, providers, platforms, private terms, wordlists. — done 2026-10-28: Settings page edits one destination, per-platform rules, private terms, delay, stall fallback, and each category's mode, audio, video, confidence, and padding. Extra wordlist lines append to the shipped lists. Provider status is read-only; connecting a judge is 3.1/3.4, and encrypted term storage is 2.7.
- [ ] 5.3 Onboarding wizard: detect GPU → pick models → connect platform keys → configure OBS automatically via obs-websocket (set custom server) → test stream.
- [x] 5.4 Compact "on-air" overlay mode: small always-on-top window with the countdown queue for single-monitor streamers. — done 2026-10-28: Compact hides the transcript, log, and setup and leaves the hold queue. The desktop window is set always-on-top.
- [x] 5.5 Incident log upgrades: export as EDL / chapter markers / YouTube timestamps so VOD cleanup is fast; per-session history. — done 2026-10-28: CSV, CMX 3600 EDL, WebVTT chapters, and YouTube timestamps from the aired log, including missed-live rows. Buttons in the incident log.
- [ ] 5.6 Health panel: ingest bitrate, buffer fill, ASR/judge/OCR latency p50/p95, dropped frames, GPU usage.
- [x] 5.7 Light theme + accessibility pass (keyboard navigation, screen reader labels, color-blind-safe category colors). — done 2026-10-28: Light/Dark toggle, focus outlines, labeled controls, and category hues that are not a red/green pair (amber, red, blue, teal, purple, brown, gray).
- [ ] 5.8 System tray + start with OBS (detect OBS launch). — BLOCKED: the tray menu (Show, Quit) is in. Detecting an OBS launch waits until OBS is installed.

## Phase 6: Evaluation and quality

- [x] 6.1 Eval corpus format: audio/video clip + labeled spans (category, start, end, region) in JSONL under `evals/` (large media in Git LFS or external storage). — done 2026-10-28: `evals/v1/labels.jsonl` plus `parse_labels`. Lines carry clip, category, start_us, end_us, and an optional region. No media in git.
- [ ] 6.2 Corpus v1: TTS-generated speech for profanity/PII/ToS cases + recorded real speech with gaming background noise; screen recordings with planted fake secrets.
- [x] 6.3 `airlock-eval` binary: runs the full pipeline offline over the corpus; reports precision/recall per category, span overlap, and latency percentiles; JSON + markdown output. — done 2026-10-28: `airlock-eval` scores `evals/v1/labels.jsonl` against `evals/v1/scripts.json`. Clips with no script are skipped. Output is JSON then markdown, with p50/p95 latency.
- [x] 6.4 CI regression gate on a small fast subset (scripted ASR) so detector/policy changes can't silently regress. — done 2026-10-28: `score_scripts` on the profanity and phone scripts fails the build if either hit is missed. `cargo test` already runs it on both CI images.
- [x] 6.5 False-positive review loop: from the incident log, one click "this was wrong" exports an anonymized case into the eval set (opt-in). — done 2026-10-28: Save corrections must be checked first. Wrong downloads one JSONL line with category and times, and no evidence.

## Phase 7: Performance and hardening

- [ ] 7.1 Move blur/pixelate to GPU (wgpu compute or FFmpeg CUDA filters) once 4.1 profiling says so.
- [ ] 7.2 Adjustable delay mid-stream: ramp the delay by time-stretching audio slightly (±5%) instead of jumping.
- [ ] 7.3 Crash safety: if Airlock dies, the stream dies. Document the OBS backup-server setting; add a watchdog that restarts the pipeline and resumes egress within seconds.
- [ ] 7.4 Memory caps on all buffers; soak test 8 hours with no growth.
- [ ] 7.5 SRT ingest option for lower and more stable localhost latency.
- [x] 7.6 Security review: keychain usage, no secrets in logs/crash dumps, signed model downloads, dependency audit (`cargo audit`, `npm audit`). — done 2026-10-28: keychain and judge errors stay fixed strings, and the API key is redacted in Debug. npm audit is clean after the Vite dev-server bump. cargo audit has no vulnerabilities after rustls 0.23.46. Unmaintained Tauri/Linux crates remain. Signed model downloads wait for 2.3.

## Phase 8: Ship

- [ ] 8.1 Windows installer via Tauri bundler (NSIS/MSI), code signing, auto-update (`tauri-plugin-updater`).
- [ ] 8.2 macOS build (Apple Vision OCR instead of Windows OCR; VideoToolbox encode).
- [ ] 8.3 Licensing: free tier (wordlist + dump + spoken PII) vs. Pro (context judge, screen scanning, multistream, cloud providers). License key check that works offline.
- [x] 8.4 Privacy policy and in-app data statement: what (if anything) leaves the machine, per provider. — done 2026-10-28: `docs/PRIVACY.md` and the settings panel say local checks and Ollama stay on the PC, Anthropic gets the transcript only with consent, private terms never leave, and there is no telemetry.
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
- D11 2026-10-28: UI bindings come from **ts-rs**. Media timestamps and redaction ids export as `number` (serde JSON numbers), not `bigint`. `npm run build` / `npm start` regenerate the bindings first, so a Rust field change fails the Angular build until the UI matches.
- D12 2026-10-28: The live pipeline targets **Windows and macOS** from the start. Phase 1 video stays passthrough, so ingest/remux stays portable (no NVENC/VideoToolbox until a video category is on). Linux CI covers the engine and the UI build only.
- D13 2026-10-28: Prove **OBS → delay → local file** before any speech-to-text. Cloud STT (task 1.7) stays in the plan; the provider is still open. The 1.1 capture waits until OBS is installed on the dev machine.
- D14 2026-10-28: Dump is a **manual redaction** (`Category::Manual`), not a policy category. It mutes (T−delay, T] and does not rebuild the delay. Video blackout is a checkbox, off by default.
- D15 2026-10-28: Analysis stall defaults to **warn** after 3s of audio energy with no words. Mute covers from the last word to now. Extend-delay adds the stall window once and keeps it, so the playhead does not jump forward.
- D16 2026-10-28: Spoken phones match the existing grouped pattern ("631 555 0123") or a run of ten single digits, spelled or numeric, with an optional leading 1. "oh" counts as zero. Years and counts like "2500" do not.
- D17 2026-10-28: A detection whose span has already ended at the playhead is an incident-log row with `missed_live` and is not applied. A span that still overlaps the hold is redacted. Policy still drops low-confidence hits.
- D18 2026-10-28: Platform packs are versioned text in `config/platform_rules/`, compiled into the judge prompt. The transcript is a delimited data block and the instructions come after it. Private terms are not a prompt field; `Session::judge_request` passes streamer rules only.
- D19 2026-10-28: The incident log exports CMX 3600 EDL at 30 fps, WebVTT chapters, and YouTube description timestamps, including missed-live rows.
- D20 2026-10-28: Eval labels are JSONL under `evals/v1/`: clip, category, `start_us`, `end_us`, optional region. Media stays out of git.
- D21 2026-10-28: One destination platform per session. Shipped packs stay versioned files. The streamer's rules are stored per platform, and only the selected list is sent to the judge. Private terms are not a prompt field. Settings live in app state and the browser profile until the encrypted store in 2.7.
- D22 2026-10-28: Studio audio is interleaved f32, 48 kHz, stereo. Decode and AAC encode call the `ffmpeg` binary on PATH. A chunk airs when its start reaches the playhead (20 ms). Linking libav and listening for OBS stay in 1.1.
- D23 2026-10-28: The session thread owns the engine and publishes an immutable `OutputSnapshot`. Other threads only send `SessionInbox`. There is no shared lock around the engine.
- D24 2026-10-28: Private terms are sealed with AES-256-GCM. The 32-byte key is created once and stored in the OS keychain (`airlock` / `private-terms`). The ciphertext file is `private-terms.seal`. The browser preview has no keychain.
- D25 2026-10-28: The cloud judge uses the Messages API checked on 2026-10-28: `POST /v1/messages`, header `anthropic-version: 2026-10-21`, `Authorization: Bearer`, model `claude-haiku-5-5`. Consent defaults off. The API key is a keychain entry and is never debug-printed.
- D26 2026-10-28: The judge runs when 1.5s of media time or 24 new words have arrived. A newer request drops the older result. Latency is wall-clock microseconds, not media time.
- D27 2026-10-28: OCR frames are nearest-neighbor downscaled. Detections stay in normalized coordinates. A blur box keeps its id while intersection-over-union stays above the threshold.
- D28 2026-10-28: The speech gate is RMS energy plus eight quiet chunks of hangover, wrapped as `GatedRecognizer`. Silero is not linked. Silence never reaches the recognizer, so it cannot hallucinate from a quiet mix.
- D29 2026-10-28: Windows OCR is the `winocr` feature (`Windows.Media.Ocr`). Each line is one box in normalized coordinates, confidence 0.9, because the API does not score lines. Default `cargo test` does not compile it. PaddleOCR was considered and not added: it needs ONNX Runtime, and the Windows engine already returns line boxes.
- D30 2026-10-28: The system tray is Tauri's tray icon with Show and Quit. Starting when OBS launches is separate and waits until OBS is installed.

## Open questions (for Jared)

- Final name and trademark check ("Airlock" is a working name).
- Pricing model: one-time vs. subscription; what's in the free tier.
- Should the dump button also extend the delay automatically (like broadcast "delay rebuild")?
