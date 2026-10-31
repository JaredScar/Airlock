//! Picks a Whisper ggml file from detected VRAM, downloads it, and checks its bytes.
//! Tests talk to a local HTTP server. They do not fetch the public model files.

use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::Path;

use crate::AsrError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeechModel {
    /// CPU fallback.
    SmallEn,
    DistilLargeV3,
    LargeV3Turbo,
}

impl SpeechModel {
    pub fn file_name(self) -> &'static str {
        match self {
            SpeechModel::SmallEn => "ggml-small.en.bin",
            SpeechModel::DistilLargeV3 => "ggml-distil-large-v3.bin",
            SpeechModel::LargeV3Turbo => "ggml-large-v3-turbo.bin",
        }
    }

    /// Public ggml file. The hash is the Hugging Face LFS id for that file.
    pub fn url(self) -> &'static str {
        match self {
            SpeechModel::SmallEn => {
                "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.en.bin"
            }
            SpeechModel::DistilLargeV3 => {
                "https://huggingface.co/distil-whisper/distil-large-v3-ggml/resolve/main/ggml-distil-large-v3.bin"
            }
            SpeechModel::LargeV3Turbo => {
                "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin"
            }
        }
    }

    pub fn sha256(self) -> &'static str {
        match self {
            SpeechModel::SmallEn => {
                "c6138d6d58ecc8322097e0f987c32f1be8bb0a18532a3f88f734d1bbf9c41e5d"
            }
            SpeechModel::DistilLargeV3 => {
                "2883a11b90fb10ed592d826edeaee7d2929bf1ab985109fe9e1e7b4d2b69a298"
            }
            SpeechModel::LargeV3Turbo => {
                "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69"
            }
        }
    }
}

/// True when the file's SHA-256 matches. A missing file is a mismatch, not an error.
pub fn file_matches(path: &Path, expected_hex: &str) -> bool {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(_) => return false,
    };
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => hasher.update(&buf[..n]),
            Err(_) => return false,
        }
    }
    let hex = format!("{:x}", hasher.finalize());
    hex.eq_ignore_ascii_case(expected_hex.trim())
}

/// Downloads `url` and keeps `dest` only when the checksum matches.
/// `progress` receives bytes received and the content length when the server sent one.
pub fn download_verified(
    url: &str,
    dest: &Path,
    expected_sha256: &str,
    mut progress: impl FnMut(u64, Option<u64>),
) -> Result<(), AsrError> {
    let response = ureq::get(url)
        .call()
        .map_err(|err| AsrError::ModelLoad(err.to_string()))?;
    let total = response
        .header("Content-Length")
        .and_then(|value| value.parse().ok());
    let mut reader = response.into_reader();
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|err| AsrError::ModelLoad(err.to_string()))?;
    }
    let tmp = dest.with_extension("partial");
    let mut file = File::create(&tmp).map_err(|err| AsrError::ModelLoad(err.to_string()))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    let mut received = 0u64;
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|err| AsrError::ModelLoad(err.to_string()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        file.write_all(&buf[..n])
            .map_err(|err| AsrError::ModelLoad(err.to_string()))?;
        received += n as u64;
        progress(received, total);
    }
    drop(file);
    let hex = format!("{:x}", hasher.finalize());
    if !hex.eq_ignore_ascii_case(expected_sha256.trim()) {
        let _ = fs::remove_file(&tmp);
        return Err(AsrError::ModelLoad("checksum did not match".into()));
    }
    if dest.exists() {
        let _ = fs::remove_file(dest);
    }
    fs::rename(&tmp, dest).map_err(|err| AsrError::ModelLoad(err.to_string()))?;
    Ok(())
}

/// Model this build can actually run. A CPU build stays on `small.en` even when
/// the GPU has room for a larger file, because that file would not keep up.
pub fn runtime_speech_model(vram_mb: Option<u64>) -> SpeechModel {
    if cfg!(feature = "cuda") {
        pick_speech_model(vram_mb)
    } else {
        SpeechModel::SmallEn
    }
}

/// `vram_mb` is NVIDIA total memory. `None` means no GPU was detected.
pub fn pick_speech_model(vram_mb: Option<u64>) -> SpeechModel {
    match vram_mb {
        Some(mb) if mb >= 8_000 => SpeechModel::LargeV3Turbo,
        Some(mb) if mb >= 4_000 => SpeechModel::DistilLargeV3,
        _ => SpeechModel::SmallEn,
    }
}

/// First GPU from `nvidia-smi --query-gpu=memory.total --format=csv,noheader,nounits`.
pub fn parse_vram_mb(text: &str) -> Option<u64> {
    text.lines().next()?.trim().parse().ok()
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn verify_sha256(bytes: &[u8], expected_hex: &str) -> bool {
    sha256_hex(bytes).eq_ignore_ascii_case(expected_hex.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpu_size_picks_the_model_and_the_hash_rejects_a_swap() {
        assert_eq!(pick_speech_model(None), SpeechModel::SmallEn);
        assert_eq!(pick_speech_model(Some(2_000)), SpeechModel::SmallEn);
        assert_eq!(pick_speech_model(Some(6_000)), SpeechModel::DistilLargeV3);
        assert_eq!(pick_speech_model(Some(12_000)), SpeechModel::LargeV3Turbo);
        if !cfg!(feature = "cuda") {
            assert_eq!(runtime_speech_model(Some(32_000)), SpeechModel::SmallEn);
        }
        assert_eq!(parse_vram_mb("24564 MiB\n"), None);
        assert_eq!(parse_vram_mb("24564\n"), Some(24_564));
        let hash = sha256_hex(b"airlock-model");
        assert!(verify_sha256(b"airlock-model", &hash));
        assert!(!verify_sha256(b"other", &hash));
        for model in [
            SpeechModel::SmallEn,
            SpeechModel::DistilLargeV3,
            SpeechModel::LargeV3Turbo,
        ] {
            assert_eq!(model.sha256().len(), 64);
            assert!(model.url().starts_with("https://"));
            assert!(model.file_name().starts_with("ggml-"));
        }
    }

    #[test]
    fn download_reports_progress_and_drops_a_bad_checksum() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::thread;

        let body = b"airlock-model-bytes";
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut req = [0u8; 1024];
            let _ = sock.read(&mut req);
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            sock.write_all(header.as_bytes()).unwrap();
            sock.write_all(body).unwrap();
        });
        let url = format!("http://127.0.0.1:{port}/model.bin");
        let dir = std::env::temp_dir().join(format!("airlock-model-{}", std::process::id()));
        let dest = dir.join("model.bin");
        let hash = sha256_hex(body);
        let mut ticks = Vec::new();
        download_verified(&url, &dest, &hash, |got, total| ticks.push((got, total))).unwrap();
        assert_eq!(
            ticks.last().copied(),
            Some((body.len() as u64, Some(body.len() as u64)))
        );
        assert!(file_matches(&dest, &hash));
        assert!(!dest.with_extension("partial").exists());

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let body = body.to_vec();
        thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut req = [0u8; 1024];
            let _ = sock.read(&mut req);
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            sock.write_all(header.as_bytes()).unwrap();
            sock.write_all(&body).unwrap();
        });
        let bad = dir.join("bad.bin");
        let err = download_verified(
            &format!("http://127.0.0.1:{port}/bad.bin"),
            &bad,
            "0000000000000000000000000000000000000000000000000000000000000000",
            |_, _| {},
        )
        .unwrap_err();
        assert!(err.to_string().contains("checksum"));
        assert!(!bad.exists());
        assert!(!bad.with_extension("partial").exists());
        let _ = fs::remove_dir_all(dir);
    }
}
