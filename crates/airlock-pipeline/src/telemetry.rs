//! Opt-in latency and crash reports. The payload has no transcript, terms, or evidence.
//! Nothing is produced unless the streamer opted in and a destination is configured.

/// Returns the JSON body to send, or `None` when the report must not leave the machine.
pub fn telemetry_report(
    opt_in: bool,
    endpoint: &str,
    kind: &str,
    latency_us: i64,
) -> Option<String> {
    if !opt_in || endpoint.is_empty() {
        return None;
    }
    if kind != "crash" && kind != "latency" {
        return None;
    }
    if latency_us < 0 {
        return None;
    }
    Some(format!(r#"{{"kind":"{kind}","latency_us":{latency_us}}}"#))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_cannot_leave_and_the_default_is_silence() {
        let planted = "live_123456789_AbCdEfGhIjKlMnOpQrStUvWx";
        assert!(telemetry_report(false, "https://example.invalid", "latency", 10).is_none());
        assert!(telemetry_report(true, "", "latency", 10).is_none());
        assert!(telemetry_report(true, "https://example.invalid", "transcript", 10).is_none());
        let body = telemetry_report(true, "https://example.invalid", "latency", 250_000).unwrap();
        assert!(!body.contains(planted));
        assert!(!body.contains("evidence"));
        assert!(body.contains("\"kind\":\"latency\""));
    }
}
