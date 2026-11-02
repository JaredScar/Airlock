//! Measures task 2.2 on this machine: Parakeet word-finalization latency, and
//! word error against one offline Whisper pass on the same synthesized clips.
//!
//! ```text
//! cargo run -p airlock-asr --example speech_bar --features cuda
//! ```
//!
//! Default `cargo test` does not enable that feature, so this file only prints
//! how to run it.

fn main() {
    #[cfg(all(feature = "parakeet", feature = "whisper"))]
    {
        if let Err(err) = measure() {
            eprintln!("{err}");
            std::process::exit(1);
        }
    }
    #[cfg(not(all(feature = "parakeet", feature = "whisper")))]
    {
        eprintln!("speech_bar needs --features cuda");
        std::process::exit(2);
    }
}

#[cfg(all(feature = "parakeet", feature = "whisper"))]
fn measure() -> Result<(), String> {
    use airlock_asr::{ParakeetRecognizer, WhisperConfig, WhisperRecognizer};
    use std::path::PathBuf;

    let appdata = std::env::var("APPDATA").map_err(|_| "APPDATA is not set".to_string())?;
    let models = PathBuf::from(appdata)
        .join("com.badger.airlock")
        .join("models");
    let parakeet_dir = models.join(airlock_asr::PARAKEET_DIR);
    let whisper_path = models.join("ggml-large-v3-turbo.bin");
    if !airlock_asr::parakeet_ready(&parakeet_dir) {
        return Err(format!(
            "Parakeet model is not ready in {}",
            parakeet_dir.display()
        ));
    }
    if !whisper_path.is_file() {
        return Err(format!("missing {}", whisper_path.display()));
    }

    let dir = std::env::temp_dir().join("airlock-speech-bar");
    std::fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
    let clips = [
        ("wordlist-bleep-01", "please bleep the word fuck"),
        ("spoken-phone-01", "the number is 631 555 0123"),
        ("tos-threat-01", "I will dox you and post your address"),
    ];
    for (name, text) in clips {
        speak(&dir.join(format!("{name}.wav")), text)?;
    }

    eprintln!("loading Parakeet");
    let mut parakeet = ParakeetRecognizer::new(parakeet_dir).map_err(|err| err.to_string())?;
    eprintln!("loading Whisper");
    let mut whisper = WhisperRecognizer::new(WhisperConfig {
        model_path: whisper_path,
        use_gpu: true,
        window_ms: 60_000,
        hop_ms: 60_000,
        ..WhisperConfig::default()
    })
    .map_err(|err| err.to_string())?;

    let mut latencies = Vec::new();
    let mut ref_words = 0usize;
    let mut parakeet_edits = 0usize;
    let mut whisper_edits = 0usize;
    for (index, (name, text)) in clips.into_iter().enumerate() {
        let audio = read_f32(&dir.join(format!("{name}.wav")))?;
        let origin = airlock_core::Ts((index as i64) * 60_000_000);
        let reference = normalize(text);
        let (heard, clip_latencies) = live_parakeet(&mut parakeet, &audio, origin)?;
        let offline = whisper
            .transcribe_clip(&audio)
            .map_err(|err| err.to_string())?;
        let hyp = normalize(&heard.join(" "));
        let off = normalize(
            &offline
                .iter()
                .map(|word| word.text.as_str())
                .collect::<Vec<_>>()
                .join(" "),
        );
        let p_edits = edits(&hyp, &reference);
        let w_edits = edits(&off, &reference);
        ref_words += reference.len();
        parakeet_edits += p_edits;
        whisper_edits += w_edits;
        latencies.extend(clip_latencies.iter().copied());
        println!(
            "{name} ref={} parakeet={} whisper={} edits {p_edits}/{} vs {w_edits}/{}",
            reference.len(),
            hyp.len(),
            off.len(),
            reference.len(),
            reference.len()
        );
        println!("  parakeet: {}", hyp.join(" "));
        println!("  whisper:  {}", off.join(" "));
    }
    latencies.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let p95 = percentile(&latencies, 0.95);
    let p50 = percentile(&latencies, 0.50);
    let parakeet_wer = parakeet_edits as f64 / ref_words.max(1) as f64;
    let whisper_wer = whisper_edits as f64 / ref_words.max(1) as f64;
    let points = (parakeet_wer - whisper_wer).abs() * 100.0;
    println!(
        "words {ref_words} parakeet_wer {parakeet_wer:.4} whisper_wer {whisper_wer:.4} points {points:.2}"
    );
    println!(
        "finalization_ms n {} p50 {:.0} p95 {:.0} max {:.0}",
        latencies.len(),
        p50,
        p95,
        latencies.last().copied().unwrap_or(0.0)
    );
    let pass = p95 <= 1_500.0 && parakeet_wer <= whisper_wer + 0.02;
    println!("bar {}", if pass { "pass" } else { "fail" });
    Ok(())
}

