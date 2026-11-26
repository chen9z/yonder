//! Masking secrets before text reaches the ledger: API keys by their
//! well-known prefixes, bearer tokens, and the values of `KEY=value`
//! assignments whose key names a secret. Leaves everything else as it is.

/// What a secret is replaced with.
pub const MASK: &str = "[REDACTED]";

/// Prefixes of API keys and tokens, each followed by at least `MIN_SECRET`
/// key characters.
const PREFIXES: &[&str] = &[
    "sk-", "ghp_", "gho_", "ghu_", "ghs_", "github_pat_", "glpat-", "xoxb-", "xoxp-", "AKIA",
];
const MIN_SECRET: usize = 16;

/// Keys whose values are secrets, in lowercase: `API_KEY=...`, `token: ...`.
const KEY_WORDS: &[&str] = &["api_key", "apikey", "api-key", "token", "secret", "password", "passwd"];
const MIN_VALUE: usize = 4;

/// `text` with every secret in it replaced by `MASK`.
pub fn redact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    let mut prev: Option<char> = None;
    while let Some(c) = rest.chars().next() {
        if !prev.is_some_and(is_key_char) {
            let found = prefixed(rest).map(|n| (0, n)).or_else(|| bearer(rest)).or_else(|| assigned(rest));
            if let Some((keep, n)) = found {
                out.push_str(&rest[..keep]);
                out.push_str(MASK);
                prev = Some(']');
                rest = &rest[keep + n..];
                continue;
            }
        }
        out.push(c);
        prev = Some(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}

/// `text` masked, then cut to `max` characters (`…` on a line of its own
/// if cut). Masking first: a cut could leave half a key that no longer
/// looks like one.
pub fn excerpt(text: &str, max: usize) -> String {
    let masked = redact(text);
    let mut cut: String = masked.chars().take(max).collect();
    if cut.len() < masked.len() {
        cut.push_str("\n…");
    }
    cut
}

fn is_key_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

fn run_len(s: &str, ok: impl Fn(char) -> bool) -> usize {
    s.find(|c: char| !ok(c)).unwrap_or(s.len())
}

/// The length of a prefixed key at the start of `s`.
fn prefixed(s: &str) -> Option<usize> {
    PREFIXES.iter().find_map(|p| {
        let n = run_len(s.strip_prefix(p)?, is_key_char);
        (n >= MIN_SECRET).then_some(p.len() + n)
    })
}

/// `Bearer <token>`: what to keep, and the token's length.
fn bearer(s: &str) -> Option<(usize, usize)> {
    let head = s.get(..7)?;
    if !head.eq_ignore_ascii_case("bearer ") {
        return None;
    }
    let n = run_len(&s[7..], |c| is_key_char(c) || ".~+/=".contains(c));
    (n >= MIN_SECRET).then_some((7, n))
}

/// `SOME_TOKEN = "value"`: what to keep (key, separator, quote), and the
/// value's length.
fn assigned(s: &str) -> Option<(usize, usize)> {
    let key_len = run_len(s, is_key_char);
    let key = s[..key_len].to_ascii_lowercase();
    if !KEY_WORDS.iter().any(|w| key.contains(w)) {
        return None;
    }
    let after = &s[key_len..];
    let sep = after.trim_start_matches(' ');
    let sep = sep.strip_prefix('=').or_else(|| sep.strip_prefix(':'))?;
    let value = sep.trim_start_matches(' ').trim_start_matches(['"', '\'']);
    let keep = s.len() - value.len();
    let n = run_len(value, |c| !c.is_whitespace() && !"\"'`,;&".contains(c));
    (n >= MIN_VALUE && !value.starts_with(MASK)).then_some((keep, n))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_api_keys() {
        let cases = [
            ("key sk-1234567890abcdefghijklmno here", "key [REDACTED] here"),
            ("(sk-proj-AbCdEfGhIjKlMnOpQrStUv)", "([REDACTED])"),
            ("push with ghp_0123456789abcdefABCDEF0123", "push with [REDACTED]"),
            ("aws AKIAIOSFODNN7EXAMPLE.", "aws [REDACTED]."),
            ("slack xoxb-1111-2222-abcdefghijkl", "slack [REDACTED]"),
            ("Authorization: Bearer eyJhbGciOi.eyJzdWIiOi.c2ln", "Authorization: Bearer [REDACTED]"),
            ("SECRET_API_KEY=leak and more", "SECRET_API_KEY=[REDACTED] and more"),
            ("export HF_TOKEN=\"hf_abcdefgh\";", "export HF_TOKEN=\"[REDACTED]\";"),
            ("password: hunter22", "password: [REDACTED]"),
            ("密钥 sk-1234567890abcdefghijklmno，已换", "密钥 [REDACTED]，已换"),
        ];
        for (text, want) in cases {
            assert_eq!(redact(text), want, "{text}");
            assert_eq!(redact(want), want, "masking twice changes nothing: {want}");
        }
    }

    #[test]
    fn excerpts_mask_before_cutting() {
        let text = format!("{} sk-1234567890abcdefghijklmno", "x".repeat(10));
        assert_eq!(excerpt(&text, 15), "xxxxxxxxxx [RED\n…");
        assert_eq!(excerpt("short", 15), "short");
        assert_eq!(excerpt("密钥很长很长", 3), "密钥很\n…");
    }

    #[test]
    fn leaves_plain_text() {
        for text in [
            "",
            "T-0001 fix the build on gpu-box",
            "sk-short and task-1234567890abcdefghij",
            "risk-1234567890abcdefghijklmno is not a key",
            "max token: 42, the password reset is tomorrow",
            "token budget 4096",
            "部署完成，端口 8090 恢复 200",
        ] {
            assert_eq!(redact(text), text);
        }
    }
}
