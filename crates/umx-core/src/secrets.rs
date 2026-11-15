//! Secret scan on ingest (req-49, SEC-10; rules agreed with security, ws004 s08): does a text look like it holds a
//! credential? Single writes warn; batches refuse. The embedding hold judges only its own text inputs.
//!
//! The rules are a small, gitleaks-derived set of named shapes plus an entropy check on long tokens. Every rule is
//! linear in the text: a rule starts only where its own character class starts (so two starts never share a run),
//! and every look-ahead is capped. A hit names its rule only: never the text, an offset or a prefix.

/// The key a read's record names its flagged places under (F188): umx computes it, so a query column may not take its
/// name (F241, `query_column_reserved`).
pub const SECRET_SUSPECTED: &str = "secret_suspected";

/// One rule of the scan, by the name a warning shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rule {
    PrivateKey,
    AwsAccessKey,
    AwsSecretKey,
    GithubToken,
    SlackToken,
    ApiKey,
    GoogleApiKey,
    Jwt,
    Bearer,
    UrlPassword,
    ConnectionString,
    AssignedSecret,
    HighEntropy,
}

impl Rule {
    pub fn name(self) -> &'static str {
        match self {
            Rule::PrivateKey => "private_key",
            Rule::AwsAccessKey => "aws_access_key",
            Rule::AwsSecretKey => "aws_secret_key",
            Rule::GithubToken => "github_token",
            Rule::SlackToken => "slack_token",
            Rule::ApiKey => "api_key",
            Rule::GoogleApiKey => "google_api_key",
            Rule::Jwt => "jwt",
            Rule::Bearer => "bearer_token",
            Rule::UrlPassword => "url_password",
            Rule::ConnectionString => "connection_string_secret",
            Rule::AssignedSecret => "assigned_secret",
            Rule::HighEntropy => "high_entropy_token",
        }
    }
}

/// The first rule `text` trips, in [`Rule`] order, or None when it looks clean.
pub fn scan(text: &str) -> Option<Rule> {
    let b = text.as_bytes();
    type Check = fn(&[u8]) -> bool;
    let checks: [(Rule, Check); 13] = [
        (Rule::PrivateKey, private_key),
        (Rule::AwsAccessKey, aws),
        (Rule::AwsSecretKey, aws_secret),
        (Rule::GithubToken, github),
        (Rule::SlackToken, slack),
        (Rule::ApiKey, api_key),
        (Rule::GoogleApiKey, google),
        (Rule::Jwt, jwt),
        (Rule::Bearer, bearer),
        (Rule::UrlPassword, url_password),
        (Rule::ConnectionString, connection_string),
        (Rule::AssignedSecret, assigned),
        (Rule::HighEntropy, high_entropy),
    ];
    checks.iter().find(|(_, f)| f(b)).map(|(r, _)| *r)
}

/// Whether field `field`'s value looks like a secret, judged with the field's name in front (`password: ...`), as the
/// whole-node text shows it.
pub fn field_suspect(field: &str, value: &str) -> Option<Rule> {
    scan(&alloc::format!("{field}: {value}"))
}

fn alnum(c: u8) -> bool {
    c.is_ascii_alphanumeric()
}

/// Base64url and friends: the body of most tokens.
fn token_char(c: u8) -> bool {
    alnum(c) || c == b'_' || c == b'-'
}

fn b64_char(c: u8) -> bool {
    alnum(c) || matches!(c, b'+' | b'/' | b'=' | b'_' | b'-')
}

/// `b[i..]` starts with `p`, ASCII case-insensitively.
fn at(b: &[u8], i: usize, p: &[u8]) -> bool {
    b.len() >= i + p.len() && b[i..i + p.len()].eq_ignore_ascii_case(p)
}

/// `b[i..]` starts with `p` exactly.
fn at_exact(b: &[u8], i: usize, p: &[u8]) -> bool {
    b.len() >= i + p.len() && &b[i..i + p.len()] == p
}

/// Whether position `i` starts a run of `class`: the byte before it is not in the class.
fn starts(b: &[u8], i: usize, class: fn(u8) -> bool) -> bool {
    i == 0 || !class(b[i - 1])
}

/// How many bytes of `class` follow from `i`, counting no further than `cap`.
fn run(b: &[u8], i: usize, class: impl Fn(u8) -> bool, cap: usize) -> usize {
    b.get(i..)
        .unwrap_or_default()
        .iter()
        .take(cap)
        .take_while(|c| class(**c))
        .count()
}

/// Positions where any of `prefixes` starts a run of `class` (exact case).
fn prefixed(
    b: &[u8],
    prefixes: &[&[u8]],
    class: fn(u8) -> bool,
    f: impl Fn(usize, usize) -> bool,
) -> bool {
    (0..b.len())
        .any(|i| starts(b, i, class) && prefixes.iter().any(|p| at_exact(b, i, p) && f(i, p.len())))
}

