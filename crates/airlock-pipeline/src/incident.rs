//! Incident log export for VOD cleanup. Evidence is already masked upstream.
//!
//! CSV is the full row. EDL (CMX 3600, 30 fps), WebVTT chapters, and YouTube
//! description timestamps are timelines a VOD editor can jump through.

use airlock_core::{Redaction, Ts};
use serde::Serialize;

fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

fn snake(value: impl Serialize) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

fn by_start(items: &[Redaction]) -> Vec<&Redaction> {
    let mut v: Vec<_> = items.iter().collect();
    v.sort_by_key(|r| r.span.start);
    v
}

fn note(r: &Redaction) -> String {
    let mut text = format!("{} — {}", snake(r.category), snake(r.action));
    if r.missed_live {
        text.push_str(" — missed live");
    }
    text
}

/// One eval-set line for "this was wrong". Evidence is left out entirely.
pub fn anonymized_case(item: &Redaction) -> String {
    serde_json::json!({
        "clip": "correction",
        "category": item.category,
        "start_us": item.span.start.0,
        "end_us": item.span.end.0,
        "note": "marked wrong by the streamer"
    })
    .to_string()
}

/// One row per redaction: when, what, what was done, whether it actually applied live.
pub fn to_csv(items: &[Redaction]) -> String {
    let mut out = String::from(
        "start,end,category,action,applied_live,status,confidence,reasons,evidence,missed_live\n",
    );
    for r in items {
        out.push_str(&format!(
            "{},{},{},{},{},{:?},{:.2},{},{},{}\n",
            r.span.start,
            r.span.end,
            snake(r.category),
            snake(r.action),
            r.is_active() && r.action != airlock_core::Action::LogOnly,
            r.status,
            r.confidence,
            csv_field(&r.reasons.join("; ")),
            csv_field(&r.evidence.join("; ")),
            r.missed_live,
        ));
    }
    out
}

/// CMX 3600 edit decision list at 30 fps, one event per redaction.
pub fn to_edl(items: &[Redaction]) -> String {
    let mut out = String::from("TITLE: Airlock incidents\nFCM: NON-DROP FRAME\n\n");
    for (i, r) in by_start(items).into_iter().enumerate() {
        let track = if r.is_audio() { "A" } else { "V" };
        let inn = edl_tc(r.span.start);
        let out_tc = edl_tc(later(r.span.end, r.span.start + airlock_core::Dur(1)));
        let n = i + 1;
        out.push_str(&format!(
            "{n:03}  AX       {track:<6}C        {inn} {out_tc} {inn} {out_tc}\n* FROM CLIP NAME: {}\n",
            note(r)
        ));
    }
    out
}

/// YouTube description chapters. The first line is always `0:00`.
pub fn to_youtube_timestamps(items: &[Redaction]) -> String {
    let ordered = by_start(items);
    let mut lines = Vec::new();
    let starts_at_zero = ordered
        .first()
        .is_some_and(|r| r.span.start.0 / 1_000_000 == 0);
    if !starts_at_zero {
        lines.push("0:00 Start".to_string());
    }
    for r in ordered {
        lines.push(format!("{} {}", youtube_clock(r.span.start), note(r)));
    }
    lines.join("\n") + "\n"
}

/// WebVTT chapter cues an editor can import.
pub fn to_chapters(items: &[Redaction]) -> String {
    let mut out = String::from("WEBVTT\n\n");
    for r in by_start(items) {
        let end = later(r.span.end, r.span.start + airlock_core::Dur::from_millis(1));
        out.push_str(&format!("{} --> {}\n{}\n\n", r.span.start, end, note(r)));
    }
    out
}

fn later(a: Ts, b: Ts) -> Ts {
    a.max(b)
}

/// `HH:MM:SS:FF` at 30 fps, non-drop.
fn edl_tc(ts: Ts) -> String {
    let frames_total = ts.0.max(0).saturating_mul(30) / 1_000_000;
    let frames = frames_total % 30;
    let total_secs = frames_total / 30;
    let s = total_secs % 60;
    let m = (total_secs / 60) % 60;
    let h = total_secs / 3600;
    format!("{h:02}:{m:02}:{s:02}:{frames:02}")
}

fn youtube_clock(ts: Ts) -> String {
    let secs = ts.0.max(0) / 1_000_000;
    let h = secs / 3600;
    let m = (secs / 60) % 60;
    let s = secs % 60;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use airlock_core::{Action, Category, DetectorKind, Mode, Status, Target};

    fn item(start_ms: i64, end_ms: i64, missed: bool) -> Redaction {
        Redaction {
            id: 1,
            category: Category::Profanity,
            mode: Mode::Apply,
            action: Action::Bleep,
            target: Target::Audio,
            span: airlock_core::Span::from_millis(start_ms, end_ms),
            status: Status::Pending,
            confidence: 0.9,
            sources: vec![DetectorKind::Wordlist],
            reasons: vec!["test".into()],
            evidence: vec!["fu••••".into()],
            missed_live: missed,
        }
    }

    #[test]
    fn exports_csv_edl_chapters_and_youtube_timestamps() {
        let items = vec![item(65_000, 66_000, true), item(1_500, 2_000, false)];
        let csv = to_csv(&items);
        assert!(csv.starts_with(
            "start,end,category,action,applied_live,status,confidence,reasons,evidence,missed_live\n"
        ));
        assert!(csv.contains(",true\n") || csv.lines().any(|l| l.ends_with(",true")));
        assert!(csv.contains("profanity"));
        assert!(!csv.contains("fuck"));

        let edl = to_edl(&items);
        let first = edl.lines().nth(3).unwrap();
        assert!(first.starts_with("001  AX       A"), "{first}");
        assert!(first.contains("00:00:01:15"), "{edl}");
        assert!(edl.contains("missed live"));
        assert!(edl.lines().nth(5).unwrap().starts_with("002"));

        let yt = to_youtube_timestamps(&items);
        assert!(yt.starts_with("0:00 Start\n0:01 profanity — bleep\n"));
        assert!(yt.contains("1:05 profanity — bleep — missed live"));

        let vtt = to_chapters(&items);
        assert!(vtt.starts_with("WEBVTT\n"));
        assert!(vtt.contains("00:00:01.500 --> 00:00:02.000\nprofanity — bleep\n"));
        assert!(vtt.contains("missed live"));
    }

    #[test]
    fn a_correction_does_not_carry_evidence() {
        let mut row = item(1_000, 2_000, false);
        row.evidence = vec!["live_123456789_AbCdEfGhIjKlMnOpQrStUvWx".into()];
        let line = anonymized_case(&row);
        assert!(line.contains("profanity"));
        assert!(!line.contains("live_"));
        assert!(!line.contains("evidence"));
    }
}
