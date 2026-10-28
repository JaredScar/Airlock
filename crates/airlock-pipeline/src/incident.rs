//! Incident log export for VOD cleanup. Evidence is already masked upstream.

use airlock_core::{Action, Redaction};

fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// One row per redaction: when, what, what was done, whether it actually applied live.
pub fn to_csv(items: &[Redaction]) -> String {
    let mut out =
        String::from("start,end,category,action,applied_live,status,confidence,reasons,evidence\n");
    for r in items {
        let category = serde_json::to_value(r.category)
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default();
        let action = serde_json::to_value(r.action)
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default();
        out.push_str(&format!(
            "{},{},{},{},{},{:?},{:.2},{},{}\n",
            r.span.start,
            r.span.end,
            category,
            action,
            r.is_active() && r.action != Action::LogOnly,
            r.status,
            r.confidence,
            csv_field(&r.reasons.join("; ")),
            csv_field(&r.evidence.join("; ")),
        ));
    }
    out
}
