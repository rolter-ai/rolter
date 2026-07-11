//! Minimal `multipart/form-data` helpers for the audio endpoints.
//!
//! The gateway forwards multipart uploads (`/v1/audio/transcriptions`,
//! `/v1/audio/translations`) verbatim; it only needs to peek at the `model`
//! form field for auth, routing, budgets and rate limits. Rather than pull in a
//! full multipart crate we extract a single named text field with a small,
//! allocation-light scan over the already-buffered body.

/// Pull the `boundary=...` token out of a `multipart/form-data` content-type
/// header. Handles quoted and unquoted boundaries and ignores parameter order.
pub fn boundary(content_type: &str) -> Option<String> {
    let lower = content_type.to_ascii_lowercase();
    if !lower.starts_with("multipart/") {
        return None;
    }
    for part in content_type.split(';') {
        let part = part.trim();
        if let Some(rest) = part
            .strip_prefix("boundary=")
            .or_else(|| part.strip_prefix("Boundary="))
        {
            let rest = rest.trim().trim_matches('"');
            if !rest.is_empty() {
                return Some(rest.to_string());
            }
        }
    }
    None
}

/// Extract the value of a named text field from a multipart body. Returns
/// `None` when the field is absent, has a `filename` (i.e. is a file part, not
/// a text field), or the body is malformed. Values are decoded as UTF-8.
pub fn text_field(body: &[u8], boundary: &str, name: &str) -> Option<String> {
    let delim = format!("--{boundary}");
    let delim = delim.as_bytes();
    let mut pos = 0usize;

    while let Some(start) = find(&body[pos..], delim).map(|i| pos + i) {
        // step past the delimiter and its trailing CRLF (or `--` at the end)
        let mut cursor = start + delim.len();
        if body[cursor..].starts_with(b"--") {
            break; // closing delimiter
        }
        if body[cursor..].starts_with(b"\r\n") {
            cursor += 2;
        }
        // headers run until a blank line
        let headers_end = match find(&body[cursor..], b"\r\n\r\n") {
            Some(i) => cursor + i,
            None => break,
        };
        let headers = &body[cursor..headers_end];
        let value_start = headers_end + 4;
        // next delimiter bounds this part's value
        let value_end = match find(&body[value_start..], delim).map(|i| value_start + i) {
            Some(i) => i,
            None => break,
        };

        if header_matches_field(headers, name) {
            // strip the CRLF that separates the value from the next delimiter
            let mut end = value_end;
            if body[..end].ends_with(b"\r\n") {
                end -= 2;
            }
            return std::str::from_utf8(&body[value_start..end])
                .ok()
                .map(str::to_string);
        }
        pos = value_end;
    }
    None
}

/// True when the part headers declare `name="<name>"` and carry no `filename`
/// (so we only match genuine text fields, never uploaded files).
fn header_matches_field(headers: &[u8], name: &str) -> bool {
    let Ok(text) = std::str::from_utf8(headers) else {
        return false;
    };
    let lower = text.to_ascii_lowercase();
    if !lower.contains("content-disposition:") || lower.contains("filename=") {
        return false;
    }
    let needle = format!("name=\"{name}\"");
    text.contains(&needle)
}

/// First index of `needle` within `haystack`, or `None`.
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_boundary_variants() {
        assert_eq!(
            boundary("multipart/form-data; boundary=abc123"),
            Some("abc123".to_string())
        );
        assert_eq!(
            boundary("multipart/form-data; boundary=\"xy z\""),
            Some("xy z".to_string())
        );
        assert_eq!(boundary("application/json"), None);
    }

    #[test]
    fn extracts_model_text_field() {
        let b = "BOUND";
        let body = format!(
            "--{b}\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\nwhisper-1\r\n\
             --{b}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.mp3\"\r\n\
             Content-Type: audio/mpeg\r\n\r\n\x00\x01\x02binary\r\n--{b}--\r\n"
        );
        assert_eq!(
            text_field(body.as_bytes(), b, "model"),
            Some("whisper-1".to_string())
        );
        // a file part is never returned as a text field
        assert_eq!(text_field(body.as_bytes(), b, "file"), None);
        assert_eq!(text_field(body.as_bytes(), b, "missing"), None);
    }
}
