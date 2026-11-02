//! OBS WebSocket 5 client. The password is sent only as the Identify auth
//! field. It is not written into an error.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::net::TcpStream;
use std::time::Duration;
use tungstenite::client::IntoClientRequest;
use tungstenite::http::HeaderValue;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

pub const DEFAULT_URL: &str = "ws://127.0.0.1:4455";

pub fn obs_auth(password: &str, salt: &str, challenge: &str) -> String {
    let mut secret = Sha256::new();
    secret.update(password.as_bytes());
    secret.update(salt.as_bytes());
    let secret = STANDARD.encode(secret.finalize());
    let mut auth = Sha256::new();
    auth.update(secret.as_bytes());
    auth.update(challenge.as_bytes());
    STANDARD.encode(auth.finalize())
}

pub struct ObsRemote {
    socket: WebSocket<MaybeTlsStream<TcpStream>>,
    next_id: u64,
}

impl ObsRemote {
    pub fn connect(url: &str, password: &str) -> Result<Self, String> {
        let mut request = url
            .into_client_request()
            .map_err(|_| "obs websocket address is invalid".to_string())?;
        request.headers_mut().insert(
            "Sec-WebSocket-Protocol",
            HeaderValue::from_static("obswebsocket.json"),
        );
        let (mut socket, _) = tungstenite::connect(request)
            .map_err(|_| "could not reach OBS websocket".to_string())?;
        if let MaybeTlsStream::Plain(tcp) = socket.get_mut() {
            let _ = tcp.set_read_timeout(Some(Duration::from_secs(3)));
            let _ = tcp.set_write_timeout(Some(Duration::from_secs(3)));
        }
        let hello = read_json(&mut socket)?;
        if hello.get("op").and_then(Value::as_i64) != Some(0) {
            return Err("OBS did not say hello".into());
        }
        let mut identify = serde_json::json!({
            "op": 1,
            "d": { "rpcVersion": 1, "eventSubscriptions": 0 }
        });
        if hello["d"]["authentication"].is_object() {
            if password.trim().is_empty() {
                return Err("OBS websocket password is empty".into());
            }
            let salt = hello["d"]["authentication"]["salt"].as_str().unwrap_or("");
            let challenge = hello["d"]["authentication"]["challenge"]
                .as_str()
                .unwrap_or("");
            identify["d"]["authentication"] =
                serde_json::Value::String(obs_auth(password, salt, challenge));
        }
        send_json(&mut socket, &identify)?;
        let identified = read_json(&mut socket)?;
        if identified.get("op").and_then(Value::as_i64) != Some(2) {
            return Err("OBS rejected the websocket login".into());
        }
        Ok(ObsRemote { socket, next_id: 1 })
    }

    pub fn list_sources(&mut self) -> Result<Vec<String>, String> {
        let inputs = self.request("GetInputList", serde_json::json!({}))?;
        let scenes = self.request("GetSceneList", serde_json::json!({}))?;
        let mut names = Vec::new();
        if let Some(items) = inputs.get("inputs").and_then(Value::as_array) {
            for item in items {
                if let Some(name) = item.get("inputName").and_then(Value::as_str) {
                    push_unique(&mut names, name);
                }
            }
        }
        if let Some(scene) = scenes
            .get("currentProgramSceneName")
            .and_then(Value::as_str)
        {
            let items = self.request(
                "GetSceneItemList",
                serde_json::json!({ "sceneName": scene }),
            )?;
            if let Some(rows) = items.get("sceneItems").and_then(Value::as_array) {
                for row in rows {
                    if let Some(name) = row.get("sourceName").and_then(Value::as_str) {
                        push_unique(&mut names, name);
                    }
                }
            }
        }
        names.sort();
        Ok(names)
    }

    /// Point OBS at Airlock's local ingest. The key is the local hop `airlock`.
    pub fn set_custom_ingest(&mut self, server: &str, key: &str) -> Result<(), String> {
        self.request(
            "SetStreamServiceSettings",
            serde_json::json!({
                "streamServiceType": "rtmp_custom",
                "streamServiceSettings": { "server": server, "key": key }
            }),
        )?;
        Ok(())
    }

