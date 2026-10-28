//! Hop-by-hop header removal for the two relays (#3338).
//!
//! RFC 9110 §7.6.1 requires an intermediary to remove the `Connection` header
//! and every field its value nominates before forwarding, together with the
//! fields that only ever describe one connection. The control plane's `/gw`
//! relay and the gateway's MCP relay both call [`strip`], so the rule has one
//! home. spec-ref: http-semantics

use reqwest::header::{HeaderMap, HeaderName, CONNECTION};

/// The fields that describe a single connection and are never relayed, whether
/// or not `Connection` names them (RFC 9110 §7.6.1, §6.6.2, §11.7.1).
const HOP_BY_HOP: [&str; 8] = [
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "proxy-connection",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

/// Removes `Connection`, every field named in its value(s) and the fixed
/// hop-by-hop set from `headers`. Used on requests and on responses.
pub fn strip(headers: &mut HeaderMap) {
    // read as bytes: a value with an obs-text byte is not `to_str`-able, and
    // dropping the whole value would keep every field it nominates
    let nominated: Vec<HeaderName> = headers
        .get_all(CONNECTION)
        .iter()
        .flat_map(|value| value.as_bytes().split(|byte| *byte == b','))
        .filter_map(|token| HeaderName::from_bytes(token.trim_ascii()).ok())
        .collect();
    headers.remove(CONNECTION);
    for name in nominated {
        headers.remove(name);
    }
    for name in HOP_BY_HOP {
        headers.remove(name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderValue;

    #[test]
    fn removes_connection_and_the_fields_it_nominates() {
        let mut headers = HeaderMap::new();
        headers.append(CONNECTION, HeaderValue::from_static("close, X-Internal"));
        headers.append(CONNECTION, HeaderValue::from_static("x-second"));
        headers.insert("x-internal", HeaderValue::from_static("secret"));
        headers.insert("x-second", HeaderValue::from_static("secret"));
        headers.insert("x-kept", HeaderValue::from_static("kept"));
        strip(&mut headers);
        assert!(headers.get(CONNECTION).is_none());
        assert!(headers.get("x-internal").is_none());
        assert!(headers.get("x-second").is_none());
        assert_eq!(headers.get("x-kept").unwrap(), "kept");
    }

    #[test]
    fn removes_trailer_and_the_fixed_set() {
        let mut headers = HeaderMap::new();
        for name in HOP_BY_HOP {
            headers.insert(name, HeaderValue::from_static("x"));
        }
        headers.insert("trailers", HeaderValue::from_static("kept"));
        headers.insert("content-type", HeaderValue::from_static("application/json"));
        strip(&mut headers);
        for name in HOP_BY_HOP {
            assert!(headers.get(name).is_none(), "{name} survived");
        }
        assert!(headers.get("content-type").is_some());
        // `trailers` is a value of `TE`, not a field: it is no hop-by-hop name
        assert!(headers.get("trailers").is_some());
    }

    #[test]
    fn a_non_ascii_byte_in_connection_does_not_hide_the_other_tokens() {
        let mut headers = HeaderMap::new();
        headers.insert(
            CONNECTION,
            HeaderValue::from_bytes(b"x-internal, \x80, x-second").unwrap(),
        );
        headers.insert("x-internal", HeaderValue::from_static("secret"));
        headers.insert("x-second", HeaderValue::from_static("secret"));
        strip(&mut headers);
        assert!(headers.get("x-internal").is_none());
        assert!(headers.get("x-second").is_none());
    }

    #[test]
    fn ignores_unparsable_nominations() {
        let mut headers = HeaderMap::new();
        headers.insert(CONNECTION, HeaderValue::from_static(", bad name, x-a"));
        headers.insert("x-a", HeaderValue::from_static("1"));
        strip(&mut headers);
        assert!(headers.get("x-a").is_none());
    }
}
