//! Offline license check. Wordlist bleeps, the dump button, and spoken PII work
//! without a key. The context judge, screen scanning, and cloud providers need Pro.
//!
//! The signing key in this crate is a development key. Pricing is still an open
//! question, so this is the check, not a store.

use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

const DEV_KEY: &[u8] = b"airlock-dev-license-key-v1!!!!!!";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feature {
    Wordlist,
    Dump,
    SpokenPii,
    ContextJudge,
    ScreenScan,
    CloudJudge,
}

/// Signs a development Pro license. The key ships with the repo until pricing exists.
pub fn issue_dev_license() -> String {
    let mac = hmac(b"pro");
    format!("pro.{mac}")
}

pub fn allows(license: &str, feature: Feature) -> bool {
    match feature {
        Feature::Wordlist | Feature::Dump | Feature::SpokenPii => true,
        Feature::ContextJudge | Feature::ScreenScan | Feature::CloudJudge => is_pro(license),
    }
}

fn is_pro(license: &str) -> bool {
    let Some(sig) = license.trim().strip_prefix("pro.") else {
        return false;
    };
    sig.len() == 64 && sig.eq_ignore_ascii_case(&hmac(b"pro"))
}

fn hmac(payload: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(DEV_KEY).expect("hmac key length");
    mac.update(payload);
    let bytes = mac.finalize().into_bytes();
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_tier_stays_on_and_pro_needs_a_key() {
        assert!(allows("", Feature::Dump));
        assert!(allows("", Feature::SpokenPii));
        assert!(!allows("", Feature::ContextJudge));
        assert!(!allows("pro.00", Feature::ScreenScan));
        let key = issue_dev_license();
        assert!(allows(&key, Feature::ContextJudge));
        assert!(allows(&key, Feature::CloudJudge));
        assert!(!key.contains("Springfield"));
    }
}
