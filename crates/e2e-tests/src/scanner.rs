//! Byte-encoding scanner for the mechanical no-key assertion (spec §10.3
//! leg d): searches a byte haystack for EVERY plausible encoding of a felt —
//! minimal hex, 64-padded hex (with/without 0x, both cases), decimal ASCII,
//! raw 32-byte BE, raw 32-byte LE, and base64 of the BE bytes.
//!
//! The detector's own sensitivity is proven by the compat leg: the same
//! scanner MUST find the viewing key in a compat-mode request capture.

use starknet_types_core::felt::Felt;

fn minimal_hex_no_prefix(f: &Felt) -> String {
    let hex = hex::encode(f.to_bytes_be());
    let trimmed = hex.trim_start_matches('0');
    if trimmed.is_empty() {
        "0".to_owned()
    } else {
        trimmed.to_owned()
    }
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_encode(data: &[u8]) -> String {
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            B64[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            B64[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// All encodings of `f` a leak could plausibly take.
pub fn encodings(f: &Felt) -> Vec<Vec<u8>> {
    let be = f.to_bytes_be().to_vec();
    let mut le = be.clone();
    le.reverse();
    let min = minimal_hex_no_prefix(f);
    let padded = hex::encode(f.to_bytes_be());
    let decimal = {
        // felt -> decimal string via u128 pieces or big formatting
        f.to_biguint().to_string()
    };
    let mut out = vec![
        format!("0x{min}").into_bytes(),
        min.clone().into_bytes(),
        format!("0x{padded}").into_bytes(),
        padded.clone().into_bytes(),
        format!("0x{}", min.to_uppercase()).into_bytes(),
        min.to_uppercase().into_bytes(),
        padded.to_uppercase().into_bytes(),
        decimal.into_bytes(),
        be.clone(),
        le,
        base64_encode(&be).into_bytes(),
    ];
    out.dedup();
    out
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || needle.len() > haystack.len() {
        return false;
    }
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// Which encodings of `f` occur in `haystack`. Empty = clean.
pub fn find_felt(haystack: &[u8], f: &Felt, label: &str) -> Vec<String> {
    let names = [
        "0x-minimal-hex",
        "minimal-hex",
        "0x-padded-hex",
        "padded-hex",
        "0x-minimal-hex-upper",
        "minimal-hex-upper",
        "padded-hex-upper",
        "decimal",
        "be-bytes",
        "le-bytes",
        "base64-be",
    ];
    encodings(f)
        .iter()
        .zip(names.iter())
        .filter(|(needle, _)| contains(haystack, needle))
        .map(|(_, name)| format!("{label}:{name}"))
        .collect()
}

/// Scan for a set of secrets; returns every hit (empty = clean).
pub fn scan(haystack: &[u8], secrets: &[(Felt, String)]) -> Vec<String> {
    let mut hits = Vec::new();
    for (f, label) in secrets {
        hits.extend(find_felt(haystack, f, label));
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_padded_and_minimal_hex() {
        let f = Felt::from_hex("0xa11ce").unwrap();
        let hay = br#"{"viewing_key":"0xa11ce"}"#;
        assert!(!find_felt(hay, &f, "k").is_empty());
        let hay2 = format!("key={}", hex::encode(f.to_bytes_be()));
        assert!(!find_felt(hay2.as_bytes(), &f, "k").is_empty());
    }

    #[test]
    fn clean_haystack_is_clean() {
        let f = Felt::from_hex("0xdeadbeefcafe1234567890").unwrap();
        assert!(find_felt(b"nothing to see here 0x1234", &f, "k").is_empty());
    }
}
