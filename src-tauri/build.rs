fn main() {
    // `signing.rs` bakes the update public key into the binary with
    // `option_env!`. Cargo does not track `env!`/`option_env!` reads, so a
    // cached target directory would otherwise keep a build made without the key
    // (or with another key). Ask for a rebuild whenever it changes.
    println!("cargo:rerun-if-env-changed=SPLITLINE_UPDATE_PUBKEY");
    tauri_build::build()
}