fn private_key(b: &[u8]) -> bool {
    (0..b.len()).any(|i| {
        if at_exact(b, i, b"PuTTY-User-Key-File-") {
            return true;
        }
        if !at_exact(b, i, b"-----BEGIN ") {
            return false;
        }
        // The label up to the closing dashes, at most 48 bytes: "RSA PRIVATE KEY", "ENCRYPTED PRIVATE KEY", ...
        let label = &b[i + 11..b.len().min(i + 11 + 48)];
        let end = label
            .windows(5)
            .position(|w| w == b"-----")
            .unwrap_or(label.len());
        label[..end].windows(11).any(|w| w == b"PRIVATE KEY")
    })
}

fn aws(b: &[u8]) -> bool {
    prefixed(b, &[b"AKIA", b"ASIA", b"ABIA", b"ACCA"], alnum, |i, n| {
        let body = &b[i + n..b.len().min(i + n + 16)];
        body.len() == 16
            && body
                .iter()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
            && b.get(i + n + 16).is_none_or(|c| !alnum(*c))
    })
}

/// A bare AWS secret access key in prose (security): exactly 40 bytes of `[A-Za-z0-9/+]` with a digit and both
/// cases (not hex), within 64 bytes after "aws" or "secret". The slash keeps it from the entropy rule.
fn aws_secret(b: &[u8]) -> bool {
    fn class(c: u8) -> bool {
        alnum(c) || c == b'/' || c == b'+'
    }
    let mut i = 0;
    while i < b.len() {
        let len = run(b, i, class, usize::MAX);
        if len == 0 {
            i += 1;
            continue;
        }
        let r = &b[i..i + len];
        let before = &b[i.saturating_sub(64)..i];
        let near = before.windows(3).any(|w| w.eq_ignore_ascii_case(b"aws"))
            || before.windows(6).any(|w| w.eq_ignore_ascii_case(b"secret"));
        if len == 40
            && near
            && r.iter().any(u8::is_ascii_digit)
            && r.iter().any(u8::is_ascii_uppercase)
            && r.iter().any(u8::is_ascii_lowercase)
        {
            return true;
        }
        i += len;
    }
    false
}

fn github(b: &[u8]) -> bool {
    prefixed(
        b,
        &[b"ghp_", b"gho_", b"ghu_", b"ghs_", b"ghr_"],
        token_char,
        |i, n| run(b, i + n, alnum, 36) >= 36,
    ) || prefixed(b, &[b"github_pat_"], token_char, |i, n| {
        run(b, i + n, |c| alnum(c) || c == b'_', 22) >= 22
    })
}

fn slack(b: &[u8]) -> bool {
    prefixed(
        b,
        &[b"xoxa-", b"xoxb-", b"xoxp-", b"xoxr-", b"xoxs-"],
        token_char,
        |i, n| run(b, i + n, |c| alnum(c) || c == b'-', 10) >= 10,
    )
}

fn api_key(b: &[u8]) -> bool {
    let openai = prefixed(b, &[b"sk-"], token_char, |i, n| {
        let len = run(b, i + n, token_char, 64);
        let body = &b[i + n..i + n + len];
        len >= 20 && body.iter().any(u8::is_ascii_digit) && body.iter().any(u8::is_ascii_alphabetic)
    });
    openai
        || prefixed(
            b,
            &[b"sk_live_", b"sk_test_", b"rk_live_", b"rk_test_"],
            token_char,
            |i, n| run(b, i + n, alnum, 16) >= 16,
        )
}

fn google(b: &[u8]) -> bool {
    prefixed(b, &[b"AIza"], token_char, |i, n| {
        run(b, i + n, token_char, 35) >= 35
    })
}

fn jwt(b: &[u8]) -> bool {
    prefixed(b, &[b"eyJ"], token_char, |i, _| {
        // Three base64url segments of 10+ bytes, the second also a JSON object ("eyJ").
        let mut j = i;
        for seg in 0..3 {
            let len = run(b, j, token_char, 4096);
            if len < 10 || (seg == 1 && !at_exact(b, j, b"eyJ")) {
                return false;
            }
            j += len;
            if seg < 2 {
                if b.get(j) != Some(&b'.') {
                    return false;
                }
                j += 1;
            }
        }
        true
    })
}

fn bearer(b: &[u8]) -> bool {
    (0..b.len()).any(|i| {
        if !(starts(b, i, alnum) && at(b, i, b"bearer")) {
            return false;
        }
        let mut j = i + 6;
        let spaces = run(b, j, |c| c == b' ' || c == b'\t', 8);
        if spaces == 0 {
            return false;
        }
        j += spaces;
        let len = run(b, j, |c| b64_char(c) || matches!(c, b'.' | b'~'), 128);
        len >= 20 && !placeholder(&b[j..j + len])
    })
}

