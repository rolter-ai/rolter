//! RFC 6238 time-based one-time passwords, and the base32 alphabet the
//! `otpauth://` URI carries the shared secret in (#1078).
//!
//! ## Why SHA-1
//!
//! RFC 6238 permits SHA-1, SHA-256 and SHA-512, and SHA-1 is the weakest of
//! the three. It is nonetheless the only interoperable choice here. Google
//! Authenticator — still the most widely installed enrolment target — ignores
//! the `algorithm` parameter in the `otpauth://` URI and computes SHA-1
//! regardless, so a deployment that armed SHA-256 would hand half its
//! operators a factor that silently never verifies. The construction is HMAC,
//! not a bare digest, so SHA-1's collision weaknesses do not apply, and the
//! secret is 160 bits of CSPRNG output; the practical attack on a TOTP factor
//! is guessing a 6-digit code, which is what the replay and throttle rules
//! below defend.
//!
//! ## What this module does not do
//!
//! It does not remember anything. Replay defence (a step may be spent only
//! once) needs storage and therefore lives with the store; this module only
//! reports *which* step a presented code matched, so the caller can compare it
//! against the last one it accepted.

use hmac::{Hmac, KeyInit, Mac};
use sha1::Sha1;

/// seconds per TOTP step; 30 is the RFC default and what every authenticator
/// assumes when the URI omits `period`
pub const STEP_SECONDS: u64 = 30;

/// digits in a generated code; 6 is the RFC default and, again, what
/// authenticators assume
pub const DIGITS: u32 = 6;

/// how many steps either side of the current one are accepted, to absorb clock
/// skew between the server and the enrolling phone. One step each way is the
/// usual compromise: it forgives ±30s of drift while keeping the window a
/// guesser has to hit at three codes rather than the five a ±2 window opens.
pub const SKEW_STEPS: u64 = 1;

/// secret length in bytes. RFC 4226 requires at least 128 bits and recommends
/// 160, which is also exactly one SHA-1 block's worth of key material
pub const SECRET_BYTES: usize = 20;

/// RFC 4648 base32 alphabet, which is the encoding `otpauth://` secrets use.
const BASE32_ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// Encode bytes as unpadded RFC 4648 base32.
///
/// Unpadded on purpose: authenticator apps accept it, and a `=` in a URI query
/// parameter is one more thing for a hand-copied secret to lose.
pub fn base32_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(5) * 8);
    for chunk in bytes.chunks(5) {
        let mut buf = [0u8; 5];
        buf[..chunk.len()].copy_from_slice(chunk);
        let bits = u64::from_be_bytes([0, 0, 0, buf[0], buf[1], buf[2], buf[3], buf[4]]);
        // 8 base32 characters cover 40 bits; a short final chunk emits only
        // the characters its bytes actually reach into
        let chars = (chunk.len() * 8).div_ceil(5);
        for i in 0..chars {
            let shift = 35 - i * 5;
            out.push(BASE32_ALPHABET[((bits >> shift) & 0x1f) as usize] as char);
        }
    }
    out
}

