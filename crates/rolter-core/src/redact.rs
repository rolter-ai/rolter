//! Keeping credentials embedded in URLs out of logs and error text.
//!
//! Datastore and upstream URLs routinely carry secrets, either as userinfo
//! (`redis://:pw@host`) or as a query parameter (`?password=...`). Every
//! binary logs those URLs at startup or quotes them in a connection error, so
//! the one place that knows how to mask them lives here and is shared.

use std::borrow::Cow;

use url::Url;

/// What an unparsable URL is printed as. The raw text is never echoed: a string
/// that is not a URL may still be a password pasted into the wrong variable.
pub const INVALID_URL_PLACEHOLDER: &str = "<invalid url>";

/// What a masked credential is printed as.
pub const MASK: &str = "***";

/// Query parameter names whose value is a credential, matched as substrings of
/// the lowercased name so `db_password`, `api_key` and `X-Amz-Signature` are
/// all caught.
const SENSITIVE_QUERY_MARKERS: [&str; 8] = [
    "pass",
    "pwd",
    "secret",
    "token",
    "key",
    "sig",
    "auth",
    "credential",
];

fn is_sensitive_param(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    SENSITIVE_QUERY_MARKERS
        .iter()
        .any(|marker| name.contains(marker))
}

/// Mask the credentials of a parsed URL in place, reporting whether any were
/// present.
fn mask(parsed: &mut Url) -> bool {
    let mut masked = false;
    if !parsed.username().is_empty() || parsed.password().is_some() {
        // a fixed username so `user:pw@` and `:pw@` both print as `***@`
        let _ = parsed.set_username(MASK);
        let _ = parsed.set_password(None);
        masked = true;
    }
    if parsed.query().is_some() {
        let pairs: Vec<(String, String)> = parsed
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        if pairs.iter().any(|(k, _)| is_sensitive_param(k)) {
            let mut query = parsed.query_pairs_mut();
            query.clear();
            for (k, v) in &pairs {
                if is_sensitive_param(k) {
                    query.append_pair(k, MASK);
                } else {
                    query.append_pair(k, v);
                }
            }
            drop(query);
            masked = true;
        }
    }
    masked
}

/// A URL that is safe to log: scheme, host, port and path are kept, userinfo
/// and credential-named query values become `***`.
///
/// A URL with nothing to hide is returned verbatim. One that does not parse
/// becomes [`INVALID_URL_PLACEHOLDER`] rather than being echoed back.
pub fn redact_url(url: &str) -> String {
    redact_url_cow(url).into_owned()
}

fn redact_url_cow(url: &str) -> Cow<'_, str> {
    match Url::parse(url.trim()) {
        Ok(mut parsed) => {
            if mask(&mut parsed) {
                Cow::Owned(parsed.to_string())
            } else {
                Cow::Borrowed(url)
            }
        }
        Err(_) => Cow::Borrowed(INVALID_URL_PLACEHOLDER),
    }
}

/// Mask every `scheme://...` URL embedded in free text, such as the error a
/// HTTP client raises after quoting the URL it could not reach.
///
/// Text outside URLs is untouched. A URL-shaped span that does not parse is
/// replaced by [`INVALID_URL_PLACEHOLDER`].
pub fn redact_urls_in_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(sep) = rest.find("://") {
        let scheme_start = rest[..sep]
            .rfind(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')))
            .map_or(0, |i| i + 1);
        let after = sep + 3;
        let end = rest[after..]
            .find(|c: char| c.is_whitespace() || matches!(c, ')' | '"' | '\'' | '<' | '>' | ','))
            .map_or(rest.len(), |i| after + i);
        out.push_str(&rest[..scheme_start]);
        if scheme_start == sep {
            // no scheme before the separator, so this is not a URL
            out.push_str(&rest[scheme_start..after]);
            rest = &rest[after..];
            continue;
        }
        out.push_str(&redact_url_cow(&rest[scheme_start..end]));
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A credential generated per call, so no test writes one out.
    fn secret() -> String {
        format!("pw-{}", uuid::Uuid::new_v4())
    }

    #[test]
    fn user_and_password_are_masked() {
        let out = redact_url(&format!(
            "postgres://admin:{}@db.internal:5432/rolter",
            secret()
        ));
        assert_eq!(out, "postgres://***@db.internal:5432/rolter");
    }

    #[test]
    fn a_password_only_redis_url_is_masked() {
        let secret = secret();
        let out = redact_url(&format!("redis://:{secret}@cache:6379/0"));
        assert_eq!(out, "redis://***@cache:6379/0");
        assert!(!out.contains(&secret));
    }

    #[test]
    fn a_username_only_url_loses_the_username() {
        assert_eq!(redact_url("http://token123@ch:8123"), "http://***@ch:8123/");
    }

    #[test]
    fn a_url_without_userinfo_is_returned_verbatim() {
        assert_eq!(redact_url("redis://cache:6379"), "redis://cache:6379");
        assert_eq!(
            redact_url("http://clickhouse:8123/?database=rolter"),
            "http://clickhouse:8123/?database=rolter"
        );
    }

    #[test]
    fn credential_query_values_are_masked() {
        let secret = secret();
        let out = redact_url(&format!(
            "http://ch:8123/?user=default&password={secret}&database=logs"
        ));
        assert!(!out.contains(&secret), "{out}");
        assert!(out.contains("password=***"), "{out}");
        assert!(out.contains("user=default"), "{out}");
        assert!(out.contains("database=logs"), "{out}");
        let out = redact_url("https://h/x?X-Amz-Signature=abc&api_key=def");
        assert!(!out.contains("abc") && !out.contains("def"), "{out}");
    }

    #[test]
    fn userinfo_and_query_secrets_are_both_masked() {
        let out = redact_url("redis://u:pw1@h:6379/?password=pw2");
        assert!(!out.contains("pw1") && !out.contains("pw2"), "{out}");
    }

    #[test]
    fn an_invalid_url_never_prints_the_input() {
        let secret = secret();
        for raw in [
            format!("not a url {secret}"),
            secret.clone(),
            format!("redis://:{secret}@host:notaport"),
            String::new(),
        ] {
            let out = redact_url(&raw);
            assert_eq!(out, INVALID_URL_PLACEHOLDER, "input {raw:?}");
        }
    }

    #[test]
    fn text_redaction_masks_each_embedded_url() {
        assert_eq!(redact_urls_in_text("no url here"), "no url here");
        assert_eq!(
            redact_urls_in_text("post http://ch:8123/?q=a@b failed"),
            "post http://ch:8123/?q=a@b failed"
        );
        assert_eq!(
            redact_urls_in_text("a https://u:p@h/x and http://v@k"),
            "a https://***@h/x and http://***@k/"
        );
        let out =
            redact_urls_in_text("error sending request for url (redis://:pw@c:6379/0): refused");
        assert_eq!(
            out,
            "error sending request for url (redis://***@c:6379/0): refused"
        );
    }

    #[test]
    fn text_redaction_ignores_a_bare_separator() {
        assert_eq!(redact_urls_in_text("a :// b"), "a :// b");
    }
}