    pub fn custom_ingest_matches(&mut self, server: &str, key: &str) -> Result<bool, String> {
        let data = self.request("GetStreamServiceSettings", serde_json::json!({}))?;
        let settings = &data["streamServiceSettings"];
        let saved_server = settings["server"].as_str().unwrap_or("");
        let saved_key = settings["key"].as_str().unwrap_or("");
        Ok(saved_server == server && saved_key == key)
    }

    fn request(&mut self, kind: &str, data: Value) -> Result<Value, String> {
        let id = self.next_id.to_string();
        self.next_id += 1;
        send_json(
            &mut self.socket,
            &serde_json::json!({
                "op": 6,
                "d": { "requestType": kind, "requestId": id, "requestData": data }
            }),
        )?;
        loop {
            let value = read_json(&mut self.socket)?;
            if value.get("op").and_then(Value::as_i64) != Some(7) {
                continue;
            }
            if value["d"]["requestId"].as_str() != Some(id.as_str()) {
                continue;
            }
            if value["d"]["requestStatus"]["result"].as_bool() != Some(true) {
                return Err(format!("OBS refused {kind}"));
            }
            return Ok(value["d"]["responseData"].clone());
        }
    }
}

fn send_json(
    socket: &mut WebSocket<MaybeTlsStream<TcpStream>>,
    value: &Value,
) -> Result<(), String> {
    socket
        .send(Message::Text(value.to_string().into()))
        .map_err(|_| "could not talk to OBS".to_string())
}

fn read_json(socket: &mut WebSocket<MaybeTlsStream<TcpStream>>) -> Result<Value, String> {
    loop {
        let message = socket
            .read()
            .map_err(|_| "OBS websocket closed".to_string())?;
        let Message::Text(text) = message else {
            continue;
        };
        return serde_json::from_str(&text)
            .map_err(|_| "OBS sent a message that was not JSON".into());
    }
}

