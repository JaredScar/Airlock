//! Go live: listen for one local publisher and push its audio through the delay.
//! Words are not invented here. The word list runs when speech recognition
//! supplies words. Until then, audio with energy and no words uses the stall rule.

use crate::EVENT_CHANNEL;
use airlock_asr::audio::has_speech_energy;
use airlock_core::Dur;
use airlock_media::{AudioHold, INGEST_PORT, serve_capture};
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
    let delay = config.policy.delay();
    let dir = app.path().app_data_dir().map_err(|err| err.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
    std::thread::spawn(move || {
        if generation.load(Ordering::SeqCst) != mine {
            return;
        }
        {
            let mut guard = match slot.lock() {
                Ok(guard) => guard,
                Err(_) => return,
            };
            *guard = Some(Session::new(config));
        }
        let _ = app.emit(EVENT_CHANNEL, &UiEvent::Ingest { connected: false });
        let delayed = Arc::new(Mutex::new(Vec::<f32>::new()));
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
        let slot_pcm = slot.clone();
        let hold_pcm = Arc::new(Mutex::new(AudioHold::new(delay)));
        let delayed_pcm = Arc::clone(&delayed);
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
            move |pcm| {
                if generation.load(Ordering::SeqCst) != mine {
                    return;
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
                let Ok(mut hold) = hold_pcm.lock() else {
                    return;
                };
                let _ = hold.push(pcm);
                session.on_live_time(end);
                session.note_audio_energy(energetic);
                let redactions = session.engine().all().to_vec();
                if let Ok(mut delayed) = delayed_pcm.lock() {
                    delayed.extend(hold.release(end, &redactions));
                }
                for event in session.drain_events() {
                    let _ = app_pcm.emit(EVENT_CHANNEL, &event);
                }
            },
        );
        let delayed = delayed
            .lock()
            .map(|samples| samples.clone())
            .unwrap_or_default();
        let _ = write_wav(&dir.join("delayed.wav"), &delayed);
        let _ = app.emit(EVENT_CHANNEL, &UiEvent::Ingest { connected: false });
    });
    Ok(())
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
