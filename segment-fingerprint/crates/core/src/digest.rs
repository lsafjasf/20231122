//! Deterministic hashing helpers.
//!
//! All digests are SHA-256 over canonical UTF-8 JSON (two-space indentation,
//! struct fields in declaration order, arrays in pipeline order). The same
//! encoder is used when first writing evidence and when re-verifying it, so a
//! colleague can independently reproduce every hash.

use serde::Serialize;
use sha2::{Digest, Sha256};

pub fn sha256_hex_bytes(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex_encode(&hasher.finalize())
}

pub fn sha256_hex_str(text: &str) -> String {
    sha256_hex_bytes(text.as_bytes())
}

pub fn canonical_json<T: Serialize>(value: &T) -> String {
    let mut buf = Vec::with_capacity(256);
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b"  ");
    let mut serializer = serde_json::Serializer::with_formatter(&mut buf, formatter);
    value
        .serialize(&mut serializer)
        .expect("serialization into a Vec<u8> cannot fail");
    buf.push(b'\n');
    String::from_utf8(buf).expect("serde_json always emits valid UTF-8")
}

pub fn digest_canonical<T: Serialize>(value: &T) -> String {
    sha256_hex_str(&canonical_json(value))
}

pub fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn known_empty_sha256() {
        assert_eq!(
            sha256_hex_bytes(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn canonical_format_is_stable_and_terminated() {
        let mut map = BTreeMap::new();
        map.insert("b", 2u32);
        map.insert("a", 1u32);
        let text = canonical_json(&map);
        assert_eq!(text, "{\n  \"a\": 1,\n  \"b\": 2\n}\n");
        assert_eq!(
            digest_canonical(&map),
            sha256_hex_str(&canonical_json(&map))
        );
    }
}