fn url_password(b: &[u8]) -> bool {
    (0..b.len()).any(|i| {
        if !at_exact(b, i, b"://") {
            return false;
        }
        let from = i + 3;
        fn stop(c: u8) -> bool {
            matches!(c, b'/' | b'?' | b'#' | b'"' | b'\'' | b'<' | b'>' | b'@')
                || c.is_ascii_whitespace()
        }
        let len = run(b, from, |c| !stop(c), 512);
        if b.get(from + len) != Some(&b'@') {
            return false;
        }
        let userinfo = &b[from..from + len];
        userinfo
            .iter()
            .position(|c| *c == b':')
            .map(|k| &userinfo[k + 1..])
            .is_some_and(|pass| !pass.is_empty() && !placeholder(pass))
    })
}

/// `Key=value` pairs of connection strings (ADO.NET, Azure Storage and Service Bus, SAS URLs).
fn connection_string(b: &[u8]) -> bool {
    const KEYS: [&[u8]; 6] = [
        b"password=",
        b"pwd=",
        b"accountkey=",
        b"sharedaccesskey=",
        b"sharedaccesssignature=",
        b"sig=",
    ];
    (0..b.len()).any(|i| {
        starts(b, i, alnum)
            && KEYS.iter().any(|k| {
                at(b, i, k) && {
                    let v = value(b, i + k.len());
                    v.len() >= 6 && !placeholder(v)
                }
            })
    })
}

/// A value after `=` or `:`, up to a separator, at most 64 bytes (enough to judge it).
fn value(b: &[u8], i: usize) -> &[u8] {
    let len = run(
        b,
        i,
        |c| {
            !(c.is_ascii_whitespace()
                || matches!(c, b';' | b'&' | b'"' | b'\'' | b',' | b')' | b'}' | b']'))
        },
        64,
    );
    &b[i..i + len]
}

/// `api_key: ...`, `AZURE_OPENAI_API_KEY=...`, `"password": "..."`: a secret-named key assigned a value that is not
/// a placeholder and is not plain words (8+ bytes, entropy 3 bits a byte or more).
fn assigned(b: &[u8]) -> bool {
    const KEYS: [&[u8]; 12] = [
        b"api_key",
        b"api-key",
        b"apikey",
        b"access_key",
        b"access-key",
        b"private_key",
        b"private-key",
        b"client_secret",
        b"client-secret",
        b"secret",
        b"token",
        b"passw",
    ];
    (0..b.len()).any(|i| {
        if !starts(b, i, alnum) {
            return false;
        }
        let Some(k) = KEYS.iter().find(|k| at(b, i, k)) else {
            return false;
        };
        let mut j = i + k.len();
        // "passw" covers password and passwd.
        if *k == b"passw" {
            j += run(b, j, |c| c.is_ascii_alphabetic(), 4);
        }
        // A tail joined by _ or - (secret_key, token-value); a letter right after the key (tokenizer) is another word.
        if matches!(b.get(j), Some(b'_' | b'-')) {
            j += run(b, j, token_char, 32);
        } else if b.get(j).is_some_and(|c| alnum(*c)) {
            return false;
        }
        j += run(b, j, |c| c == b'"' || c == b'\'', 1);
        j += run(b, j, |c| c == b' ' || c == b'\t', 8);
        if !matches!(b.get(j), Some(b':' | b'=')) || b.get(j + 1) == Some(&b'=') {
            return false;
        }
        j += 1;
        j += run(b, j, |c| c == b' ' || c == b'\t', 8);
        j += run(b, j, |c| c == b'"' || c == b'\'', 1);
        let v = value(b, j);
        v.len() >= 8 && !placeholder(v) && entropy(v) >= 3.0
    })
}

/// A long run (32+ bytes) of token bytes that looks random. Ids and digests are exempt: hex (dashes allowed, as in
/// UUIDs) of 40 digits or fewer, or exactly 64. Other hex is a hit; anything else needs a digit, both cases and 4.2
/// bits a byte. File and folder names made of plain words are not candidates ([`named_like`]).
fn high_entropy(b: &[u8]) -> bool {
    let mut i = 0;
    while i < b.len() {
        let len = run(b, i, b64_char, usize::MAX);
        if len == 0 {
            i += 1;
            continue;
        }
        let r = &b[i..i + len];
        i += len;
        // Each part between slashes on its own: a path or URL is words between slashes, and its parts together are
        // varied enough to pass for random. A base64 secret with a slash is left to the named rules.
        if r.split(|c| *c == b'/').any(random_looking) {
            return true;
        }
    }
    false
}

fn random_looking(r: &[u8]) -> bool {
    r.len() >= 32 && !named_like(r) && random_stats(r)
}

/// The byte statistics of a random token, before any name exemption: hex past id lengths, or a digit, both cases and
/// 4.2 bits a byte.
fn random_stats(r: &[u8]) -> bool {
    if r.iter().all(|c| c.is_ascii_hexdigit() || *c == b'-') {
        let digits = r.iter().filter(|c| c.is_ascii_hexdigit()).count();
        return digits > 40 && digits != 64;
    }
    r.iter().any(u8::is_ascii_digit)
        && r.iter().any(u8::is_ascii_uppercase)
        && r.iter().any(u8::is_ascii_lowercase)
        && entropy(r) >= 4.2
}

