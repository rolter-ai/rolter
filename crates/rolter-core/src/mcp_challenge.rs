//! Reading an RFC 6750 / RFC 9728 `WWW-Authenticate` challenge (#1417).
//!
//! An MCP server that wants a client to authorize, or to authorize again for
//! more, says so in the challenge of its `401` or `403`: `resource_metadata`
//! points at its protected resource metadata, `scope` names what the request
//! needed, and `error="insufficient_scope"` marks a token that was valid but
//! too narrow. The control plane reads it to choose the scopes of an
//! authorization request; the gateway reads it to turn an upstream's refusal
//! into the step-up answer it gives the caller.
//!
//! The parser is written by hand because the value is one quoted parameter
//! among several and the scheme need not be `Bearer`, and it is shared so the
//! two planes cannot disagree about what a challenge says.

/// Most scopes taken from one challenge or metadata document. A hostile server
/// must not be able to make an authorization URL arbitrarily long.
pub const MAX_CHALLENGE_SCOPES: usize = 32;

/// Longest single scope token kept.
pub const MAX_SCOPE_LEN: usize = 128;

/// The value of parameter `key` in a challenge, quoted or bare.
///
/// Only a whole parameter counts: `x_scope` is a different parameter that
/// happens to end in `scope`. A quoted value ends at the first `"`.
pub fn challenge_param(header: &str, key: &str) -> Option<String> {
    let mut rest = header;
    while let Some(index) = rest.find(key) {
        let (before, at_key) = rest.split_at(index);
        let boundary = before
            .chars()
            .next_back()
            .is_none_or(|c| c == ' ' || c == ',' || c == '\t');
        let after = at_key[key.len()..].trim_start();
        rest = &at_key[key.len()..];
        if !boundary {
            continue;
        }
        let Some(value) = after.strip_prefix('=') else {
            continue;
        };
        let value = value.trim_start();
        let parsed = match value.strip_prefix('"') {
            Some(quoted) => quoted.split('"').next().map(str::to_string),
            None => value
                .split([',', ' ', '\t'])
                .next()
                .filter(|v| !v.is_empty())
                .map(str::to_string),
        };
        if let Some(parsed) = parsed.filter(|v| !v.is_empty()) {
            return Some(parsed);
        }
    }
    None
}

/// Whether `token` is a scope token as RFC 6749 §3.3 writes it: printable ASCII
/// without a space, `"` or `\`, bounded in length.
pub fn is_scope_token(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= MAX_SCOPE_LEN
        && token
            .bytes()
            .all(|b| b == 0x21 || (0x23..=0x5b).contains(&b) || (0x5d..=0x7e).contains(&b))
}

/// The valid, distinct scope tokens of a space-delimited list, in order and
/// capped at [`MAX_CHALLENGE_SCOPES`]. A token that is not valid is dropped
/// rather than failing the whole list.
pub fn parse_scopes(list: &str) -> Vec<String> {
    let mut scopes: Vec<String> = Vec::new();
    for token in list.split_whitespace().filter(|t| is_scope_token(t)) {
        if !scopes.iter().any(|seen| seen == token) {
            scopes.push(token.to_string());
        }
        if scopes.len() == MAX_CHALLENGE_SCOPES {
            break;
        }
    }
    scopes
}

/// The `scope` a challenge asks for, as scope tokens.
pub fn challenge_scopes(header: &str) -> Vec<String> {
    challenge_param(header, "scope")
        .map(|list| parse_scopes(&list))
        .unwrap_or_default()
}

/// Whether the challenge reports a token that was valid but lacked a scope.
pub fn is_insufficient_scope(header: &str) -> bool {
    challenge_param(header, "error").is_some_and(|error| error == "insufficient_scope")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_quoted_and_bare_parameters_in_any_position() {
        let header = r#"Bearer error="insufficient_scope", scope="files:read files:write", resource_metadata=https://m.example/prm"#;
        assert_eq!(
            challenge_param(header, "scope").as_deref(),
            Some("files:read files:write")
        );
        assert_eq!(
            challenge_param(header, "resource_metadata").as_deref(),
            Some("https://m.example/prm")
        );
        assert_eq!(
            challenge_param(header, "error").as_deref(),
            Some("insufficient_scope")
        );
        assert!(is_insufficient_scope(header));
    }

    #[test]
    fn a_parameter_that_merely_ends_in_the_name_is_another_parameter() {
        assert_eq!(challenge_param(r#"Bearer x_scope="a""#, "scope"), None);
        assert_eq!(challenge_param("Bearer", "scope"), None);
        assert_eq!(challenge_param(r#"Bearer scope="""#, "scope"), None);
        assert!(!is_insufficient_scope(r#"Bearer error="invalid_token""#));
    }

    #[test]
    fn scopes_are_validated_deduplicated_and_bounded() {
        assert_eq!(parse_scopes("a b  a c"), ["a", "b", "c"]);
        // a token with a quote or backslash is not a scope token
        assert_eq!(
            parse_scopes(r#"ok bad"quote back\slash ok2"#),
            ["ok", "ok2"]
        );
        assert!(parse_scopes(&"x".repeat(MAX_SCOPE_LEN + 1)).is_empty());
        let many: String = (0..100).map(|n| format!("s{n} ")).collect();
        assert_eq!(parse_scopes(&many).len(), MAX_CHALLENGE_SCOPES);
        assert_eq!(
            challenge_scopes(r#"Bearer error="insufficient_scope", scope="repo read:org""#),
            ["repo", "read:org"]
        );
        assert!(challenge_scopes("Bearer").is_empty());
    }
}
