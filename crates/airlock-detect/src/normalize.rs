//! Token normalization for wordlist matching.

/// Lowercases, maps common leetspeak, and strips everything that isn't a letter.
/// `"F#CK!"` stays `"fck"` (no match by design), `"$h1t"` -> `"shit"`.
pub fn normalize_token(s: &str) -> String {
    // Trim edge punctuation first so "shit!" doesn't become "shiti"; keep `$`/`@`, which
    // are leetspeak at the edges ("$hit").
    s.trim_matches(|c: char| !c.is_alphanumeric() && c != '$' && c != '@')
        .chars()
        .map(|c| match c {
            '0' => 'o',
            '1' | '!' => 'i',
            '3' => 'e',
            '4' | '@' => 'a',
            '5' | '$' => 's',
            '7' => 't',
            c => c,
        })
        .flat_map(|c| c.to_lowercase())
        .filter(|c| c.is_alphabetic())
        .collect()
}

/// Collapses every run of repeated letters to one: `"fuuuuck"` -> `"fuck"`, `"ass"` -> `"as"`.
pub fn collapse_runs(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev = None;
    for c in s.chars() {
        if prev != Some(c) {
            out.push(c);
        }
        prev = Some(c);
    }
    out
}

/// True if the token has a run of 3+ identical letters (deliberate elongation).
pub fn is_elongated(s: &str) -> bool {
    let mut run = 1;
    let mut prev = None;
    for c in s.chars() {
        if prev == Some(c) {
            run += 1;
            if run >= 3 {
                return true;
            }
        } else {
            run = 1;
        }
        prev = Some(c);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leet_and_case() {
        assert_eq!(normalize_token("$h1T"), "shit");
        assert_eq!(normalize_token("Hello,"), "hello");
        assert_eq!(normalize_token("shit!"), "shit");
        assert_eq!(normalize_token("sh!t"), "shit");
    }

    #[test]
    fn runs() {
        assert_eq!(collapse_runs("fuuuuck"), "fuck");
        assert!(is_elongated("shiiit"));
        assert!(!is_elongated("assess"));
    }
}