#[cfg(all(feature = "parakeet", feature = "whisper"))]
fn live_parakeet(
    recognizer: &mut airlock_asr::ParakeetRecognizer,
    audio: &[f32],
    origin: airlock_core::Ts,
) -> Result<(Vec<String>, Vec<f64>), String> {
    use airlock_asr::{ASR_SAMPLE_RATE, SpeechRecognizer};
    use std::time::Instant;

    let mut heard = Vec::new();
    let mut latencies = Vec::new();
    let mut fed = 0usize;
    let mut wall = 0.0_f64;
    let chunk = (ASR_SAMPLE_RATE as usize) / 50;
    while fed < audio.len() {
        let end = (fed + chunk).min(audio.len());
        let start =
            origin + airlock_core::Dur((fed as i64) * 1_000_000 / i64::from(ASR_SAMPLE_RATE));
        let media_end = (end as f64) / f64::from(ASR_SAMPLE_RATE);
        if wall < media_end {
            wall = media_end;
        }
        let began = Instant::now();
        let words = recognizer
            .push(&audio[fed..end], start)
            .map_err(|err| err.to_string())?;
        wall += began.elapsed().as_secs_f64();
        for word in words {
            let spoken_end = word.span.end.as_secs_f64() - origin.as_secs_f64();
            latencies.push((wall - spoken_end) * 1_000.0);
            heard.push(word.text);
        }
        fed = end;
    }
    let began = Instant::now();
    let words = recognizer.flush().map_err(|err| err.to_string())?;
    wall += began.elapsed().as_secs_f64();
    for word in words {
        let spoken_end = word.span.end.as_secs_f64() - origin.as_secs_f64();
        latencies.push((wall - spoken_end) * 1_000.0);
        heard.push(word.text);
    }
    Ok((heard, latencies))
}

#[cfg(all(feature = "parakeet", feature = "whisper"))]
fn speak(wav: &std::path::Path, text: &str) -> Result<(), String> {
    use std::process::{Command, Stdio};

    let script = format!(
        "Add-Type -AssemblyName System.Speech; $s = New-Object System.Speech.Synthesis.SpeechSynthesizer; $s.SetOutputToWaveFile('{}'); $s.Speak('{}'); $s.Dispose()",
        wav.display().to_string().replace('\'', "''"),
        text.replace('\'', "''"),
    );
    let status = Command::new("powershell")
        .args(["-NoProfile", "-Command", &script])
        .stdin(Stdio::null())
        .status()
        .map_err(|err| err.to_string())?;
    if status.success() && wav.exists() {
        Ok(())
    } else {
        Err(format!("speech synthesis failed for {}", wav.display()))
    }
}

#[cfg(all(feature = "parakeet", feature = "whisper"))]
fn read_f32(wav: &std::path::Path) -> Result<Vec<f32>, String> {
    use std::process::{Command, Stdio};

    let output = Command::new("ffmpeg")
        .args([
            "-loglevel",
            "error",
            "-i",
            &wav.display().to_string(),
            "-f",
            "f32le",
            "-acodec",
            "pcm_f32le",
            "-ac",
            "1",
            "-ar",
            "16000",
            "-",
        ])
        .stdin(Stdio::null())
        .output()
        .map_err(|err| err.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "ffmpeg failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let samples = output
        .stdout
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect();
    Ok(samples)
}

#[cfg(all(feature = "parakeet", feature = "whisper"))]
fn normalize(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut digits = String::new();
    for word in text
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(|word| word.to_ascii_lowercase())
    {
        if let Some(digit) = spoken_digit(&word) {
            digits.push(digit);
            continue;
        }
        if !digits.is_empty() {
            out.push(std::mem::take(&mut digits));
        }
        out.push(word);
    }
    if !digits.is_empty() {
        out.push(digits);
    }
    out
}

#[cfg(all(feature = "parakeet", feature = "whisper"))]
fn spoken_digit(word: &str) -> Option<char> {
    if word.len() == 1
        && let Some(digit) = word.chars().next()
        && digit.is_ascii_digit()
    {
        return Some(digit);
    }
    Some(match word {
        "zero" | "oh" => '0',
        "one" => '1',
        "two" => '2',
        "three" => '3',
        "four" => '4',
        "five" => '5',
        "six" => '6',
        "seven" => '7',
        "eight" => '8',
        "nine" => '9',
        _ => return None,
    })
}

#[cfg(all(feature = "parakeet", feature = "whisper"))]
fn edits(hyp: &[String], reference: &[String]) -> usize {
    let m = hyp.len();
    let mut prev: Vec<usize> = (0..=m).collect();
    let mut cur = vec![0; m + 1];
    for (i, reference_word) in reference.iter().enumerate() {
        cur[0] = i + 1;
        for (j, hyp_word) in hyp.iter().enumerate() {
            let cost = usize::from(reference_word != hyp_word);
            cur[j + 1] = (prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[m]
}

#[cfg(all(feature = "parakeet", feature = "whisper"))]
fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = (p * sorted.len() as f64).ceil() as usize;
    sorted[(rank.max(1) - 1).min(sorted.len() - 1)]
}
