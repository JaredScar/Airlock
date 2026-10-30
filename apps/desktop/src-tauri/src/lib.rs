//! Tauri host. Owns the live `Session` and bridges it to the Angular UI:
//! commands in (approve/veto/policy), `airlock://event` events out.
//!
//! Right now the only session source is `start_demo`, which replays a scripted
//! stream through the real engine so the UI can be built before media ingest exists.
//! PLAN.md Phase 1/2 replaces it with real ingest + ASR threads.

mod demo;
mod keys;

use airlock_core::RedactionId;
use airlock_pipeline::{Session, StudioSettings, incident};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

pub const EVENT_CHANNEL: &str = "airlock://event";

#[derive(Default)]
pub struct AppState {
    pub session: Arc<Mutex<Option<Session>>>,
    pub studio: Arc<Mutex<StudioSettings>>,
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
fn set_cloud_key(key: String) -> Result<(), String> {
    keys::store_cloud_key(&key)
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
    demo::start(app, state.session.clone(), settings).map_err(|e| e.to_string())
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
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_studio_settings,
            set_studio_settings,
            set_cloud_key,
            set_safe_mode,
            set_compact,
            start_demo,
            approve,
            veto,
            dump,
            export_incidents
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
