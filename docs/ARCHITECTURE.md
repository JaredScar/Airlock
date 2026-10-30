# Airlock architecture

Airlock is a local relay that sits between the streaming app and the platform. OBS and Streamlabs Desktop both publish to the local RTMP ingest. It holds the
stream in a delay buffer (default 8s), analyzes the *live* edge of that buffer with AI,
and redacts problems in the *delayed* copy before it leaves the machine.

```
                         ┌────────────── analysis taps (live edge) ──────────────┐
                         │  ASR (Whisper) ─► wordlist + PII + private terms       │
                         │                └► rolling transcript ─► LLM judge      │
                         │  frame sampler ─► OCR ─► secret/PII scanner            │
                         └──────────────────────────┬─────────────────────────────┘
                                                    ▼ Detections
 encoder ──RTMP/SRT──► ingest ──► DelayLine (audio) ─► Session/DecisionEngine ─► Redactions
  (OBS or Streamlabs,       │      DelayLine (video)          ▲                       │
   localhost)               │                                 │ approve / veto        │
                            │                        Review UI (Angular)              │
                            ▼                                                         ▼
               playhead = live − delay ──► audio_fx (bleep/mute) ──► AAC ──┐
                                      └──► video_fx (blur/pixelate) ─► NVENC ─┴─► mux ─► RTMPS ─► Twitch/YouTube/Kick
                                           (or packet passthrough when no video redaction)
```

## Crates

| Crate | Owns | Status |
|---|---|---|
| `airlock-core` | `Ts`/`Span` media time, `Detection`, `Policy`, `DelayLine`, `DecisionEngine` | Implemented + tested |
| `airlock-detect` | Wordlist (leet/elongation aware, token-based), spoken PII regex, private terms, on-screen secret/PII scanner | Implemented + tested |
| `airlock-asr` | `SpeechRecognizer` trait, audio downmix/resample, silence gate, `ScriptedRecognizer`, Whisper stub | Gate tested; Whisper TODO |
| `airlock-llm` | `ContextJudge` trait, prompt builder, response parser, deadline budget, Ollama and Anthropic HTTP | Ollama and consented Anthropic clients tested against a local server |
| `airlock-vision` | `TextRecognizer` trait, `FrameSampler`, downscale, region tracking, Windows OCR (`winocr` feature) | Sampler, downscale, tracking, and Windows OCR tested |
| `airlock-media` | Audio bleep/mute, delay-then-redact PCM, time-stretch, passthrough video hold, output pacer, FLV tags, AAC via ffmpeg, SRT listen, RTMPS URL publish with retry, CUDA region pixelate then NVENC (CPU pixelate when CUDA is missing), CPU blackout | File audio, an hour of video timestamps, a local RTMP publish and retry, an SRT tone, and a GPU-pixelated clip tested; OBS ingest TODO |
| `airlock-pipeline` | `Session` orchestrator, session-thread bus, `StudioSettings`, sealed private terms, judge scheduler, eval scoring, incident exports | Implemented + end-to-end test |
| `apps/desktop` | Tauri 2 shell + Angular review UI, settings, compact mode, global hotkeys, system tray | Demo and settings working; OBS autostart and real ingest TODO |

## Core rules

1. **One clock.** Everything is media time (`Ts`, microseconds from the first ingested packet).
   Wall clock is only used to pace output. Never compare wall time to media time directly.
2. **Detections are cheap, redactions are policy.** Detectors only report what they saw.
   `DecisionEngine` applies confidence thresholds, padding, merge rules and modes.
3. **Retroactive redaction is the superpower.** Because output is delayed, a detection at
   time *t* can redact media from before *t* (padding, OCR sample gaps). Anything detected
   before the playhead reaches it is safe.
4. **Fast path never waits on slow path.** Wordlist/PII/secret detectors run on every batch.
   The LLM judge runs beside them with a deadline; if it's late, its finding is logged for
   VOD cleanup instead of blocking anything.
5. **Fail loud, not open.** If an analysis tap dies, the UI shows it immediately and the
   session can apply a fallback (e.g., mute on lost ASR is configurable). Never silently
   pass unanalyzed media while looking healthy.
6. **No secrets in logs.** Every `Detection.evidence` goes through `mask_evidence`. Stream
   keys and API keys live in the OS keychain only.
7. **Private terms stay local.** They're matched in `airlock-detect` and never put into any
   prompt sent to a cloud provider.

## Latency budget (default 8s delay)

| Stage | Budget | Notes |
|---|---|---|
| Ingest + decode | ~50ms | localhost |
| ASR word finalized after spoken | ≤ 1.5s p95 | whisper large-v3-turbo on GPU, 3s window / 1s hop |
| Wordlist + PII + private terms | < 1ms | per batch |
| LLM judge | ≤ `deadline_budget` ≈ 5s | runs every ~1.5s over the rolling window |
| OCR per sample | ≤ 300ms | 4 fps sampling → worst-case detection lag ~550ms |
| Streamer review window | whatever remains | UI shows a countdown per item |
| Redact + encode + mux | ~1 frame | NVENC |

If p95 ASR + judge exceeds `delay − 1.5s`, the UI warns and suggests a longer delay.

## Threading (target, Phase 2)

- Media thread(s): ingest → `DelayLine`s; output pacing loop pulls due items, asks the engine
  for redactions, applies effects, encodes, sends.
- ASR thread, OCR thread, judge thread: read from taps, send results over channels.
- Session thread: owns `Session`, applies results, emits `UiEvent`s to Tauri, and publishes an immutable `OutputSnapshot` after each message.
- The output loop reads that snapshot. It does not lock the engine.