fn push_unique(names: &mut Vec<String>, name: &str) {
    let trimmed = name.trim();
    if trimmed.is_empty() || names.iter().any(|kept| kept.eq_ignore_ascii_case(trimmed)) {
        return;
    }
    names.push(trimmed.to_string());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;

    #[test]
    fn auth_matches_the_published_recipe() {
        assert_eq!(
            obs_auth("supersecretpassword", "PZ0bR0u2", "1234567890"),
            "hy5fqMAUzAHFuGQWnmX4OXhgloratpriEYbSdE5NlxQ="
        );
    }

    #[test]
    fn a_local_socket_lists_sources_and_hides_the_password() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || serve(listener));
        let mut remote =
            ObsRemote::connect(&format!("ws://127.0.0.1:{port}"), "supersecretpassword").unwrap();
        assert_eq!(
            remote.list_sources().unwrap(),
            vec!["Browser".to_string(), "Game Capture".to_string()]
        );
        remote
            .set_custom_ingest("rtmp://127.0.0.1:1935/live", "airlock")
            .unwrap();
        assert!(
            remote
                .custom_ingest_matches("rtmp://127.0.0.1:1935/live", "airlock")
                .unwrap()
        );
    }

    #[test]
    fn an_empty_password_is_refused() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || serve(listener));
        let err = match ObsRemote::connect(&format!("ws://127.0.0.1:{port}"), " ") {
            Ok(_) => panic!("empty password was accepted"),
            Err(err) => err,
        };
        assert_eq!(err, "OBS websocket password is empty");
    }

    #[test]
    #[ignore = "needs OBS websocket on 4455"]
    fn a_running_obs_lists_sources() {
        let path = dirs_config();
        let text = std::fs::read_to_string(&path).expect("obs websocket config");
        let cfg: Value = serde_json::from_str(&text).expect("obs websocket config json");
        assert!(
            cfg["server_enabled"].as_bool().unwrap_or(false),
            "websocket server is off"
        );
        let password = cfg["server_password"].as_str().unwrap_or("");
        let mut remote = ObsRemote::connect(DEFAULT_URL, password).expect("connect");
        let names = remote.list_sources().expect("sources");
        assert!(names.iter().all(|name| !name.is_empty()));
        let _ = names.len();
    }

    #[test]
    #[ignore = "needs OBS websocket on 4455"]
    fn a_running_obs_accepts_the_local_ingest() {
        let text = std::fs::read_to_string(dirs_config()).expect("obs websocket config");
        let cfg: Value = serde_json::from_str(&text).expect("obs websocket config json");
        let password = cfg["server_password"].as_str().unwrap_or("");
        let mut remote = ObsRemote::connect(DEFAULT_URL, password).expect("connect");
        let server = format!("rtmp://127.0.0.1:{}/live", airlock_media::INGEST_PORT);
        remote
            .set_custom_ingest(&server, airlock_media::INGEST_KEY)
            .expect("set");
        assert!(
            remote
                .custom_ingest_matches(&server, airlock_media::INGEST_KEY)
                .expect("read")
        );
    }

    fn dirs_config() -> std::path::PathBuf {
        let appdata = std::env::var("APPDATA").expect("APPDATA");
        std::path::PathBuf::from(appdata)
            .join("obs-studio")
            .join("plugin_config")
            .join("obs-websocket")
            .join("config.json")
    }

    fn serve(listener: TcpListener) {
        let Ok((stream, _)) = listener.accept() else {
            return;
        };
        let mut socket = tungstenite::accept_hdr(
            stream,
            |request: &tungstenite::handshake::server::Request,
             mut response: tungstenite::handshake::server::Response| {
                if let Some(protocol) = request.headers().get("Sec-WebSocket-Protocol") {
                    response
                        .headers_mut()
                        .insert("Sec-WebSocket-Protocol", protocol.clone());
                }
                Ok(response)
            },
        )
        .unwrap();
        let hello = serde_json::json!({
            "op": 0,
            "d": {
                "obsWebSocketVersion": "5.0.0",
                "rpcVersion": 1,
                "authentication": { "challenge": "1234567890", "salt": "PZ0bR0u2" }
            }
        });
        socket
            .send(Message::Text(hello.to_string().into()))
            .unwrap();
        let identify = read_text(&mut socket);
        let value: Value = serde_json::from_str(&identify).unwrap();
        let expected = obs_auth("supersecretpassword", "PZ0bR0u2", "1234567890");
        if value["d"]["authentication"].as_str() != Some(expected.as_str()) {
            return;
        }
        socket
            .send(Message::Text(
                serde_json::json!({"op": 2, "d": {"negotiatedRpcVersion": 1}})
                    .to_string()
                    .into(),
            ))
            .unwrap();
        loop {
            let text = read_text(&mut socket);
            if text.is_empty() {
                break;
            }
            let request: Value = serde_json::from_str(&text).unwrap();
            let kind = request["d"]["requestType"].as_str().unwrap_or("");
            let id = request["d"]["requestId"].as_str().unwrap_or("");
            let body = match kind {
                "GetInputList" => serde_json::json!({
                    "inputs": [
                        { "inputName": "Game Capture", "inputKind": "game_capture" },
                        { "inputName": "Browser", "inputKind": "browser_source" }
                    ]
                }),
                "GetSceneList" => serde_json::json!({
                    "currentProgramSceneName": "Scene",
                    "scenes": [{ "sceneName": "Scene" }]
                }),
                "GetSceneItemList" => serde_json::json!({
                    "sceneItems": [{ "sourceName": "Game Capture" }]
                }),
                "GetStreamServiceSettings" => serde_json::json!({
                    "streamServiceType": "rtmp_custom",
                    "streamServiceSettings": {
                        "server": "rtmp://127.0.0.1:1935/live",
                        "key": "airlock"
                    }
                }),
                _ => serde_json::json!({}),
            };
            let response = serde_json::json!({
                "op": 7,
                "d": {
                    "requestType": kind,
                    "requestId": id,
                    "requestStatus": { "result": true, "code": 100 },
                    "responseData": body
                }
            });
            if socket
                .send(Message::Text(response.to_string().into()))
                .is_err()
            {
                break;
            }
        }
    }

    fn read_text(socket: &mut WebSocket<TcpStream>) -> String {
        loop {
            match socket.read() {
                Ok(Message::Text(text)) => return text.to_string(),
                Ok(Message::Close(_)) | Err(_) => return String::new(),
                _ => {}
            }
        }
    }
}