/// A file or folder name rather than a token (ws004 s23, agreed with security): words joined by `-` or `_`, such as
/// `RECAP-GLOBEX-roadmaps-2026-03-14`. Every segment must be plain and non-empty, so a leading, trailing or doubled
/// separator leaves the part a candidate (a token cannot escape by gaining one `_`), and one random segment does too.
/// Measured on uniformly random base64url tokens, this exempts 0.07% of those the entropy rule would catch at 32
/// bytes, 0.015% at 40 and none at 64. A part with no `-` or `_` is a name only in camelCase or PascalCase
/// ([`camel_name`], F146).
fn named_like(r: &[u8]) -> bool {
    let sep = |c: &u8| matches!(c, b'-' | b'_');
    if r.iter().any(sep) {
        r.split(sep).all(plain_segment)
    } else {
        camel_name(r)
    }
}

/// A camelCase or PascalCase name with no separator, such as a long column name (F146, rule from security):
/// `customerAccountBalanceHistory2024Total`, `ExternalBillingSystemCustomerID`. Split at every lower-to-upper and
/// letter/digit boundary, each word is an optional capital and lowercase letters (`Account`, `balance`), one all-caps
/// acronym of 2 to 5 letters (`DB`, `ID`), or at most 4 digits (`2024`). So no two capitals are in a row except
/// inside one acronym, and an acronym straight before a capitalised word (`UTCFor`) is not a name; at most 2
/// acronyms, at least 3 words, and 4 or more bytes a word on average. Measured on 10,000 uniformly random base64url
/// tokens at each of 32, 40 and 64 bytes, it exempts none of those the entropy rule catches (the test pins it).
fn camel_name(r: &[u8]) -> bool {
    let (mut start, mut words, mut acronyms) = (0, 0, 0);
    for i in 1..=r.len() {
        let boundary = i == r.len() || {
            let (a, b) = (r[i - 1], r[i]);
            (a.is_ascii_lowercase() && b.is_ascii_uppercase())
                || a.is_ascii_digit() != b.is_ascii_digit()
        };
        if !boundary {
            continue;
        }
        let w = &r[start..i];
        start = i;
        words += 1;
        let lower = |s: &[u8]| !s.is_empty() && s.iter().all(u8::is_ascii_lowercase);
        if w.iter().all(u8::is_ascii_uppercase) && (2..=5).contains(&w.len()) {
            acronyms += 1;
        } else if !(w.iter().all(u8::is_ascii_digit) && w.len() <= 4
            || lower(w)
            || w[0].is_ascii_uppercase() && lower(&w[1..]))
        {
            return false;
        }
    }
    words >= 3 && acronyms <= 2 && r.len() >= 4 * words
}

/// Letters and digits with at most one letter/digit boundary (`2026`, `v3`, `FY2026`, `300M`), whose letters change
/// case at most once per 3 letters, plus one (`HANDOVER`, `ProjectAlpha`, `iPhone`; not `wpymQWNUcXBv`).
fn plain_segment(s: &[u8]) -> bool {
    let digit_changes = s
        .windows(2)
        .filter(|p| p[0].is_ascii_digit() != p[1].is_ascii_digit())
        .count();
    let (mut letters, mut case_changes, mut last) = (0, 0, None);
    for upper in s
        .iter()
        .filter(|c| c.is_ascii_alphabetic())
        .map(|c| c.is_ascii_uppercase())
    {
        letters += 1;
        case_changes += usize::from(last.is_some_and(|l| l != upper));
        last = Some(upper);
    }
    !s.is_empty()
        && s.iter().all(|c| alnum(*c))
        && digit_changes <= 1
        && case_changes * 3 <= letters + 3
}

/// Shannon entropy in bits a byte.
fn entropy(v: &[u8]) -> f64 {
    let mut counts = [0u32; 256];
    for c in v {
        counts[usize::from(*c)] += 1;
    }
    let n = v.len() as f64;
    counts
        .iter()
        .filter(|c| **c > 0)
        .map(|c| {
            let p = f64::from(*c) / n;
            -p * libm::log2(p)
        })
        .sum()
}