/// Decode unpadded or padded RFC 4648 base32, case-insensitively.
///
/// Returns `None` on any character outside the alphabet. Lenient about case
/// and padding because the input is frequently a human retyping a secret.
pub fn base32_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 5 / 8);
    let mut acc: u32 = 0;
    let mut bits: u32 = 0;
    for c in text.chars().filter(|c| *c != '=' && !c.is_whitespace()) {
        let upper = c.to_ascii_uppercase() as u8;
        let value = BASE32_ALPHABET.iter().position(|b| *b == upper)? as u32;
        acc = (acc << 5) | value;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// The HOTP value for one counter (RFC 4226 §5.3), truncated to [`DIGITS`].
fn hotp(secret: &[u8], counter: u64) -> u32 {
    // `new_from_slice` only rejects a key length HMAC cannot pad, which for
    // this construction is none of them
    let mut mac = <Hmac<Sha1>>::new_from_slice(secret).expect("hmac accepts any key length");
    mac.update(&counter.to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let offset = (digest[digest.len() - 1] & 0x0f) as usize;
    let binary = u32::from_be_bytes([
        digest[offset] & 0x7f,
        digest[offset + 1],
        digest[offset + 2],
        digest[offset + 3],
    ]);
    binary % 10u32.pow(DIGITS)
}

/// The step number a unix timestamp falls in.
pub fn step_at(unix_seconds: u64) -> u64 {
    unix_seconds / STEP_SECONDS
}

/// The code for a given secret and step, zero-padded to [`DIGITS`].
pub fn code_at_step(secret: &[u8], step: u64) -> String {
    format!("{:0width$}", hotp(secret, step), width = DIGITS as usize)
}

/// Check a presented code against the steps around `now`, returning the step
/// it matched.
///
/// The caller must reject a step it has already accepted: this function has no
/// memory, and without that check a code stays valid for its whole window, so
/// one shoulder-surfed code is replayable. Comparison is constant-time in the
/// code's contents, and every candidate step is evaluated — no early return —
/// so the response time does not narrow down which step matched.
pub fn verify_at(secret: &[u8], presented: &str, unix_seconds: u64) -> Option<u64> {
    let presented = presented.trim();
    if presented.len() != DIGITS as usize || !presented.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let current = step_at(unix_seconds);
    let mut matched = None;
    for offset in -(SKEW_STEPS as i64)..=(SKEW_STEPS as i64) {
        let Some(step) = current.checked_add_signed(offset) else {
            continue;
        };
        if crate::verify_key(presented, &code_at_step(secret, step)) {
            matched = Some(step);
        }
    }
    matched
}

/// Build the `otpauth://totp/...` URI an authenticator app enrols from.
///
/// `issuer` and `account` are percent-encoded, and the issuer is repeated as a
/// query parameter as well as in the label: apps disagree about which one they
/// read, and one that reads neither shows the account as belonging to nobody.
pub fn otpauth_uri(issuer: &str, account: &str, secret: &[u8]) -> String {
    let label = format!(
        "{}:{}",
        percent_encode(issuer.trim()),
        percent_encode(account.trim())
    );
    format!(
        "otpauth://totp/{label}?secret={}&issuer={}&algorithm=SHA1&digits={DIGITS}&period={STEP_SECONDS}",
        base32_encode(secret),
        percent_encode(issuer.trim()),
    )
}

/// Percent-encode everything outside the unreserved set (RFC 3986 §2.3).
///
/// Deliberately strict rather than a URI-component encoder: an issuer or email
/// is arbitrary operator text, and `:` `/` `?` `#` `&` all change what the URI
/// means if they reach it raw.
fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 6238 appendix B, SHA-1 rows. The published vectors are 8-digit, so
    /// they are compared against the low 6 digits of each.
    #[test]
    fn matches_the_rfc_6238_test_vectors() {
        // the RFC's SHA-1 seed is the ASCII "12345678901234567890"
        let secret = b"12345678901234567890";
        for (unix, expected8) in [
            (59u64, "94287082"),
            (1111111109, "07081804"),
            (1111111111, "14050471"),
            (1234567890, "89005924"),
            (2000000000, "69279037"),
            (20000000000, "65353130"),
        ] {
            let expected = &expected8[expected8.len() - DIGITS as usize..];
            assert_eq!(
                code_at_step(secret, step_at(unix)),
                expected,
                "rfc 6238 vector at t={unix}"
            );
        }
    }

    #[test]
    fn base32_round_trips_and_matches_rfc_4648_vectors() {
        // RFC 4648 §10, with the padding stripped
        for (plain, encoded) in [
            ("", ""),
            ("f", "MY"),
            ("fo", "MZXQ"),
            ("foo", "MZXW6"),
            ("foob", "MZXW6YQ"),
            ("fooba", "MZXW6YTB"),
            ("foobar", "MZXW6YTBOI"),
        ] {
            assert_eq!(base32_encode(plain.as_bytes()), encoded, "encode {plain:?}");
            assert_eq!(
                base32_decode(encoded).as_deref(),
                Some(plain.as_bytes()),
                "decode {encoded:?}"
            );
        }
    }

    #[test]
    fn base32_decode_is_lenient_about_case_padding_and_spacing() {
        let expected = Some(b"foobar".to_vec());
        assert_eq!(base32_decode("MZXW6YTBOI"), expected);
        assert_eq!(base32_decode("mzxw6ytboi"), expected);
        assert_eq!(base32_decode("MZXW 6YTB OI"), expected);
        assert_eq!(base32_decode("MZXW6YTBOI======"), expected);
    }

    #[test]
    fn base32_decode_rejects_characters_outside_the_alphabet() {
        // 0, 1 and 8 are excluded from the alphabet precisely because they are
        // confusable, so they must be an error rather than silently skipped
        for bad in ["MZXW6YTB01", "MZXW6YTB8I", "MZXW6YTB!I"] {
            assert_eq!(base32_decode(bad), None, "{bad} should not decode");
        }
    }

    #[test]
    fn verify_accepts_the_current_step_and_one_either_side() {
        let secret = b"12345678901234567890";
        let now = 1_700_000_000u64;
        let current = step_at(now);
        for offset in [-1i64, 0, 1] {
            let step = current.wrapping_add_signed(offset);
            assert_eq!(
                verify_at(secret, &code_at_step(secret, step), now),
                Some(step),
                "offset {offset} should verify and report its own step"
            );
        }
    }

    #[test]
    fn verify_rejects_a_step_outside_the_skew_window() {
        let secret = b"12345678901234567890";
        let now = 1_700_000_000u64;
        let current = step_at(now);
        for offset in [-2i64, 2, 10] {
            let step = current.wrapping_add_signed(offset);
            assert_eq!(
                verify_at(secret, &code_at_step(secret, step), now),
                None,
                "offset {offset} is outside the window and must not verify"
            );
        }
    }

    #[test]
    fn verify_rejects_malformed_input() {
        let secret = b"12345678901234567890";
        let now = 1_700_000_000u64;
        for bad in ["", "12345", "1234567", "abcdef", "12 34 56", "١٢٣٤٥٦"] {
            assert_eq!(verify_at(secret, bad, now), None, "{bad:?} must not verify");
        }
    }

    #[test]
    fn verify_reports_the_matched_step_so_a_caller_can_refuse_a_replay() {
        // the property the store's replay check depends on: the same code
        // presented twice reports the same step both times, so a caller that
        // records the last accepted step can tell the second one apart
        let secret = b"12345678901234567890";
        let now = 1_700_000_000u64;
        let code = code_at_step(secret, step_at(now));
        assert_eq!(verify_at(secret, &code, now), verify_at(secret, &code, now));
        // and still the same step a few seconds later, inside the same window
        assert_eq!(
            verify_at(secret, &code, now),
            verify_at(secret, &code, now + 5)
        );
    }

    #[test]
    fn otpauth_uri_encodes_the_label_and_repeats_the_issuer() {
        let uri = otpauth_uri("rolter gateway", "ops@example.com", b"12345678901234567890");
        assert!(
            uri.starts_with("otpauth://totp/rolter%20gateway:ops%40example.com?"),
            "label must be percent-encoded: {uri}"
        );
        // derived rather than written out: the literal is the base32 of the
        // published RFC 6238 key and carries no secret, but it is high-entropy
        // enough that every credential scanner reports it. `base32_encode` is
        // pinned against the RFC 4648 vectors above, so this still fails if the
        // uri drops the secret or encodes the wrong bytes
        assert!(
            uri.contains(&format!(
                "secret={}",
                base32_encode(b"12345678901234567890")
            )),
            "{uri}"
        );
        assert!(uri.contains("issuer=rolter%20gateway"), "{uri}");
        assert!(uri.contains("algorithm=SHA1&digits=6&period=30"), "{uri}");
    }

    #[test]
    fn otpauth_uri_neutralises_separators_an_operator_could_put_in_an_issuer() {
        // an issuer containing `?`, `&` or `:` would otherwise inject query
        // parameters or split the label
        let uri = otpauth_uri("a:b?c&d/e#f", "u", b"12345678901234567890");
        assert!(
            !uri[.."otpauth://totp/".len() + 20].contains(['?', '&', '#']),
            "{uri}"
        );
        assert!(uri.contains("a%3Ab%3Fc%26d%2Fe%23f"), "{uri}");
    }
}
