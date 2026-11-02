//! Go live: listen for one local publisher, push its audio through the delay,
//! recognize speech on a 16 kHz copy, and ask the context judge when a Pro
//! license is saved. When that license allows on-screen scanning, video frames
//! are sampled on the same clock and a secret is pixelated before it airs.
//! Cloud speech is used only when the streamer opted in, a Pro license allows
//! it, and a Deepgram key is in the keychain. Otherwise Parakeet runs when its
//! model folder is on disk, and Whisper is the fallback.

use crate::EVENT_CHANNEL;
use airlock_asr::audio::{downmix_to_mono, has_speech_energy, resample_linear};
use airlock_asr::{
    ASR_SAMPLE_RATE, DeepgramConfig, DeepgramRecognizer, ParakeetRecognizer, SpeechRecognizer,
    WhisperConfig, WhisperRecognizer,
};
use airlock_core::{Action, Dur, Platform, Rect, Span, StallFallback, Target, Ts};
use airlock_llm::{AnthropicJudge, ContextJudge, JudgeRequest, OllamaJudge};
use airlock_media::{
    AudioHold, DelayedForward, FrameCover, INGEST_PORT, LiveFrame, PublishTarget, VideoCover,
    VideoHold, cover_frame, key_account, platform_name, publish_base, secure_publish_url,
    serve_capture, serve_capture_frames,
};
use airlock_pipeline::{Feature, JudgeScheduler, JudgeWindow, Session, UiEvent, allows};
use airlock_vision::{
    FrameRef, FrameSampler, OllamaScene, PixelFormat, TextRecognizer, WindowsOcr, bgra_png,
    downscale_bgra, scene_deadline,
};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager};