/// A value that stands for a secret rather than being one: `<password>`, `${TOKEN}`, `$VAR`, `%VAR%`, `{{x}}`,
/// `***`, `xxxx`, `changeme`, `redacted`, `your_key`, ...
fn placeholder(v: &[u8]) -> bool {
    let v = v.trim_ascii();
    let has = |p: &[u8]| v.windows(p.len()).any(|w| w.eq_ignore_ascii_case(p));
    v.is_empty()
        || matches!(v[0], b'<' | b'$' | b'%' | b'{' | b'[')
        || v.ends_with(b">")
        || v.iter()
            .all(|c| matches!(c, b'*' | b'x' | b'X' | b'.' | b'-' | b'_' | b'#' | b'?'))
        || [
            &b"redacted"[..],
            b"placeholder",
            b"changeme",
            b"change_me",
            b"example",
            b"your_",
            b"your-",
            b"yourkey",
            b"xxxx",
            b"****",
        ]
        .iter()
        .any(|p| has(p))
        || [
            &b"password"[..],
            b"secret",
            b"token",
            b"none",
            b"null",
            b"undefined",
            b"string",
        ]
        .iter()
        .any(|p| v.eq_ignore_ascii_case(p))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::format;
    use alloc::string::String;
    use alloc::vec::Vec;

    /// Fake credentials built at run time, so no secret-shaped literal sits in the source for other scanners.
    fn fake(prefix: &str, body: &str, n: usize) -> String {
        let mut s = String::from(prefix);
        while s.len() < prefix.len() + n {
            s.push_str(body);
        }
        s.truncate(prefix.len() + n);
        s
    }

    fn mixed(n: usize) -> String {
        fake("", "Q7mZp2Kx9VbR4tLw8NcY3hJd6FsG1aE5", n)
    }

    #[test]
    fn each_rule_fires_on_its_shape() {
        let cases: Vec<(String, Rule)> = alloc::vec![
            (
                format!("-----BEGIN RSA {}-----\nMIIE", "PRIVATE KEY"),
                Rule::PrivateKey
            ),
            (
                format!("-----BEGIN ENCRYPTED {}-----", "PRIVATE KEY"),
                Rule::PrivateKey
            ),
            (
                format!("-----BEGIN OPENSSH {}-----", "PRIVATE KEY"),
                Rule::PrivateKey
            ),
            (
                format!("-----BEGIN PGP {} BLOCK-----", "PRIVATE KEY"),
                Rule::PrivateKey
            ),
            (
                format!("{}-User-Key-File-3: ssh-ed25519", "PuTTY"),
                Rule::PrivateKey
            ),
            (
                format!("key {} here", fake("AKIA", "Q7Z3X9K2", 16)),
                Rule::AwsAccessKey
            ),
            (
                format!("use {}", fake("ghp_", "q7Z3x9K2", 36)),
                Rule::GithubToken
            ),
            (fake("github_pat_", "11AQ7Z3_x9K2", 40), Rule::GithubToken),
            (fake("xoxb-", "1234-abcd", 30), Rule::SlackToken),
            (fake("sk-proj-", "q7Z3x9K2", 40), Rule::ApiKey),
            (fake("sk_live_", "q7Z3x9K2", 24), Rule::ApiKey),
            (fake("AIza", "Sy7Z3x9K2-", 35), Rule::GoogleApiKey),
            (
                format!(
                    "{}.{}.{}",
                    fake("eyJ", "hbGciOi", 20),
                    fake("eyJ", "zdWIiOi", 20),
                    mixed(20)
                ),
                Rule::Jwt,
            ),
            (format!("Authorization: Bearer {}", mixed(30)), Rule::Bearer),
            (format!("bearer {}", fake("", "a1.b2_c3", 24)), Rule::Bearer),
            (
                String::from("postgres://admin:s3cr3tPass@db.internal:5432/app"),
                Rule::UrlPassword
            ),
            (
                String::from("Server=db;User Id=sa;Password=Pa55word!x;"),
                Rule::ConnectionString
            ),
            (
                format!("DefaultEndpointsProtocol=https;AccountKey={}==;", mixed(40)),
                Rule::ConnectionString
            ),
            (
                format!(
                    "https://acct.blob.core.windows.net/c?sv=2022&sig={}",
                    mixed(20)
                ),
                Rule::ConnectionString
            ),
            (
                format!("AZURE_OPENAI_API_KEY={}", fake("", "3f9a0c7e1b2d4568", 32)),
                Rule::AssignedSecret
            ),
            (
                format!("\"api-key\": \"{}\"", fake("", "3f9a0c7e1b2d4568", 32)),
                Rule::AssignedSecret
            ),
            (
                format!("x-api-key: {}", fake("", "3f9a0c7e1b2d4568", 32)),
                Rule::AssignedSecret
            ),
            (
                String::from("password: correct-horse-battery"),
                Rule::AssignedSecret
            ),
            (
                format!("MY_SECRET={}", fake("", "q1w2e3r4t5y6u7i8", 16)),
                Rule::AssignedSecret
            ),
            (
                format!("client_secret = '{}'", fake("", "q1w2e3r4t5y6u7i8", 16)),
                Rule::AssignedSecret
            ),
            (format!("blob {} end", mixed(40)), Rule::HighEntropy),
            (fake("", "0123456789abcdef", 48), Rule::HighEntropy),
        ];
        for (text, rule) in cases {
            assert_eq!(scan(&text), Some(rule), "{rule:?} on {text}");
        }
    }

    #[test]
    fn ordinary_graph_text_is_clean() {
        let clean = [
            "Node 0f3c2a9be4d81f6a7c5e3b2d1a0f9e8d links to commit 3b18e512dba79e4c8300dd08aeb37f8e728b8dad.",
            "sha256 9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08 of the file",
            "uuid 123e4567-e89b-12d3-a456-426614174000 in the log",
            "See https://github.com/AI-Substrate/umx/pull/106 and http://localhost:8080/api",
            "postgres://user@db/app and postgres://user:<password>@db/app and redis://:${REDIS_PASS}@cache",
            "crates/umx-services/src/graph_store/vectors.rs and docs/project/workshops/001-concept-graph",
            "The password: see the vault. Rotate the token: weekly. secret: Kubernetes",
            "api_key: <your-api-key> and token = ${GITHUB_TOKEN} and password=changeme and secret: ********",
            "Password=;  pwd=$PWD  AccountKey=<key>  sig=xxxx",
            "tokenizer: bpe-based-tokenizer-v2 and the secretariat met; password == other",
            "MyVeryLongClassNameForTesting and the AbstractSingletonProxyFactoryBean",
            "sk-learn is a library; task-runner; desk-booking-system",
            "Bearer tokens are described in RFC 6750; use a bearer token header.",
            "AKIA is the prefix of AWS access keys; ghp_ is GitHub's.",
            "-----BEGIN CERTIFICATE----- and -----BEGIN PUBLIC KEY-----",
            "eyJ is how base64 JSON starts",
            "model embeddinggemma-300m-BF16 at /Users/me/.umx/models/embeddinggemma/model.gguf",
        ];
        for text in clean {
            assert_eq!(scan(text), None, "{text}");
        }
    }

    #[test]
    fn hex_ids_are_exempt_only_at_id_lengths() {
        assert_eq!(scan(&fake("", "0123456789abcdef", 40)), None);
        assert_eq!(scan(&fake("", "0123456789abcdef", 64)), None);
        assert_eq!(
            scan(&fake("", "0123456789abcdef", 41)),
            Some(Rule::HighEntropy)
        );
        assert_eq!(
            scan(&fake("", "0123456789abcdef", 128)),
            Some(Rule::HighEntropy)
        );
    }

    /// ws004 s23: a tester's batch warned high_entropy_token on a document path. Each of these fires without
    /// [`named_like`].
    #[test]
    fn file_names_and_paths_are_clean() {
        assert_eq!(
            field_suspect("path", "RECAP-GLOBEX-roadmaps-2026-03-14.md"),
            None
        );
        let names = [
            "docs/handovers/RECAP-GLOBEX-roadmaps-2026-03-14.md",
            "Meeting-Notes-ProjectAlpha-2026-10-29-v3.docx",
            "Q3_Financial_Report_FINAL_v2_2026-09-30.xlsx",
            "/Users/me/Downloads/IMG_20261029_143512_HDR-Edited-Final.jpg",
            "models/EmbeddingGemma-300M-Q8_0-Instruct-2026-11.gguf",
            "Architecture-Decision-Record-0042-Use-LadybugDB.md",
            "Customer_Onboarding_Checklist_2026_v12_DRAFT.csv",
            "Sprint-Retro-Notes-Team-Unimatrix-2026-Q4-Week3.yaml",
            "/srv/data/Export_SalesPipeline_FY2026_Q3_Region-APAC.json",
            "McDonald-iPhone-Launch-Notes-2026-11-11-Rev2.md",
            "C:\\Users\\me\\Documents\\HANDOVER-ConceptGraph-Release-Notes-2026-11-11.md",
            "\\\\fileserver\\share\\Board-Pack-November-2026-Final-Approved-v7.pdf",
        ];
        for name in names {
            assert_eq!(scan(name), None, "{name}");
            assert_eq!(field_suspect("source", name), None, "{name}");
        }
    }

    /// A key pasted into a path is still a key: named rules see through paths, and a random segment keeps a file
    /// name a high-entropy candidate.
    #[test]
    fn a_secret_inside_a_path_is_still_caught() {
        let in_path = format!("docs/{}.md", fake("sk-proj-", "q7Z3x9K2", 40));
        assert_eq!(scan(&in_path), Some(Rule::ApiKey));
        // A real token body is random: the entropy rule catches it here, since the github rule starts only where a
        // token starts (after a non-token byte), not after the '-'.
        let in_name = format!("notes-ghp_{}-2026-10-29.md", mixed(36));
        assert_eq!(scan(&in_name), Some(Rule::HighEntropy), "{in_name}");
        let segment = format!("HANDOVER-{}-2026-10-29.md", mixed(40));
        assert_eq!(scan(&segment), Some(Rule::HighEntropy));
        let beside_extension = format!("docs/{}.md", mixed(40));
        assert_eq!(scan(&beside_extension), Some(Rule::HighEntropy));
    }

    /// A leading, trailing or doubled separator is not a split: plain-looking runs that fire alone still fire.
    #[test]
    fn a_lone_separator_does_not_make_a_name() {
        let run = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijk9";
        assert_eq!(scan(run), Some(Rule::HighEntropy));
        for text in [
            format!("{run}_"),
            format!("_{run}"),
            format!("{run}-"),
            format!("{}__{}", &run[..20], &run[20..]),
            format!("{}_", mixed(40)),
            format!("{}-", mixed(40)),
        ] {
            assert_eq!(scan(&text), Some(Rule::HighEntropy), "{text}");
        }
    }

    /// Random tokens whose segments each have at most one letter/digit boundary: only the case rule keeps them.
    #[test]
    fn random_case_letters_are_not_words() {
        for token in [
            "Y_JwYXCAn0-oemF-xFsUhSaLFzKwdG89",
            "LXFz7_MxnbGasqZpJKugqmuRiNhNbNeX",
            "ffRksnXaIaBHStNjIL-RWL-4QJyCvOwa",
        ] {
            assert_eq!(scan(token), Some(Rule::HighEntropy), "{token}");
        }
    }

    /// Pins the exemption's reach: 1,000 seeded random base64url tokens of 40 bytes, each split by one '-' in the
    /// middle. 999 fire (the one that does not lacks a digit or a case, as before s23); a wider exemption lowers it.
    #[test]
    fn split_random_tokens_still_fire_at_the_measured_rate() {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut s: u64 = 0x005E_C2E7_5A7E_0023;
        let mut fired = 0;
        for _ in 0..1000 {
            let mut token = String::new();
            for i in 0..40 {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                if i == 20 {
                    token.push('-');
                }
                token.push(char::from(ALPHABET[(s >> 58) as usize]));
            }
            fired += usize::from(high_entropy(token.as_bytes()));
        }
        assert_eq!(fired, 999);
    }

    /// F146 (a user report): long camelCase and PascalCase column names are names, not tokens, alone, after a table
    /// name or in a sentence. Each name in `caught_before` passes the entropy rule alone, so it was flagged before
    /// [`camel_name`]; the packet's two examples were not (one is 31 bytes, the other under 4.2 bits a byte).
    #[test]
    fn camel_case_names_are_clean() {
        let caught_before = [
            "totalOutstandingInvoiceAmountExcludingGST2024",
            "quarterlyRevenueByRegion2026ForecastJuly",
            "customerWebLoginHistory2025TrackedByZone",
            "PayrollTaxJurisdiction2026FilingBankQueue",
            "monthlyActiveUserCountByDeviceType2025",
            "shippingWarehouseZoneCapacity2026PeakWeek",
        ];
        for name in caught_before {
            assert!(
                random_stats(name.as_bytes()),
                "{name} would fire without the exemption"
            );
        }
        let in_text = [
            "billing.customerWebLoginHistory2025TrackedByZone",
            "SELECT totalOutstandingInvoiceAmountExcludingGST2024 FROM invoices",
            "customerAccountBalanceHistory2024Total",
            "ExternalBillingSystemCustomerID",
        ];
        for name in caught_before.into_iter().chain(in_text) {
            assert_eq!(scan(name), None, "{name}");
            assert_eq!(field_suspect("column", name), None, "{name}");
        }
    }

    /// F146: the exempt `customerWebLoginHistory2025TrackedByZone` changed to break one part of the rule each is
    /// still a candidate, and so are an acronym run straight into a capitalised word and short random-case words. The
    /// acronym-cap and 3-word fixtures meet every other clause, so each fails only when its own clause is removed
    /// (plants `cap` and `words`).
    #[test]
    fn camel_like_runs_that_break_the_rule_still_fire() {
        for token in [
            // Two capitals in a row outside one acronym.
            "customerWebLoginHIstory2025TrackedByZone",
            // An acronym of 6.
            "customerWebLoginHISTORY2025TrackedByZone",
            // A digit run of 5.
            "customerWebLoginHistory98765TrackedByZone",
            // Three acronyms, every other clause met (review, R286): only the cap of 2 keeps it a candidate.
            "customerID2025SSN2026TFN2027TrackedByZone",
            // Two words, every other clause met (review, R286): only the 3-word floor keeps it a candidate.
            "Abcdefghijklmnopqrstuvwxyzabcdef2024",
            // An acronym straight before a capitalised word is one word (UTCFor), as the rule splits.
            "lastModifiedTimestampUTCForAuditTrail2025",
            // Words under 4 bytes on average.
            "aBcDeFgHiJkLmNoPqRsTuVwXyZ0123456789",
        ] {
            assert_eq!(scan(token), Some(Rule::HighEntropy), "{token}");
        }
    }

    /// F146: the camelCase exemption's reach. 10,000 seeded uniformly random base64url tokens at each length, with
    /// no split added: how many the entropy rule alone catches ([`random_stats`]), how many of those with no `-` or
    /// `_` the camelCase exemption lets through, and how many fire. Measured: none exempt at 32, 40 or 64 bytes (the
    /// rule asks under 0.1% at 32). A wider exemption raises the middle number.
    #[test]
    fn unsplit_random_tokens_are_not_camel_names() {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut s: u64 = 0x00F1_46CA_3E1C_A5E5;
        let mut got = Vec::new();
        for len in [32, 40, 64] {
            let (mut candidates, mut exempt, mut fired) = (0, 0, 0);
            for _ in 0..10_000 {
                let mut token = Vec::with_capacity(len);
                for _ in 0..len {
                    s ^= s << 13;
                    s ^= s >> 7;
                    s ^= s << 17;
                    token.push(ALPHABET[(s >> 58) as usize]);
                }
                let caught = random_stats(&token);
                let unsplit = !token.iter().any(|c| matches!(c, b'-' | b'_'));
                candidates += usize::from(caught);
                exempt += usize::from(caught && unsplit && camel_name(&token));
                fired += usize::from(high_entropy(&token));
            }
            got.push((len, candidates, exempt, fired));
        }
        assert_eq!(
            got,
            [
                (32, 9892, 0, 9885),
                (40, 9990, 0, 9986),
                (64, 10000, 0, 10000)
            ]
        );
        assert!(got[0].2 * 1000 < got[0].1, "under 0.1% at 32 bytes");
    }

    /// Security's probe set, rebuilt from its verdict: 18 realistic secrets caught and 12 benign texts clean.
    #[test]
    fn the_security_probe_set_holds() {
        let hex32 = fake("", "3f9a0c7e1b2d4568", 32);
        let caught = [
            format!("AZURE_OPENAI_API_KEY={hex32}"),
            format!("api-key: {hex32}"),
            format!(
                "DefaultEndpointsProtocol=https;AccountName=acct;AccountKey={}==;",
                mixed(86)
            ),
            String::from("Server=tcp:db.example.net;Database=app;User Id=sa;Password=Pa55word!x;"),
            format!(
                "https://acct.blob.core.windows.net/c?sv=2022-11-02&se=2026&sig={}%3D",
                mixed(43)
            ),
            String::from("postgres://admin:s3cr3tPass@db.internal:5432/app"),
            format!("Authorization: Bearer {}", mixed(40)),
            format!("export GH={}", fake("ghp_", "q7Z3x9K2", 36)),
            format!("aws_access_key_id = {}", fake("AKIA", "Q7Z3X9K2", 16)),
            format!(
                "-----BEGIN OPENSSH {}-----\nb3BlbnNzaC1rZXktdjEAAAAA",
                "PRIVATE KEY"
            ),
            format!("OPENAI_KEY {}", fake("sk-proj-", "q7Z3x9K2", 48)),
            format!("client_secret: {}", fake("", "q1w2e3r4t5y6u7i8", 34)),
            format!("SLACK={}", fake("xoxb-", "1234-abcd", 40)),
            format!(
                "{}.{}.{}",
                fake("eyJ", "hbGciOi", 20),
                fake("eyJ", "zdWIiOi", 20),
                mixed(43)
            ),
            String::from("password=Tr0ub4dor&3x"),
            format!("token={}", fake("", "q1w2e3r4t5y6u7i8", 24)),
            format!("aws_secret_access_key = {}", mixed(40)),
            String::from("my AWS secret is wJalrXUtnFEMI/K7MDENG/bPxRfiCYzq8Yt3Lm2N"),
        ];
        for text in &caught {
            assert!(scan(text).is_some(), "missed {text}");
        }
        let clean = [
            "fixed in commit 3b18e512dba79e4c8300dd08aeb37f8e728b8dad",
            "node 01JC8ZK3Q7M2X9V4B5R6T8W0YH links to 01JC8ZK3Q7M2X9V4B5R6T8W0YJ",
            "uuid 123e4567-e89b-12d3-a456-426614174000",
            "the password is in the vault",
            "see https://learn.microsoft.com/en-us/azure/ai-services/openai/reference",
            "crates/umx-core/src/secrets.rs",
            "api_key: <redacted>",
            "api_key=${API_KEY}",
            "Tokens expire hourly; send a Bearer token in the Authorization header.",
            "sha256 9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08",
            "short base64 aGVsbG8gd29ybGQ=",
            "the token budget is 8192 per request",
        ];
        for text in clean {
            assert_eq!(scan(text), None, "{text}");
        }
    }
}

