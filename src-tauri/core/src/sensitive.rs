//! Conservative, explainable detection of likely secrets. This is a safety net, not a
//! guarantee: it only flags well-known token formats and password-shaped strings.

use std::sync::LazyLock;

use regex::Regex;

struct Rule {
    reason: &'static str,
    re: Regex,
}

static RULES: LazyLock<Vec<Rule>> = LazyLock::new(|| {
    let r = |reason, pat: &str| Rule {
        reason,
        re: Regex::new(pat).expect("valid regex"),
    };
    vec![
        r(
            "private key block",
            r"-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----",
        ),
        r("AWS access key", r"\b(AKIA|ASIA)[0-9A-Z]{16}\b"),
        r(
            "GitHub token",
            r"\b(gh[pousr]_[A-Za-z0-9]{36,}|github_pat_[A-Za-z0-9_]{50,})\b",
        ),
        r("GitLab token", r"\bglpat-[A-Za-z0-9_-]{20,}\b"),
        r("Slack token", r"\bxox[abprs]-[A-Za-z0-9-]{10,}"),
        r(
            "API secret key",
            r"\bsk-(ant-|proj-|live_)?[A-Za-z0-9_-]{20,}",
        ),
        r("Stripe key", r"\b[rs]k_live_[A-Za-z0-9]{20,}\b"),
        r("Google API key", r"\bAIza[0-9A-Za-z_-]{35}\b"),
        r(
            "JSON Web Token",
            r"\beyJ[A-Za-z0-9_-]{10,}\.eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}",
        ),
    ]
});

/// Returns a human-readable reason if `text` looks like a secret.
pub fn detect(text: &str) -> Option<&'static str> {
    // Scanning megabytes of text for tokens is wasteful; secrets are short.
    let sample = if text.len() > 64 * 1024 {
        return None;
    } else {
        text
    };
    if let Some(rule) = RULES.iter().find(|r| r.re.is_match(sample)) {
        return Some(rule.reason);
    }
    if looks_like_password(sample.trim()) {
        return Some("password-like string (single word mixing upper, lower, digits and symbols)");
    }
    None
}

/// A single 8–64 char token that mixes all four character classes and is not an obvious
/// URL, path or e-mail address.
fn looks_like_password(s: &str) -> bool {
    let len = s.chars().count();
    if !(8..=64).contains(&len) || s.chars().any(char::is_whitespace) {
        return false;
    }
    if s.contains("://") || s.starts_with('/') || s.starts_with('~') || s.starts_with("./") {
        return false;
    }
    if s.contains('@') && s.contains('.') && !s.contains('!') && !s.contains('#') {
        return false; // e-mail-like
    }
    let lower = s.chars().any(|c| c.is_lowercase());
    let upper = s.chars().any(|c| c.is_uppercase());
    let digit = s.chars().any(|c| c.is_ascii_digit());
    let symbol = s.chars().any(|c| !c.is_alphanumeric());
    lower && upper && digit && symbol
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_known_tokens() {
        assert_eq!(detect("AKIAIOSFODNN7EXAMPLE"), Some("AWS access key"));
        assert_eq!(
            detect("token: ghp_abcdefghijklmnopqrstuvwxyz0123456789"),
            Some("GitHub token")
        );
        assert_eq!(
            detect("-----BEGIN OPENSSH PRIVATE KEY-----\nabc"),
            Some("private key block")
        );
        assert_eq!(
            detect("sk-ant-api03-abcdefghijklmnopqrstuvwx"),
            Some("API secret key")
        );
        assert_eq!(
            detect("Tr0ub4dor&3x"),
            Some("password-like string (single word mixing upper, lower, digits and symbols)")
        );
    }

    #[test]
    fn ignores_ordinary_text() {
        for s in [
            "hello world",
            "https://Example.com/Path?a=1",
            "/home/User/Docs/File2.txt",
            "First.Last2024@example.com",
            "fn main() { println!(\"Hi 2\"); }",
            "Password123",
            "sk-short",
            "ThisIsAVeryLongSentenceWithoutSpacesButNoSymbolsOrDigits",
        ] {
            assert_eq!(detect(s), None, "{s}");
        }
    }
}
