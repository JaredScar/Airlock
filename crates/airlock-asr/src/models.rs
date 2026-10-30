//! Picks a Whisper ggml file from detected VRAM and checks its bytes.
//! The download itself is a file copy into the model directory; tests use a tiny buffer.

use sha2::{Digest, Sha256};

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
        assert_eq!(parse_vram_mb("24564 MiB\n"), None);
        assert_eq!(parse_vram_mb("24564\n"), Some(24_564));
        let hash = sha256_hex(b"airlock-model");
        assert!(verify_sha256(b"airlock-model", &hash));
        assert!(!verify_sha256(b"other", &hash));
    }
}