#[cfg(test)]
mod aws_secret_tests {
    use super::*;

    #[test]
    fn a_forty_byte_key_counts_only_near_aws_or_secret_and_only_at_forty() {
        let key = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYzq8Yt3Lm2N";
        assert_eq!(key.len(), 40);
        assert_eq!(
            scan(&alloc::format!("aws key {key}")),
            Some(Rule::AwsSecretKey)
        );
        assert_eq!(
            scan(&alloc::format!("a value {key}")),
            None,
            "no aws or secret nearby"
        );
        let far = alloc::format!("aws {} {key}", "word ".repeat(16));
        assert_eq!(scan(&far), None, "more than 64 bytes away");
        assert_eq!(
            scan(&alloc::format!("aws key {key}X")),
            None,
            "41 bytes is not one"
        );
        assert_eq!(
            scan(&alloc::format!("SECRET {key} for prod")),
            Some(Rule::AwsSecretKey)
        );
        // The benign texts of the scan's own tests stay clean, and so do these.
        for text in [
            "the aws commit 3b18e512dba79e4c8300dd08aeb37f8e728b8dad fixed it",
            "the secret handling lives in docs/project/workshops/ConceptGraph2/Security",
        ] {
            assert_eq!(scan(text), None, "{text}");
        }
    }
}
