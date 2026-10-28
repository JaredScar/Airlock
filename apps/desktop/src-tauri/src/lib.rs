//! Tauri host. Owns the live `Session` and bridges it to the Angular UI:
//! commands in (approve/veto/policy), `airlock://event` events out.
//!
//! Right now the only session source is `start_demo`, which replays a scripted
//! stream through the real engine so the UI can be built before media ingest exists.
//! PLAN.md Phase 1/2 replaces it with real ingest + ASR threads.

mod demo;

use airlock_core::{Policy, RedactionId};
use airlock_pipeline::{Session, incident};
use std::sync::{Arc, Mutex};
use tauri::State;

pub const EVENT_CHANNEL: &str = "airlock://event";

#[derive(Default)]
pub struct AppState {
    pub session: Arc<Mutex<Option<Session>>>,
}

#[tauri::command]
fn get_default_policy() -> Policy {
    Policy::default()
}

#[tauri::command]
fn start_demo(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    demo::start(app, state.session.clone()).map_err(|e| e.to_string())
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
fn export_incidents_csv(state: State<'_, AppState>) -> Result<String, String> {
    with_session(&state, |s| Ok(incident::to_csv(s.aired())))
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            get_default_policy,
            start_demo,
            approve,
            veto,
            export_incidents_csv
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
