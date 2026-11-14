//! Opens the moderator socket while a session is the current one.
//! The password stays in the keychain. This module does not log it.

use crate::EVENT_CHANNEL;
use airlock_pipeline::{Session, StudioSettings, UiEvent, serve_moderator};
use std::net::{Ipv4Addr, TcpListener};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};
use std::thread;
use tauri::{AppHandle, Emitter};

pub fn start(
    app: &AppHandle,
    slot: Arc<Mutex<Option<Session>>>,
    generation: Arc<AtomicU64>,
    mine: u64,
    settings: &StudioSettings,
) {
    if !settings.moderator_enabled {
        emit(app, false, "");
        return;
    }
    let Ok(token) = crate::keys::load_moderator_password() else {
        emit(
            app,
            false,
            "Save a moderator password in Settings before a moderator can connect.",
        );
        return;
    };
    let port = if settings.moderator_port == 0 {
        1940
    } else {
        settings.moderator_port
    };
    let listener = match bind_port(port) {
        Ok(listener) => listener,
        Err(()) => {
            emit(
                app,
                false,
                &format!("Moderator port {port} is already in use."),
            );
            return;
        }
    };
    emit(
        app,
        true,
        &format!("A moderator can connect on port {port}."),
    );
    thread::spawn(move || serve_moderator(listener, slot, generation, mine, token));
}

fn bind_port(port: u16) -> Result<TcpListener, ()> {
    TcpListener::bind((Ipv4Addr::UNSPECIFIED, port)).map_err(|_| ())
}

fn emit(app: &AppHandle, listening: bool, detail: &str) {
    let _ = app.emit(
        EVENT_CHANNEL,
        &UiEvent::Moderator {
            listening,
            detail: detail.to_string(),
        },
    );
}
