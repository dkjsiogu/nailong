fn main() {
    // `config.rs` bakes this optional key with option_env!; make Cargo rebuild
    // when a release pipeline adds or rotates it.
    println!("cargo:rerun-if-env-changed=NAILONG_CURSEFORGE_API_KEY");
}