pub fn start(
    app: AppHandle,
    slot: Arc<Mutex<Option<Session>>>,
    settings: airlock_pipeline::StudioSettings,
    generation: Arc<AtomicU64>,
    mine: u64,
) -> Result<(), String> {
    let config = settings.session_config()?;
    let fallback = config.policy.stall_fallback;
    let delay = config.policy.delay();
    let platform = settings.platform;
    let judge_on = allows(&settings.license_key, Feature::ContextJudge);
    let screen_on = allows(&settings.license_key, Feature::ScreenScan);
    let cloud = settings.cloud_judge_consent && allows(&settings.license_key, Feature::CloudJudge);
    let speech_key =
        if settings.cloud_stt_consent && allows(&settings.license_key, Feature::CloudSpeech) {
            crate::keys::load_speech_key().ok()
        } else {
            None
        };
    let selected = settings.publish_platforms();
    let limited = settings.saved_destinations().len() > selected.len();
    let mut ready_names = Vec::new();
    let mut missing_names = Vec::new();
    let mut targets = Vec::new();
    for destination in &selected {
        let name = platform_name(*destination).to_string();
        match publish_target(*destination) {
            Some(url) => {
                targets.push(PublishTarget {
                    url,
                    name: name.clone(),
                });
                ready_names.push(name);
            }
            None => missing_names.push(name),
        }
    }
    let primary = selected.first().copied().unwrap_or(platform);
    let also: Vec<Platform> = selected.iter().copied().skip(1).collect();
    let rules = settings.rules_for(&selected);
    let dir = app.path().app_data_dir().map_err(|err| err.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
    std::thread::spawn(move || {
        if generation.load(Ordering::SeqCst) != mine {
            return;
        }
        let model = airlock_asr::runtime_speech_model(crate::detect_vram_mb());
        let model_path = dir.join("models").join(model.file_name());
        let parakeet_dir = dir.join("models").join(airlock_asr::PARAKEET_DIR);
        let parakeet_on = airlock_asr::parakeet_ready(&parakeet_dir);
        let model_ready = speech_key.is_some()
            || parakeet_on
            || airlock_asr::file_matches(&model_path, model.sha256());
        {
            let mut guard = match slot.lock() {
                Ok(guard) => guard,
                Err(_) => return,
            };
            let mut session = Session::new(config);
            if model_ready {
                session.note_recognizer(true);
            }
            *guard = Some(session);
        }
        let speech_tx = if model_ready {
            let (tx, rx) = std::sync::mpsc::channel();
            let app_asr = app.clone();
            let slot_asr = Arc::clone(&slot);
            let generation_asr = Arc::clone(&generation);
            let judge = judge_on.then(|| {
                spawn_judge(
                    app.clone(),
                    Arc::clone(&slot),
                    Arc::clone(&generation),
                    mine,
                    JudgeTarget {
                        platform: primary,
                        also,
                        rules,
                    },
                    cloud,
                )
            });
            let engine = match speech_key {
                Some(key) => SpeechEngine::Cloud(key),
                None if parakeet_on => SpeechEngine::Parakeet(parakeet_dir),
                None => SpeechEngine::Whisper(model_path),
            };
            std::thread::spawn(move || {
                recognize(app_asr, slot_asr, generation_asr, mine, engine, judge, rx);
            });
            Some(tx)
        } else {
            let _ = app.emit(
                EVENT_CHANNEL,
                &UiEvent::AnalysisStall {
                    active: true,
                    fallback,
                    message: "Speech recognition is not connected yet. Download the speech model in Settings, then go live again.".into(),
                },
            );
            None
        };
        let forwarding = !targets.is_empty();
        let _ = app.emit(EVENT_CHANNEL, &UiEvent::Ingest { connected: false });
        let _ = app.emit(
            EVENT_CHANNEL,
            &UiEvent::Egress {
                active: false,
                detail: waiting_detail(&ready_names, &missing_names, limited, platform),
            },
        );
        let delayed = Arc::new(Mutex::new(Vec::<f32>::new()));
        let covers = Arc::new(Mutex::new(Vec::<FrameCover>::new()));
        let media = Arc::new(Mutex::new(LiveMedia {
            audio: AudioHold::new(delay),
            video: VideoHold::new(delay),
            forward: DelayedForward::new(targets),
            cover: VideoCover::default(),
        }));
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop_flag = Arc::clone(&stop);
        let watch = generation.clone();
        std::thread::spawn(move || {
            while watch.load(Ordering::SeqCst) == mine {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            stop_flag.store(true, Ordering::SeqCst);
        });
        let app_pcm = app.clone();
        let speech_out = speech_tx.clone();
        let slot_pcm = slot.clone();
        let media_pcm = Arc::clone(&media);
        let delayed_pcm = Arc::clone(&delayed);
        let media_video = Arc::clone(&media);
        let covers_video = Arc::clone(&covers);
        let screen_tx = if screen_on {
            let (tx, rx) = std::sync::mpsc::sync_channel(1);
            let app_screen = app.clone();
            let slot_screen = Arc::clone(&slot);
            let generation_screen = Arc::clone(&generation);
            let covers_screen = Arc::clone(&covers);
            std::thread::spawn(move || {
                watch_screen(
                    app_screen,
                    slot_screen,
                    generation_screen,
                    mine,
                    delay,
                    covers_screen,
                    rx,
                );
            });
            Some(tx)
        } else {
            None
        };
        let on_connection = {
            let app = app.clone();
            move |connected| {
                let _ = app.emit(EVENT_CHANNEL, &UiEvent::Ingest { connected });
            }
        };
        let on_video = move |packet| {
            if let Ok(mut media) = media_video.lock() {
                media.video.push(packet);
            }
        };
        let on_pcm = move |pcm: airlock_media::Pcm| {
            if generation.load(Ordering::SeqCst) != mine {
                return;
            }
            if let Some(tx) = &speech_out {
                let mono = downmix_to_mono(&pcm.samples, pcm.channels.max(1));
                let speech = resample_linear(&mono, pcm.sample_rate, ASR_SAMPLE_RATE);
                let _ = tx.send((pcm.start, speech));
            }
            let end =
                pcm.start + Dur::from_secs_f64(pcm.frames() as f64 / pcm.sample_rate.max(1) as f64);
            let energetic = has_speech_energy(&pcm.samples);
            let mut guard = match slot_pcm.lock() {
                Ok(guard) => guard,
                Err(_) => return,
            };
            let Some(session) = guard.as_mut() else {
                return;
            };
            let Ok(mut media) = media_pcm.lock() else {
                return;
            };
            let _ = media.audio.push(pcm);
            session.on_live_time(end);
            session.note_audio_energy(energetic);
            let redactions = session.engine().all().to_vec();
            let samples = media.audio.release(end, &redactions);
            let pictures = media.video.release(end);
            let ready = covers_video
                .lock()
                .map(|guard| guard.clone())
                .unwrap_or_default();
            let pictures = media.cover.apply(pictures, &redactions, &ready);
            if let Ok(mut delayed) = delayed_pcm.lock() {
                delayed.extend_from_slice(&samples);
            }
            let report = media.forward.write(&pictures, &samples);
            if !report.started.is_empty() || !report.failed.is_empty() {
                let connected = media.forward.connected_names();
                let _ = app_pcm.emit(
                    EVENT_CHANNEL,
                    &UiEvent::Egress {
                        active: !connected.is_empty(),
                        detail: forward_detail(&connected, &report.failed),
                    },
                );
            }
            for event in session.drain_events() {
                let _ = app_pcm.emit(EVENT_CHANNEL, &event);
            }
        };
        let _ = if let Some(tx) = screen_tx {
            serve_capture_frames(
                INGEST_PORT,
                &dir,
                &stop,
                on_connection,
                on_video,
                on_pcm,
                move |frame| {
                    let _ = tx.try_send(frame);
                },
            )
        } else {
            serve_capture(INGEST_PORT, &dir, &stop, on_connection, on_video, on_pcm)
        };
        drop(speech_tx);
        let delayed = delayed
            .lock()
            .map(|samples| samples.clone())
            .unwrap_or_default();
        let _ = write_wav(&dir.join("delayed.wav"), &delayed);
        drop(media);
        let _ = app.emit(EVENT_CHANNEL, &UiEvent::Ingest { connected: false });
        if forwarding {
            let _ = app.emit(
                EVENT_CHANNEL,
                &UiEvent::Egress {
                    active: false,
                    detail: format!(
                        "Forwarding to {} stopped.",
                        destination_sentence(&ready_names)
                    ),
                },
            );
        }
    });
    Ok(())
}

struct LiveMedia {
    audio: AudioHold,
    video: VideoHold,
    forward: DelayedForward,
    cover: VideoCover,
}

enum SpeechEngine {
    Whisper(std::path::PathBuf),
    Parakeet(std::path::PathBuf),
    Cloud(String),
}

fn recognize(
    app: AppHandle,
    slot: Arc<Mutex<Option<Session>>>,
    generation: Arc<AtomicU64>,
    mine: u64,
    engine: SpeechEngine,
    judge: Option<JudgeHook>,
    rx: std::sync::mpsc::Receiver<(Ts, Vec<f32>)>,
) {
    let mut recognizer: Box<dyn SpeechRecognizer> = match engine {
        SpeechEngine::Cloud(api_key) => match DeepgramRecognizer::new(DeepgramConfig {
            api_key,
            ..DeepgramConfig::default()
        }) {
            Ok(recognizer) => Box::new(recognizer),
            Err(err) => {
                report_recognizer_failure(&app, &slot, &err.to_string());
                return;
            }
        },
        SpeechEngine::Whisper(model_path) => {
            let config = WhisperConfig {
                model_path,
                use_gpu: cfg!(feature = "cuda"),
                // Two seconds of audio, then a quarter-second hop, so a word
                // shows during the delay. The tail of the window waits one hop.
                // A pause transcribes immediately.
                window_ms: 2_000,
                hop_ms: 250,
                ..WhisperConfig::default()
            };
            match WhisperRecognizer::new(config) {
                Ok(recognizer) => Box::new(recognizer),
                Err(err) => {
                    report_recognizer_failure(&app, &slot, &err.to_string());
                    return;
                }
            }
        }
        SpeechEngine::Parakeet(model_dir) => match ParakeetRecognizer::new(model_dir) {
            Ok(recognizer) => Box::new(recognizer),
            Err(err) => {
                report_recognizer_failure(&app, &slot, &err.to_string());
                return;
            }
        },
    };
    while generation.load(Ordering::SeqCst) == mine {
        let Some((start, samples)) = next_speech(&rx) else {
            break;
        };
        if generation.load(Ordering::SeqCst) != mine {
            break;
        }
        let end = start + Dur((samples.len() as i64) * 1_000_000 / i64::from(ASR_SAMPLE_RATE));
        note_recognizer_progress(&slot, end);
        if samples.is_empty() {
            continue;
        }
        match recognizer.push(&samples, start) {
            Ok(words) => deliver_words(&app, &slot, words, judge.as_ref()),
            Err(err) => {
                report_recognizer_failure(&app, &slot, &err.to_string());
                return;
            }
        }
    }
    if let Ok(words) = recognizer.flush() {
        deliver_words(&app, &slot, words, judge.as_ref());
    }
}

fn note_recognizer_progress(slot: &Mutex<Option<Session>>, at: Ts) {
    let Ok(mut guard) = slot.lock() else {
        return;
    };
    let Some(session) = guard.as_mut() else {
        return;
    };
    session.note_recognizer_progress(at);
}

/// Takes the next audio, plus anything already queued. Audio older than seven
/// seconds has reached the delay, so only the newer part is transcribed.
fn next_speech(rx: &std::sync::mpsc::Receiver<(Ts, Vec<f32>)>) -> Option<(Ts, Vec<f32>)> {
    let first = rx.recv().ok()?;
    let mut parts = vec![first];
    while let Ok(more) = rx.try_recv() {
        parts.push(more);
    }
    let (last_at, last_samples) = parts.last()?;
    let last_end = *last_at + sample_dur(last_samples.len());
    let keep = last_end - Dur::from_millis(7_000);
    let mut start = None;
    let mut samples = Vec::new();
    for (at, chunk) in parts {
        if at + sample_dur(chunk.len()) <= keep {
            continue;
        }
        if start.is_none() {
            let drop_n = if at < keep {
                ((keep.0 - at.0).max(0) * i64::from(ASR_SAMPLE_RATE) / 1_000_000) as usize
            } else {
                0
            };
            start = Some(at.max(keep));
            if drop_n < chunk.len() {
                samples.extend_from_slice(&chunk[drop_n..]);
            }
            continue;
        }
        samples.extend_from_slice(&chunk);
    }
    Some((start.unwrap_or(last_end), samples))
}

fn sample_dur(n: usize) -> Dur {
    Dur((n as i64) * 1_000_000 / i64::from(ASR_SAMPLE_RATE))
}

fn deliver_words(
    app: &AppHandle,
    slot: &Mutex<Option<Session>>,
    words: Vec<airlock_core::Word>,
    judge: Option<&JudgeHook>,
) {
    if words.is_empty() {
        return;
    }
    let count = words.len();
    let now = words.last().map(|word| word.span.end).unwrap_or(Ts::ZERO);
    let pending = {
        let Ok(mut guard) = slot.lock() else {
            return;
        };
        let Some(session) = guard.as_mut() else {
            return;
        };
        session.on_words(words);
        for event in session.drain_events() {
            let _ = app.emit(EVENT_CHANNEL, &event);
        }
        judge.map(|_| (session.next_judge_window(), session.hold()))
    };
    let Some(judge) = judge else {
        return;
    };
    let Some((Some(window), delay)) = pending else {
        return;
    };
    let mut sched = match judge.sched.lock() {
        Ok(sched) => sched,
        Err(_) => return,
    };
    if !sched.due(count, now) {
        return;
    }
    let generation = sched.begin(now);
    drop(sched);
    let _ = judge.tx.send(JudgeJob {
        generation,
        window,
        delay,
        started: std::time::Instant::now(),
    });
}

fn report_recognizer_failure(app: &AppHandle, slot: &Mutex<Option<Session>>, detail: &str) {
    if let Ok(mut guard) = slot.lock()
        && let Some(session) = guard.as_mut()
    {
        session.note_recognizer(false);
        for event in session.drain_events() {
            let _ = app.emit(EVENT_CHANNEL, &event);
        }
    }
    let _ = app.emit(
        EVENT_CHANNEL,
        &UiEvent::AnalysisStall {
            active: true,
            fallback: StallFallback::Warn,
            message: format!("Speech recognition could not run. {detail}"),
        },
    );
}

struct JudgeJob {
    generation: u64,
    window: JudgeWindow,
    delay: Dur,
    /// Wall clock for the latency sample only. Redactions stay on media time.
    started: std::time::Instant,
}

struct JudgeHook {
    sched: Arc<Mutex<JudgeScheduler>>,
    tx: std::sync::mpsc::Sender<JudgeJob>,
}

enum LiveJudge {
    Local(OllamaJudge),
    Cloud(AnthropicJudge),
}

impl LiveJudge {
    fn judge(
        &self,
        req: &JudgeRequest<'_>,
    ) -> Result<Vec<airlock_core::Detection>, airlock_llm::JudgeError> {
        match self {
            Self::Local(judge) => judge.judge(req),
            Self::Cloud(judge) => judge.judge(req),
        }
    }
}

struct JudgeTarget {
    platform: Platform,
    also: Vec<Platform>,
    rules: Vec<String>,
}

fn spawn_judge(
    app: AppHandle,
    slot: Arc<Mutex<Option<Session>>>,
    generation: Arc<AtomicU64>,
    mine: u64,
    target: JudgeTarget,
    cloud: bool,
) -> JudgeHook {
    let (tx, rx) = std::sync::mpsc::channel();
    let sched = Arc::new(Mutex::new(JudgeScheduler::default()));
    let sched_loop = Arc::clone(&sched);
    let JudgeTarget {
        platform,
        also,
        rules,
    } = target;
    std::thread::spawn(move || {
        judge_loop(
            JudgeLoop {
                app,
                slot,
                generation,
                mine,
                platform,
                also,
                rules,
                cloud,
                sched: sched_loop,
            },
            rx,
        );
    });
    JudgeHook { sched, tx }
}

struct JudgeLoop {
    app: AppHandle,
    slot: Arc<Mutex<Option<Session>>>,
    generation: Arc<AtomicU64>,
    mine: u64,
    platform: Platform,
    also: Vec<Platform>,
    rules: Vec<String>,
    cloud: bool,
    sched: Arc<Mutex<JudgeScheduler>>,
}

fn judge_loop(ctx: JudgeLoop, rx: std::sync::mpsc::Receiver<JudgeJob>) {
    let JudgeLoop {
        app,
        slot,
        generation,
        mine,
        platform,
        also,
        rules,
        cloud,
        sched,
    } = ctx;
    let judge = if cloud {
        match crate::keys::load_cloud_key() {
            Ok(key) => LiveJudge::Cloud(AnthropicJudge::new(key, true)),
            Err(_) => LiveJudge::Local(OllamaJudge::default()),
        }
    } else {
        LiveJudge::Local(OllamaJudge::default())
    };
    while generation.load(Ordering::SeqCst) == mine {
        let Ok(job) = rx.recv() else {
            break;
        };
        if generation.load(Ordering::SeqCst) != mine {
            break;
        }
        let req = JudgeRequest {
            words: &job.window.words,
            focus_start: job.window.focus_start,
            platform,
            also: &also,
            delay: job.delay,
            streamer_rules: &rules,
        };
        let result = judge.judge(&req);
        let elapsed = i64::try_from(job.started.elapsed().as_micros()).unwrap_or(i64::MAX);
        let fresh = sched
            .lock()
            .map(|mut sched| sched.complete(job.generation, elapsed))
            .unwrap_or(false);
        let Ok(detections) = result else {
            continue;
        };
        if !fresh {
            continue;
        }
        let Ok(mut guard) = slot.lock() else {
            continue;
        };
        let Some(session) = guard.as_mut() else {
            continue;
        };
        session.on_judge_result(&job.window, detections);
        for event in session.drain_events() {
            let _ = app.emit(EVENT_CHANNEL, &event);
        }
    }
}

fn waiting_detail(
    ready: &[String],
    missing: &[String],
    limited: bool,
    fallback: Platform,
) -> String {
    if ready.is_empty() {
        let who = if missing.is_empty() {
            platform_name(fallback).to_string()
        } else {
            destination_sentence(missing)
        };
        return format!(
            "Save a stream key in Settings. This broadcast stays on this PC and is not sent to {who}."
        );
    }
    let mut text = format!("Waiting to forward to {}.", destination_sentence(ready));
    if !missing.is_empty() {
        let verb = if missing.len() == 1 { "has" } else { "have" };
        text.push_str(&format!(
            " {} {verb} no stream key saved.",
            destination_sentence(missing)
        ));
    }
    if limited {
        text.push_str(" A second destination needs Pro.");
    }
    text
}

fn forward_detail(connected: &[String], failed: &[String]) -> String {
    if connected.is_empty() {
        if failed.is_empty() {
            "Forwarding stopped.".into()
        } else {
            failed.join(" ")
        }
    } else {
        let mut text = format!("Forwarding to {}.", destination_sentence(connected));
        if !failed.is_empty() {
            text.push(' ');
            text.push_str(&failed.join(" "));
        }
        text
    }
}

fn destination_sentence(names: &[String]) -> String {
    match names.len() {
        0 => "the platform".into(),
        1 => names[0].clone(),
        2 => format!("{} and {}", names[0], names[1]),
        _ => {
            let last = names.last().map(String::as_str).unwrap_or("the platform");
            format!("{}, and {last}", names[..names.len() - 1].join(", "))
        }
    }
}

fn publish_target(platform: Platform) -> Option<String> {
    let base = publish_base(platform)?;
    let key = crate::keys::load_stream_key(key_account(platform)).ok()?;
    let key = key.trim();
    if key.is_empty() {
        return None;
    }
    Some(secure_publish_url(base, key))
}

struct KeptFrame {
    ts: Ts,
    width: u32,
    height: u32,
    bgra: Vec<u8>,
    signed: Vec<u64>,
}

fn watch_screen(
    app: AppHandle,
    slot: Arc<Mutex<Option<Session>>>,
    generation: Arc<AtomicU64>,
    mine: u64,
    delay: Dur,
    covers: Arc<Mutex<Vec<FrameCover>>>,
    rx: std::sync::mpsc::Receiver<LiveFrame>,
) {
    let mut sampler = FrameSampler::new(4.0);
    let mut ocr = WindowsOcr;
    let mut kept = Vec::<KeptFrame>::new();
    let scene_busy = Arc::new(std::sync::atomic::AtomicBool::new(false));
    loop {
        if generation.load(Ordering::SeqCst) != mine {
            break;
        }
        match rx.recv_timeout(std::time::Duration::from_millis(200)) {
            Ok(frame) => {
                if let Some(span) = sampler.offer(frame.ts) {
                    let view = FrameRef {
                        ts: frame.ts,
                        width: frame.width,
                        height: frame.height,
                        stride: frame.width.saturating_mul(4),
                        format: PixelFormat::Bgra8,
                        data: &frame.bgra,
                    };
                    let boxes = downscale_bgra(&view, 960)
                        .ok()
                        .and_then(|sampled| {
                            let small = FrameRef {
                                ts: frame.ts,
                                width: sampled.width,
                                height: sampled.height,
                                stride: sampled.width.saturating_mul(4),
                                format: PixelFormat::Bgra8,
                                data: &sampled.data,
                            };
                            ocr.recognize(&small).ok()
                        })
                        .unwrap_or_default();
                    if let Ok(mut guard) = slot.lock()
                        && let Some(session) = guard.as_mut()
                    {
                        session.on_screen_text(&boxes, span);
                        for event in session.drain_events() {
                            let _ = app.emit(EVENT_CHANNEL, &event);
                        }
                    }
                    kept.push(KeptFrame {
                        ts: frame.ts,
                        width: frame.width,
                        height: frame.height,
                        bgra: frame.bgra.clone(),
                        signed: Vec::new(),
                    });
                    offer_scene(
                        &ScreenHook {
                            app: app.clone(),
                            slot: Arc::clone(&slot),
                            generation: Arc::clone(&generation),
                            mine,
                            delay,
                        },
                        &scene_busy,
                        &frame,
                        span,
                    );
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
        refresh_covers(&slot, &mut kept, &covers, delay);
        if kept.len() > 48 {
            let drop_n = kept.len() - 48;
            kept.drain(..drop_n);
        }
    }
}

struct ScreenHook {
    app: AppHandle,
    slot: Arc<Mutex<Option<Session>>>,
    generation: Arc<AtomicU64>,
    mine: u64,
    delay: Dur,
}

fn offer_scene(
    hook: &ScreenHook,
    busy: &Arc<std::sync::atomic::AtomicBool>,
    frame: &LiveFrame,
    span: Span,
) {
    if busy
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
        .is_err()
    {
        return;
    }
    let Ok(png) = bgra_png(frame.width, frame.height, &frame.bgra) else {
        busy.store(false, Ordering::Release);
        return;
    };
    let app = hook.app.clone();
    let slot = Arc::clone(&hook.slot);
    let generation = Arc::clone(&hook.generation);
    let mine = hook.mine;
    let delay = hook.delay;
    let busy = Arc::clone(busy);
    std::thread::spawn(move || {
        let finding = OllamaScene::default().classify(&png, scene_deadline(delay));
        if generation.load(Ordering::SeqCst) == mine
            && let Ok(Some(finding)) = finding
            && let Ok(mut guard) = slot.lock()
            && let Some(session) = guard.as_mut()
        {
            session.on_scene(
                span,
                finding.region,
                finding.confidence,
                finding.label.reason(),
            );
            for event in session.drain_events() {
                let _ = app.emit(EVENT_CHANNEL, &event);
            }
        }
        busy.store(false, Ordering::Release);
    });
}

fn refresh_covers(
    slot: &Arc<Mutex<Option<Session>>>,
    kept: &mut [KeptFrame],
    covers: &Arc<Mutex<Vec<FrameCover>>>,
    delay: Dur,
) {
    let Ok(guard) = slot.lock() else {
        return;
    };
    let Some(session) = guard.as_ref() else {
        return;
    };
    let redactions = session.engine().all().to_vec();
    drop(guard);
    let mut fresh = Vec::new();
    for frame in kept.iter_mut() {
        let ids = matching_ids(&redactions, frame.ts);
        if ids.is_empty() || ids == frame.signed {
            continue;
        }
        let (regions, blackouts) = painted_regions(&redactions, frame.ts);
        let span = redactions
            .iter()
            .filter(|redaction| ids.contains(&redaction.id))
            .fold(Span::new(frame.ts, frame.ts), |span, redaction| {
                span.union(&redaction.span)
            });
        frame.signed = ids;
        if let Ok(cover) = cover_frame(
            &frame.bgra,
            frame.width,
            frame.height,
            &regions,
            &blackouts,
            span,
        ) {
            fresh.push(cover);
        }
    }
    let Some(newest) = kept.iter().map(|frame| frame.ts.0).max() else {
        return;
    };
    let keep_after = newest.saturating_sub(delay.0.saturating_add(2_000_000));
    if let Ok(mut covers) = covers.lock() {
        for cover in fresh {
            if let Some(found) = covers.iter_mut().find(|item| item.span == cover.span) {
                *found = cover;
            } else {
                covers.push(cover);
            }
        }
        covers.retain(|cover| cover.span.end.0 >= keep_after);
    }
}

fn matching_ids(redactions: &[airlock_core::Redaction], ts: Ts) -> Vec<u64> {
    let mut ids: Vec<u64> = redactions
        .iter()
        .filter(|redaction| visual_hit(redaction, ts))
        .map(|redaction| redaction.id)
        .collect();
    ids.sort_unstable();
    ids
}

fn painted_regions(redactions: &[airlock_core::Redaction], ts: Ts) -> (Vec<Rect>, Vec<Rect>) {
    let mut regions = Vec::new();
    let mut blackouts = Vec::new();
    for redaction in redactions
        .iter()
        .filter(|redaction| visual_hit(redaction, ts))
    {
        let Target::Video { region } = redaction.target else {
            continue;
        };
        if redaction.action == Action::Blackout {
            blackouts.push(region);
        } else {
            regions.push(region);
        }
    }
    (regions, blackouts)
}

fn visual_hit(redaction: &airlock_core::Redaction, ts: Ts) -> bool {
    redaction.is_active()
        && !redaction.is_audio()
        && redaction.span.contains(ts)
        && matches!(
            redaction.action,
            Action::Blur | Action::Pixelate | Action::Blackout
        )
}

fn write_wav(path: &Path, samples: &[f32]) -> std::io::Result<()> {
    use std::io::Write;
    if samples.is_empty() {
        return Ok(());
    }
    let mut file = std::fs::File::create(path)?;
    let data_len = (samples.len() * 4) as u32;
    let channels = 2u16;
    let rate = 48_000u32;
    file.write_all(b"RIFF")?;
    file.write_all(&(36 + data_len).to_le_bytes())?;
    file.write_all(b"WAVEfmt ")?;
    file.write_all(&16u32.to_le_bytes())?;
    file.write_all(&3u16.to_le_bytes())?;
    file.write_all(&channels.to_le_bytes())?;
    file.write_all(&rate.to_le_bytes())?;
    file.write_all(&(rate * u32::from(channels) * 4).to_le_bytes())?;
    file.write_all(&(channels * 4).to_le_bytes())?;
    file.write_all(&32u16.to_le_bytes())?;
    file.write_all(b"data")?;
    file.write_all(&data_len.to_le_bytes())?;
    for sample in samples {
        file.write_all(&sample.to_le_bytes())?;
    }
    Ok(())
}
