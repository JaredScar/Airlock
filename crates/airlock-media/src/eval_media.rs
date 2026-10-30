//! Builds the v1 clip files the eval labels point at.
//! Speech is synthesized. The screen clip is a drawn frame with a fake key.
//! Recorded speech with a real game behind it is not produced here.

use std::path::Path;
use std::process::{Command, Stdio};

use crate::audio_path::MediaError;

/// Writes short wavs and one screen mp4 into `dir`.
pub fn write_eval_clips(dir: &Path) -> Result<(), MediaError> {
    std::fs::create_dir_all(dir).map_err(|err| MediaError::Ffmpeg(err.to_string()))?;
    speak(
        &dir.join("wordlist-bleep-01.wav"),
        "please bleep the word fuck",
    )?;
    speak(
        &dir.join("spoken-phone-01.wav"),
        "the number is 631 555 0123",
    )?;
    speak(
        &dir.join("tos-threat-01.wav"),
        "I will dox you and post your address",
    )?;
    let screen = dir.join("screen-key-01.mp4");
    let status = Command::new("ffmpeg")
        .args([
            "-y",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=c=black:s=1280x720:d=2",
            "-vf",
            "drawtext=fontfile='C\\:/Windows/Fonts/arial.ttf':text='live_EXAMPLE_KEY':fontcolor=white:fontsize=36:x=800:y=640",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
        ])
        .arg(&screen)
        .stdin(Stdio::null())
        .status()
        .map_err(|_| MediaError::FfmpegMissing)?;
    if status.success() {
        Ok(())
    } else {
        Err(MediaError::Ffmpeg("screen clip failed".into()))
    }
}

fn speak(wav: &Path, text: &str) -> Result<(), MediaError> {
    let script = format!(
        "Add-Type -AssemblyName System.Speech; $s = New-Object System.Speech.Synthesis.SpeechSynthesizer; $s.SetOutputToWaveFile('{}'); $s.Speak('{}'); $s.Dispose()",
        wav.display().to_string().replace('\'', "''"),
        text.replace('\'', "''"),
    );
    let status = Command::new("powershell")
        .args(["-NoProfile", "-Command", &script])
        .stdin(Stdio::null())
        .status()
        .map_err(|_| MediaError::Ffmpeg("speech synthesis is unavailable".into()))?;
    if status.success() && wav.exists() {
        Ok(())
    } else {
        Err(MediaError::Ffmpeg("speech synthesis failed".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_clips_are_nonempty() {
        if !cfg!(windows) {
            return;
        }
        let ffmpeg_ok = Command::new("ffmpeg")
            .arg("-version")
            .output()
            .map(|out| out.status.success())
            .unwrap_or(false);
        if !ffmpeg_ok {
            return;
        }
        let dir = std::env::temp_dir().join(format!("airlock-eval-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        write_eval_clips(&dir).expect("clips");
        for name in [
            "wordlist-bleep-01.wav",
            "spoken-phone-01.wav",
            "tos-threat-01.wav",
            "screen-key-01.mp4",
        ] {
            let len = std::fs::metadata(dir.join(name))
                .map(|m| m.len())
                .unwrap_or(0);
            assert!(len > 1000, "{name} was {len} bytes");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
