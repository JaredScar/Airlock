//! Which OBS sources get their own screenshot.
//!
//! Never-scan drops a source even when it is on the current scene. Always-scan
//! includes a source even when the program scene is something else. A name in
//! both lists is never scanned.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceScan {
    Always,
    Never,
    Default,
}

pub fn source_scan(name: &str, always: &[String], never: &[String]) -> SourceScan {
    let name = name.trim();
    if listed(name, never) {
        return SourceScan::Never;
    }
    if listed(name, always) {
        return SourceScan::Always;
    }
    SourceScan::Default
}

/// Sources to screenshot. Current-scene sources stay, minus never-scan.
/// Always-scan names are added even when they are not on that scene.
pub fn screenshot_targets(current: &[String], always: &[String], never: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for name in current.iter().chain(always.iter()) {
        let trimmed = name.trim();
        if trimmed.is_empty() || source_scan(trimmed, always, never) == SourceScan::Never {
            continue;
        }
        if out
            .iter()
            .any(|kept: &String| kept.eq_ignore_ascii_case(trimmed))
        {
            continue;
        }
        out.push(trimmed.to_string());
    }
    out
}

fn listed(name: &str, names: &[String]) -> bool {
    !name.is_empty()
        && names
            .iter()
            .any(|item| item.trim().eq_ignore_ascii_case(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_string()).collect()
    }

    #[test]
    fn gameplay_is_skipped_and_the_browser_is_kept() {
        let current = names(&["Game Capture", "Mic"]);
        let always = names(&["Browser"]);
        let never = names(&["game capture"]);
        assert_eq!(
            source_scan("Game Capture", &always, &never),
            SourceScan::Never
        );
        assert_eq!(source_scan("Browser", &always, &never), SourceScan::Always);
        assert_eq!(source_scan("Mic", &always, &never), SourceScan::Default);
        assert_eq!(
            screenshot_targets(&current, &always, &never),
            names(&["Mic", "Browser"])
        );
    }

    #[test]
    fn a_name_on_both_lists_is_not_scanned() {
        let both = names(&["Browser"]);
        assert_eq!(source_scan("Browser", &both, &both), SourceScan::Never);
        assert!(screenshot_targets(&both, &both, &both).is_empty());
    }
}
