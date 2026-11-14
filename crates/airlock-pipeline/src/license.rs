//! Feature names kept so call sites stay explicit. Every feature is on.
//! There is no paid tier.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feature {
    Wordlist,
    Dump,
    SpokenPii,
    ContextJudge,
    ScreenScan,
    CloudJudge,
    /// Cloud speech recognition. Only after explicit consent.
    CloudSpeech,
    /// More than one destination in the same session.
    Multistream,
}

/// Every feature is available. `license` is ignored so an older settings file
/// that still has a key loads the same way as one that does not.
pub fn allows(_license: &str, _feature: Feature) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_feature_is_on_without_a_key() {
        for feature in [
            Feature::Wordlist,
            Feature::Dump,
            Feature::SpokenPii,
            Feature::ContextJudge,
            Feature::ScreenScan,
            Feature::CloudJudge,
            Feature::CloudSpeech,
            Feature::Multistream,
        ] {
            assert!(allows("", feature));
            assert!(allows("pro.00", feature));
        }
    }
}
