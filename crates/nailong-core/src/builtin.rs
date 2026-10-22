//! Build-time CurseForge credential.
//!
//! Release builds get the approved project key through
//! `NAILONG_CURSEFORGE_API_KEY`; the build script XOR-obfuscates it so a casual
//! `strings` scan does not reveal it. This is deliberately not encryption:
//! anyone with the binary and a debugger can recover it.

use std::sync::OnceLock;

include!(concat!(env!("OUT_DIR"), "/builtin_key.rs"));

const MASK: &[u8] = b"nailong-cf-v1";

fn decode(encoded: &[u8]) -> Option<String> {
    if encoded.is_empty() {
        return None;
    }
    let mut plain = Vec::with_capacity(encoded.len());
    for (index, byte) in encoded.iter().enumerate() {
        plain.push(byte ^ MASK[index % MASK.len()] ^ (index as u8).wrapping_mul(31));
    }
    String::from_utf8(plain)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Runtime environment wins so a maintainer can test a rotated key without a
/// rebuild; otherwise use the obfuscated build-time default.
pub fn curseforge_api_key() -> Option<String> {
    static KEY: OnceLock<Option<String>> = OnceLock::new();
    KEY.get_or_init(|| {
        std::env::var("CURSEFORGE_API_KEY")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .or_else(|| decode(OBFUSCATED_KEY))
    })
    .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(plain: &[u8]) -> Vec<u8> {
        plain
            .iter()
            .enumerate()
            .map(|(index, byte)| byte ^ MASK[index % MASK.len()] ^ (index as u8).wrapping_mul(31))
            .collect()
    }

    #[test]
    fn obfuscation_round_trips_without_plaintext_bytes() {
        let plain = b"test-key";
        let encoded = encode(plain);
        assert!(!encoded.windows(plain.len()).any(|window| window == plain));
        assert_eq!(decode(&encoded).as_deref(), Some("test-key"));
        assert!(decode(&[]).is_none());
    }
}
