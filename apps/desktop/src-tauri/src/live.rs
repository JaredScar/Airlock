//! Go live: listen for one local publisher, push its audio through the delay,
//! and run Whisper on a 16 kHz copy when the speech model is on disk.

use crate::EVENT_CHANNEL;
use airlock_asr::audio::{downmix_to_mono, has_speech_energy, resample_linear};
use airlock_asr::{ASR_SAMPLE_RATE, SpeechRecognizer, WhisperConfig, WhisperRecognizer};
use airlock_core::{Dur, Platform, StallFallback, Ts};
use airlock_media::{
    AudioHold, DelayedForward, ForwardStep, INGEST_PORT, VideoHold, key_account, platform_name,
    publish_base, secure_publish_url, serve_capture,
};
use airlock_pipeline::{Session, UiEvent};
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
    let forward_url = publish_target(platform);
    let dir = app.path().app_data_dir().map_err(|err| err.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
    std::thread::spawn(move || {
        if generation.load(Ordering::SeqCst) != mine {
            return;
        }
        let model = airlock_asr::runtime_speech_model(crate::detect_vram_mb());
        let model_path = dir.join("models").join(model.file_name());
        let model_ready = airlock_asr::file_matches(&model_path, model.sha256());
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
            std::thread::spawn(move || {
                recognize(app_asr, slot_asr, generation_asr, mine, model_path, rx);
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
        let forwarding = forward_url.is_some();
        let _ = app.emit(EVENT_CHANNEL, &UiEvent::Ingest { connected: false });
        let _ = app.emit(
            EVENT_CHANNEL,
            &UiEvent::Egress {
                active: false,
                detail: if forwarding {
                    format!("Waiting to forward to {}.", platform_name(platform))
                } else {
                    format!(
                        "Save a stream key in Settings. This broadcast stays on this PC and is not sent to {}.",
                        platform_name(platform)
                    )
                },
            },
        );
        let delayed = Arc::new(Mutex::new(Vec::<f32>::new()));
        let media = Arc::new(Mutex::new(LiveMedia {
            audio: AudioHold::new(delay),
            video: VideoHold::new(delay),
            forward: DelayedForward::new(forward_url),
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
        let _ = serve_capture(
            INGEST_PORT,
            &dir,
            &stop,
            {
                let app = app.clone();
                move |connected| {
                    let _ = app.emit(EVENT_CHANNEL, &UiEvent::Ingest { connected });
                }
            },
            move |packet| {
                if let Ok(mut media) = media_video.lock() {
                    media.video.push(packet);
                }
            },
            move |pcm| {
                if generation.load(Ordering::SeqCst) != mine {
                    return;
                }
                if let Some(tx) = &speech_out {
                    let mono = downmix_to_mono(&pcm.samples, pcm.channels.max(1));
                    let speech = resample_linear(&mono, pcm.sample_rate, ASR_SAMPLE_RATE);
                    let _ = tx.send((pcm.start, speech));
                }
                let end = pcm.start
                    + Dur::from_secs_f64(pcm.frames() as f64 / pcm.sample_rate.max(1) as f64);
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
                if let Ok(mut delayed) = delayed_pcm.lock() {
                    delayed.extend_from_slice(&samples);
                }
                match media.forward.write(&pictures, &samples) {
                    Ok(ForwardStep::Started) => {
                        let _ = app_pcm.emit(
                            EVENT_CHANNEL,
                            &UiEvent::Egress {
                                active: true,
                                detail: format!("Forwarding to {}.", platform_name(platform)),
                            },
                        );
                    }
                    Err(detail) => {
                        let _ = app_pcm.emit(
                            EVENT_CHANNEL,
                            &UiEvent::Egress {
                                active: false,
                                detail,
                            },
                        );
                    }
                    Ok(_) => {}
                }
                for event in session.drain_events() {
                    let _ = app_pcm.emit(EVENT_CHANNEL, &event);
                }
            },
        );
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
                    detail: format!("Forwarding to {} stopped.", platform_name(platform)),
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
}

fn recognize(
    app: AppHandle,
    slot: Arc<Mutex<Option<Session>>>,
    generation: Arc<AtomicU64>,
    mine: u64,
    model_path: std::path::PathBuf,
    rx: std::sync::mpsc::Receiver<(Ts, Vec<f32>)>,
) {
    // This build has no CUDA toolkit, so the model runs on CPU.
    let config = WhisperConfig {
        model_path,
        use_gpu: false,
        ..WhisperConfig::default()
    };
    let mut recognizer = match WhisperRecognizer::new(config) {
        Ok(recognizer) => recognizer,
        Err(err) => {
            report_recognizer_failure(&app, &slot, &err.to_string());
            return;
        }
    };
    while generation.load(Ordering::SeqCst) == mine {
        let Ok((start, samples)) = rx.recv() else {
            break;
        };
        if generation.load(Ordering::SeqCst) != mine {
            break;
        }
        match recognizer.push(&samples, start) {
            Ok(words) => deliver_words(&app, &slot, words),
            Err(err) => {
                report_recognizer_failure(&app, &slot, &err.to_string());
                return;
            }
        }
    }
    if let Ok(words) = recognizer.flush() {
        deliver_words(&app, &slot, words);
    }
}

fn deliver_words(app: &AppHandle, slot: &Mutex<Option<Session>>, words: Vec<airlock_core::Word>) {
    if words.is_empty() {
        return;
    }
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

fn publish_target(platform: Platform) -> Option<String> {
    let base = publish_base(platform)?;
    let key = crate::keys::load_stream_key(key_account(platform)).ok()?;
    let key = key.trim();
    if key.is_empty() {
        return None;
    }
    Some(secure_publish_url(base, key))
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
