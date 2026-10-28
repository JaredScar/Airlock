# Airlock architecture

Airlock is a local relay that sits between OBS and the streaming platform. It holds the
stream in a delay buffer (default 8s), analyzes the *live* edge of that buffer with AI,
and redacts problems in the *delayed* copy before it leaves the machine.

```
                         ┌────────────── analysis taps (live edge) ──────────────┐
                         │  ASR (Whisper) ─► wordlist + PII + private terms       │
                         │                └► rolling transcript ─► LLM judge      │
                         │  frame sampler ─► OCR ─► secret/PII scanner            │
                         └──────────────────────────┬─────────────────────────────┘
                                                    ▼ Detections
 OBS ──RTMP/SRT──► ingest ──► DelayLine (audio) ─► Session/DecisionEngine ─► Redactions
  (localhost,          │      DelayLine (video)          ▲                       │
   high bitrate)       │                                 │ approve / veto        │
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
| `airlock-asr` | `SpeechRecognizer` trait, audio downmix/resample, `ScriptedRecognizer`, Whisper stub | Trait + helpers done; Whisper TODO |
| `airlock-llm` | `ContextJudge` trait, prompt builder, response parser, deadline budget, Ollama/Anthropic stubs | Prompt/parse done; HTTP TODO |
| `airlock-vision` | `TextRecognizer` trait, `FrameSampler`, Windows OCR stub | Sampler done; OCR TODO |
| `airlock-media` | Audio bleep/mute with fades, CPU pixelate/blackout, egress config; ffmpeg ingest/egress behind `ffmpeg` feature | Effects done; ingest/egress TODO |
| `airlock-pipeline` | `Session` orchestrator, `UiEvent`s, incident CSV export | Implemented + end-to-end test |
| `apps/desktop` | Tauri 2 shell + Angular review UI; demo mode runs a scripted stream through the real engine | Demo working; real ingest TODO |

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
- Session thread: owns `Session`, applies results, emits `UiEvent`s to Tauri.
- The output loop needs read access to redactions: share via `Arc<RwLock<..>>` snapshot or
  have the session thread publish an immutable snapshot each tick. Decide in task 2.1.
