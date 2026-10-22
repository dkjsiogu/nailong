use std::env;
use std::fs;
use std::path::PathBuf;

/// Keep this in sync with `builtin::MASK`. This is obfuscation, not
/// encryption: it only prevents the key from appearing verbatim in `strings`.
const MASK: &[u8] = b"nailong-cf-v1";

fn main() {
    println!("cargo:rerun-if-env-changed=NAILONG_CURSEFORGE_API_KEY");
    let key = env::var("NAILONG_CURSEFORGE_API_KEY").unwrap_or_default();
    let key = key.trim().as_bytes();
    let encoded: Vec<u8> = key
        .iter()
        .enumerate()
        .map(|(index, byte)| byte ^ MASK[index % MASK.len()] ^ (index as u8).wrapping_mul(31))
        .collect();

    let mut source = String::from("pub const OBFUSCATED_KEY: &[u8] = &[");
    for (index, byte) in encoded.iter().enumerate() {
        if index % 16 == 0 {
            source.push_str("\n    ");
        }
        source.push_str(&format!("{byte},"));
    }
    if !encoded.is_empty() {
        source.push('\n');
    }
    source.push_str("];\n");

    let out = env::var("OUT_DIR").expect("OUT_DIR");
    fs::write(PathBuf::from(out).join("builtin_key.rs"), source).expect("write builtin_key.rs");
}
