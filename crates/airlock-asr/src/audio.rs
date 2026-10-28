//! Converting program audio (48 kHz interleaved) into what ASR wants (16 kHz mono).

/// Averages interleaved channels into mono.
pub fn downmix_to_mono(interleaved: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return interleaved.to_vec();
    }
    interleaved
        .chunks_exact(channels)
        .map(|f| f.iter().sum::<f32>() / channels as f32)
        .collect()
}

/// Linear-interpolation resampler. Good enough for ASR input; swap for `rubato` if
/// evals show it matters. Stateless, so feed it chunks large enough to hide edges
/// (>= 20ms) or keep a 1-sample overlap upstream.
pub fn resample_linear(input: &[f32], from_hz: u32, to_hz: u32) -> Vec<f32> {
    if from_hz == to_hz || input.is_empty() {
        return input.to_vec();
    }
    let ratio = from_hz as f64 / to_hz as f64;
    let out_len = ((input.len() as f64) / ratio).floor() as usize;
    (0..out_len)
        .map(|i| {
            let pos = i as f64 * ratio;
            let i0 = pos.floor() as usize;
            let i1 = (i0 + 1).min(input.len() - 1);
            let frac = (pos - i0 as f64) as f32;
            input[i0] * (1.0 - frac) + input[i1] * frac
        })
        .collect()
}

/// True when the chunk is louder than about −40 dBFS.
/// Silence must not trip the stall fail-safe.
pub fn has_speech_energy(samples: &[f32]) -> bool {
    if samples.is_empty() {
        return false;
    }
    let mean_sq = samples
        .iter()
        .map(|s| f64::from(*s) * f64::from(*s))
        .sum::<f64>()
        / samples.len() as f64;
    mean_sq.sqrt() >= 0.01
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn energy_ignores_silence() {
        assert!(!has_speech_energy(&[]));
        assert!(!has_speech_energy(&[0.0; 480]));
        assert!(has_speech_energy(&[0.2; 480]));
    }

    #[test]
    fn downmix() {
        assert_eq!(downmix_to_mono(&[1.0, 0.0, 0.5, 0.5], 2), vec![0.5, 0.5]);
    }

    #[test]
    fn resample_length() {
        let x = vec![0.0f32; 48_000];
        assert_eq!(resample_linear(&x, 48_000, 16_000).len(), 16_000);
    }
}
