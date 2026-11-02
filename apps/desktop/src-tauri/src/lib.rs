//! Tauri host. Owns the live `Session` and bridges it to the Angular UI:
//! commands in (approve/veto/policy), `airlock://event` events out.
//!
//! `start_live` listens for a publisher and feeds the delay. `start_demo` replays
//! a scripted stream and is the dev toggle in the UI.

mod demo;
mod keys;
mod live;
mod moderator;
mod obs;
mod obs_ws;

use airlock_core::{Platform, RedactionId};
use airlock_media::key_account;
use airlock_pipeline::{Feature, Session, StudioSettings, UiEvent, allows, incident};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

pub const EVENT_CHANNEL: &str = "airlock://event";

#[derive(Default)]
pub struct AppState {
    pub session: Arc<Mutex<Option<Session>>>,
    pub studio: Arc<Mutex<StudioSettings>>,
    /// Bumped by Go live, Stop, and Run demo. A worker exits when this changes.
    pub generation: Arc<AtomicU64>,
    /// True while a demo or a live listener is the current session.
    pub listening: Arc<AtomicBool>,
}

#[tauri::command]
fn get_studio_settings(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<StudioSettings, String> {
    let mut settings = state
        .studio
        .lock()
        .map(|g| g.clone())
        .map_err(|_| "settings lock poisoned".to_string())?;
    if let Some(terms) = keys::load_terms(&app)? {
        settings.private_terms = terms;
        let mut guard = state
            .studio
            .lock()
            .map_err(|_| "settings lock poisoned".to_string())?;
        guard.private_terms = settings.private_terms.clone();
    }
    Ok(settings)
}

#[tauri::command]
fn set_studio_settings(
    app: AppHandle,
    settings: StudioSettings,
    state: State<'_, AppState>,
) -> Result<(), String> {
    settings.session_config()?;
    keys::persist_terms(&app, &settings.private_terms)?;
    register_shortcuts(&app, &settings)?;
    let mut guard = state
        .studio
        .lock()
        .map_err(|_| "settings lock poisoned".to_string())?;
    *guard = settings;
    Ok(())
}

#[tauri::command]
fn license_is_pro(key: String) -> bool {
    allows(&key, Feature::ContextJudge)
}

#[tauri::command]
fn set_cloud_key(key: String) -> Result<(), String> {
    keys::store_cloud_key(&key)
}

#[tauri::command]
fn set_obs_password(password: String) -> Result<(), String> {
    keys::store_obs_password(&password)
}

#[tauri::command]
fn set_moderator_password(password: String) -> Result<(), String> {
    keys::store_moderator_password(&password)
}

#[tauri::command]
fn moderator_password_saved() -> Result<bool, String> {
    keys::moderator_password_is_saved()
}

#[tauri::command]
fn obs_password_saved() -> Result<bool, String> {
    keys::obs_password_is_saved()
}

#[derive(serde::Serialize)]
struct ObsSourceList {
    names: Vec<String>,
    targets: Vec<String>,
}

#[tauri::command]
fn list_obs_sources(state: State<'_, AppState>) -> Result<ObsSourceList, String> {
    let password = keys::load_obs_password()?;
    let mut remote = obs_ws::ObsRemote::connect(obs_ws::DEFAULT_URL, &password)?;
    let names = remote.list_sources()?;
    let settings = state
        .studio
        .lock()
        .map_err(|_| "settings lock poisoned".to_string())?;
    let targets =
        airlock_pipeline::screenshot_targets(&names, &settings.scan_always, &settings.scan_never);
    Ok(ObsSourceList { names, targets })
}

#[derive(serde::Serialize)]
struct MachineSetup {
    gpu_mb: Option<u64>,
    model: String,
}

#[tauri::command]
fn machine_setup(app: AppHandle) -> MachineSetup {
    let gpu_mb = detect_vram_mb();
    let whisper = airlock_asr::runtime_speech_model(gpu_mb)
        .file_name()
        .to_string();
    let parakeet = app.path().app_data_dir().ok().is_some_and(|dir| {
        airlock_asr::parakeet_ready(&dir.join("models").join(airlock_asr::PARAKEET_DIR))
    });
    let model = if cfg!(feature = "cuda") && parakeet {
        airlock_asr::PARAKEET_DIR.to_string()
    } else {
        whisper
    };
    MachineSetup { gpu_mb, model }
}

#[tauri::command]
fn configure_obs_ingest() -> Result<(), String> {
    let password = keys::load_obs_password()?;
    let mut remote = obs_ws::ObsRemote::connect(obs_ws::DEFAULT_URL, &password)?;
    let server = format!("rtmp://127.0.0.1:{}/live", airlock_media::INGEST_PORT);
    remote.set_custom_ingest(&server, airlock_media::INGEST_KEY)?;
    if !remote.custom_ingest_matches(&server, airlock_media::INGEST_KEY)? {
        return Err("OBS did not keep the local ingest".into());
    }
    Ok(())
}

#[tauri::command]
fn set_speech_key(key: String) -> Result<(), String> {
    keys::store_speech_key(&key)
}

#[tauri::command]
fn set_stream_key(platform: String, key: String) -> Result<(), String> {
    let platform = parse_platform(&platform)?;
    if platform == Platform::Generic {
        return Err("choose Twitch, YouTube, or Kick".into());
    }
    keys::store_stream_key(key_account(platform), &key)
}

#[tauri::command]
fn has_stream_key(platform: String) -> Result<bool, String> {
    let platform = parse_platform(&platform)?;
    if platform == Platform::Generic {
        return Ok(false);
    }
    keys::stream_key_is_saved(key_account(platform))
}

fn parse_platform(name: &str) -> Result<Platform, String> {
    match name {
        "twitch" => Ok(Platform::Twitch),
        "youtube" => Ok(Platform::Youtube),
        "kick" => Ok(Platform::Kick),
        "generic" => Ok(Platform::Generic),
        _ => Err("unknown platform".into()),
    }
}

#[tauri::command]
fn set_safe_mode(on: bool, state: State<'_, AppState>) -> Result<(), String> {
    with_session(&state, |session| {
        session.set_safe_mode(on);
        Ok(())
    })
}

#[tauri::command]
fn set_compact(on: bool, app: AppHandle) -> Result<(), String> {
    let window = app.get_webview_window("main").ok_or("no main window")?;
    window.set_always_on_top(on).map_err(|e| e.to_string())
}

#[tauri::command]
fn start_demo(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let settings = state
        .studio
        .lock()
        .map_err(|_| "settings lock poisoned".to_string())?
        .clone();
    let mine = state.generation.fetch_add(1, Ordering::SeqCst) + 1;
    demo::start(
        app.clone(),
        state.session.clone(),
        settings.clone(),
        state.generation.clone(),
        mine,
    )
    .map_err(|e| e.to_string())?;
    moderator::start(
        &app,
        state.session.clone(),
        state.generation.clone(),
        mine,
        &settings,
    );
    state.listening.store(true, Ordering::SeqCst);
    Ok(())
}

#[tauri::command]
fn start_live(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    begin_live(&app, &state)
}

fn begin_live(app: &AppHandle, state: &AppState) -> Result<(), String> {
    let ffmpeg_ok = std::process::Command::new("ffmpeg")
        .arg("-version")
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    if !ffmpeg_ok {
        return Err("ffmpeg is not on PATH".into());
    }
    let settings = state
        .studio
        .lock()
        .map_err(|_| "settings lock poisoned".to_string())?
        .clone();
    settings.session_config()?;
    let mine = state.generation.fetch_add(1, Ordering::SeqCst) + 1;
    live::start(
        app.clone(),
        state.session.clone(),
        settings.clone(),
        state.generation.clone(),
        mine,
    )?;
    moderator::start(
        app,
        state.session.clone(),
        state.generation.clone(),
        mine,
        &settings,
    );
    state.listening.store(true, Ordering::SeqCst);
    Ok(())
}

#[tauri::command]
fn stop_live(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    state.listening.store(false, Ordering::SeqCst);
    state.generation.fetch_add(1, Ordering::SeqCst);
    let _ = app.emit(EVENT_CHANNEL, &UiEvent::Ingest { connected: false });
    let _ = app.emit(
        EVENT_CHANNEL,
        &UiEvent::Moderator {
            listening: false,
            detail: String::new(),
        },
    );
    Ok(())
}

#[tauri::command]
fn approve(id: RedactionId, state: State<'_, AppState>) -> Result<(), String> {
    with_session(&state, |s| s.approve(id).map_err(|e| e.to_string()))
}

#[tauri::command]
fn veto(id: RedactionId, state: State<'_, AppState>) -> Result<(), String> {
    with_session(&state, |s| s.veto(id).map_err(|e| e.to_string()))
}

#[tauri::command]
fn export_incidents(format: String, state: State<'_, AppState>) -> Result<String, String> {
    with_session(&state, |s| match format.as_str() {
        "csv" => Ok(incident::to_csv(s.aired())),
        "edl" => Ok(incident::to_edl(s.aired())),
        "youtube" => Ok(incident::to_youtube_timestamps(s.aired())),
        "chapters" => Ok(incident::to_chapters(s.aired())),
        other => Err(format!("unknown incident format: {other}")),
    })
}

#[tauri::command]
fn dump(blackout_video: bool, state: State<'_, AppState>) -> Result<(), String> {
    with_session(&state, |s| {
        s.dump(blackout_video);
        Ok(())
    })
}

fn with_session<T>(
    state: &State<'_, AppState>,
    f: impl FnOnce(&mut Session) -> Result<T, String>,
) -> Result<T, String> {
    let mut guard = state
        .session
        .lock()
        .map_err(|_| "session lock poisoned".to_string())?;
    let session = guard.as_mut().ok_or("no live session")?;
    f(session)
}

fn register_shortcuts(app: &AppHandle, settings: &StudioSettings) -> Result<(), String> {
    let shortcuts = app.global_shortcut();
    let _ = shortcuts.unregister_all();
    for spec in [
        settings.shortcut_veto.as_str(),
        settings.shortcut_approve.as_str(),
        settings.shortcut_dump.as_str(),
        settings.shortcut_safe.as_str(),
    ] {
        if spec.is_empty() {
            continue;
        }
        shortcuts
            .on_shortcut(spec, |app, shortcut, event| {
                if event.state != ShortcutState::Pressed {
                    return;
                }
                let pressed = shortcut.to_string();
                let state = app.state::<AppState>();
                let Ok(settings) = state.studio.lock() else {
                    return;
                };
                let action = if pressed.eq_ignore_ascii_case(&settings.shortcut_veto) {
                    "veto"
                } else if pressed.eq_ignore_ascii_case(&settings.shortcut_approve) {
                    "approve"
                } else if pressed.eq_ignore_ascii_case(&settings.shortcut_dump) {
                    "dump"
                } else if pressed.eq_ignore_ascii_case(&settings.shortcut_safe) {
                    "safe"
                } else {
                    return;
                };
                drop(settings);
                let _ = app.emit("airlock://shortcut", action);
            })
            .map_err(|_| format!("could not register {spec}"))?;
    }
    Ok(())
}

fn install_tray(app: &tauri::App) -> tauri::Result<()> {
    use tauri::menu::{Menu, MenuItem};
    use tauri::tray::TrayIconBuilder;
    let show = MenuItem::with_id(app, "show", "Show Airlock", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;
    let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/32x32.png"))?;
    TrayIconBuilder::new()
        .icon(icon)
        .tooltip("Airlock")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .build(app)?;
    Ok(())
}

/// Polls for OBS. The first time `obs64.exe` is running, and the setting is on,
/// this opens the same listener as Go live. A session that is already listening
/// is left alone. Closing the window hides it; Quit on the tray exits.
fn watch_obs(app: AppHandle) {
    std::thread::spawn(move || {
        let mut was_running = false;
        loop {
            std::thread::sleep(std::time::Duration::from_secs(2));
            let running = obs::obs_is_running();
            let launched = obs::obs_just_launched(was_running, running);
            was_running = running;
            if !launched {
                continue;
            }
            let state = app.state::<AppState>();
            let enabled = state
                .studio
                .lock()
                .map(|settings| settings.start_with_obs)
                .unwrap_or(false);
            if !enabled || state.listening.load(Ordering::SeqCst) {
                continue;
            }
            if begin_live(&app, &state).is_ok() {
                let _ = app.emit("airlock://session-started", ());
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                }
            }
        }
    });
}

#[derive(Clone, serde::Serialize)]
struct ModelProgress {
    received: u64,
    total: Option<u64>,
}

pub(crate) fn detect_vram_mb() -> Option<u64> {
    let output = std::process::Command::new("nvidia-smi")
        .args(["--query-gpu=memory.total", "--format=csv,noheader,nounits"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    airlock_asr::parse_vram_mb(&String::from_utf8_lossy(&output.stdout))
}

/// Saves the speech model under the app data directory.
/// The GPU build downloads Parakeet. A build without CUDA downloads Whisper.
/// A file is kept only after its checksum matches. Progress events are `model-progress`.
#[tauri::command]
fn download_speech_model(app: AppHandle) -> Result<String, String> {
    if cfg!(feature = "cuda") {
        download_parakeet(app)
    } else {
        download_whisper(app)
    }
}

fn download_parakeet(app: AppHandle) -> Result<String, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|err| err.to_string())?
        .join("models")
        .join(airlock_asr::PARAKEET_DIR);
    std::fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
    if airlock_asr::parakeet_ready(&dir) {
        return Ok(airlock_asr::PARAKEET_DIR.to_string());
    }
    let handle = app.clone();
    for file in airlock_asr::parakeet_files() {
        let dest = dir.join(file.name);
        if airlock_asr::file_matches(&dest, file.sha256) {
            continue;
        }
        airlock_asr::download_verified(file.url, &dest, file.sha256, |received, total| {
            let _ = handle.emit("model-progress", ModelProgress { received, total });
        })
        .map_err(|err| err.to_string())?;
    }
    Ok(airlock_asr::PARAKEET_DIR.to_string())
}

fn download_whisper(app: AppHandle) -> Result<String, String> {
    let model = airlock_asr::runtime_speech_model(detect_vram_mb());
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|err| err.to_string())?
        .join("models");
    std::fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
    let dest = dir.join(model.file_name());
    if airlock_asr::file_matches(&dest, model.sha256()) {
        return Ok(model.file_name().to_string());
    }
    let handle = app.clone();
    airlock_asr::download_verified(model.url(), &dest, model.sha256(), |received, total| {
        let _ = handle.emit("model-progress", ModelProgress { received, total });
    })
    .map_err(|err| err.to_string())?;
    Ok(model.file_name().to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .manage(AppState::default())
        .setup(|app| {
            let settings = app
                .state::<AppState>()
                .studio
                .lock()
                .map(|g| g.clone())
                .unwrap_or_default();
            let _ = register_shortcuts(app.handle(), &settings);
            install_tray(app)?;
            watch_obs(app.handle().clone());
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_studio_settings,
            set_studio_settings,
            license_is_pro,
            set_cloud_key,
            set_obs_password,
            obs_password_saved,
            set_moderator_password,
            moderator_password_saved,
            list_obs_sources,
            machine_setup,
            configure_obs_ingest,
            set_speech_key,
            set_stream_key,
            has_stream_key,
            set_safe_mode,
            set_compact,
            start_demo,
            start_live,
            stop_live,
            approve,
            veto,
            dump,
            export_incidents,
            download_speech_model
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
