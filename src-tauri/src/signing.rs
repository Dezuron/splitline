//! Detached signatures for application updates (minisign / Tauri signer format).
//!
//! The release pipeline signs `Splitline.exe` and the portable zip with a
//! minisign secret key and publishes `<asset>.sig` next to each asset. The
//! updater downloads the matching `.sig` and verifies it against the public key
//! compiled into the binary, before the running exe is replaced.
//!
//! The check is fail-closed on purpose: no key in the build, no `.sig` in the
//! release, a malformed signature or a signature made with another key all stop
//! the update. A compromised GitHub account (or a compromised release) must not
//! be enough to install an arbitrary executable with administrator rights.
//!
//! The public key comes from `SPLITLINE_UPDATE_PUBKEY` at build time. When the
//! variable is empty the updater is disabled instead of silently trusting the
//! release digest alone; there is deliberately no placeholder key.
//!
//! Rotation and manual verification are described in `SECURITY.md`.

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use minisign_verify::{PublicKey, Signature};

/// Compiled-in release public key (`RW...` base64, as printed by
/// `tauri signer generate` / `minisign -G`).
const EMBEDDED_PUBLIC_KEY: Option<&str> = option_env!("SPLITLINE_UPDATE_PUBKEY");

/// Shown when the build carries no public key: the updater stays off.
pub const MISSING_KEY_MESSAGE: &str = "Обновления отключены: не настроен ключ подписи";

/// Shown when a signature does not match the downloaded file.
pub const SIGNATURE_MISMATCH_MESSAGE: &str =
    "Обновление не прошло проверку подписи. Скачайте новую версию с GitHub вручную.";

/// Shown when the `.sig` body cannot be decoded (truncated, wrong format).
pub const CORRUPT_SIGNATURE_MESSAGE: &str =
    "Файл подписи обновления повреждён — обновление отменено.";

/// Shown when the compiled-in key is not a valid minisign public key.
pub const CORRUPT_KEY_MESSAGE: &str =
    "Ключ подписи обновлений в приложении повреждён — обновление отменено.";

/// Shown when the release publishes an asset without its `.sig`.
pub const UNSIGNED_RELEASE_MESSAGE: &str =
    "Релиз не подписан — автообновление недоступно, скачайте новую версию с GitHub вручную.";

/// Public key of the update channel, if the build was configured with one.
///
/// An empty value counts as "not configured": a build without a secret must
/// disable updates, not accept everything.
pub fn update_public_key() -> Option<&'static str> {
    EMBEDDED_PUBLIC_KEY
        .map(str::trim)
        .filter(|key| !key.is_empty())
}

/// File name of the detached signature for a release asset.
pub fn signature_file_name(asset_name: &str) -> String {
    format!("{asset_name}.sig")
}

/// Key id of a minisign public key, uppercase hex (16 characters).
///
/// This is the fingerprint the UI shows next to the update channel, so a user
/// can compare it with the one published in `SECURITY.md`.
pub fn fingerprint(key: &str) -> Option<String> {
    let bytes = key_bytes(key)?;
    if !matches!(&bytes[0..2], [b'E', b'd'] | [b'E', b'D']) {
        return None;
    }
    Some(
        bytes[2..10]
            .iter()
            .map(|byte| format!("{byte:02X}"))
            .collect(),
    )
}

/// Raw 42 bytes of a minisign public key (algorithm, key id, Ed25519 key).
///
/// Three shapes exist in the wild and all of them must work, because the
/// release pipeline and the build configuration are maintained separately:
///
/// * `RWQ...` — the bare key line, as `minisign -G` prints it;
/// * a public key box (`untrusted comment:` line + key line), as written to
///   `minisign.pub`;
/// * base64 of that box, which is what `tauri signer generate` stores in the
///   `*.pub` file and what people paste into `SPLITLINE_UPDATE_PUBKEY`.
fn key_bytes(key: &str) -> Option<Vec<u8>> {
    let trimmed = key.trim();
    let decode = |line: &str| BASE64.decode(line.trim()).ok();
    let as_key = |bytes: Vec<u8>| (bytes.len() == 42).then_some(bytes);

    if let Some(bytes) = decode(trimmed) {
        if let Some(key) = as_key(bytes.clone()) {
            return Some(key);
        }
        // base64 of the whole box: unwrap one level and read its key line.
        let boxed = String::from_utf8(bytes).ok()?;
        return as_key(decode(boxed.lines().nth(1)?)?);
    }
    // A plain box, as written by `minisign -G` and `tauri signer`.
    as_key(decode(trimmed.lines().nth(1)?)?)
}

/// Parses a detached signature in either shape the tooling produces: the
/// 4-line minisign box, or base64 of it (`tauri signer sign`).
fn parse_signature(signature_text: &str) -> Option<Signature> {
    let trimmed = signature_text.trim();
    if let Ok(signature) = Signature::decode(trimmed) {
        return Some(signature);
    }
    let decoded = BASE64.decode(trimmed).ok()?;
    let boxed = String::from_utf8(decoded).ok()?;
    Signature::decode(boxed.trim()).ok()
}

/// Verifies a detached signature over `data` with an explicit public key.
///
/// `signature_text` is the content of the `.sig` file published next to the
/// asset (anything `tauri signer sign` or `minisign -S` writes).
pub fn verify_detached(key: &str, data: &[u8], signature_text: &str) -> Result<(), String> {
    let raw_key = key_bytes(key).ok_or_else(|| CORRUPT_KEY_MESSAGE.to_string())?;
    let public_key = PublicKey::from_base64(&BASE64.encode(raw_key))
        .map_err(|_| CORRUPT_KEY_MESSAGE.to_string())?;
    let signature =
        parse_signature(signature_text).ok_or_else(|| CORRUPT_SIGNATURE_MESSAGE.to_string())?;
    // Legacy mode is the pre-0.7 minisign format (signature over the raw file,
    // no BLAKE2b pre-hash). Both are Ed25519 signatures, and older signing tools
    // still emit the legacy one, so accept both instead of locking out a key.
    public_key
        .verify(data, &signature, true)
        .map_err(|_| SIGNATURE_MISMATCH_MESSAGE.to_string())
}

/// Verifies a detached signature against the key compiled into this build.
///
/// Returns [`MISSING_KEY_MESSAGE`] when the build has no key, so a caller can
/// surface the reason verbatim.
pub fn verify_update(data: &[u8], signature_text: &str) -> Result<(), String> {
    let key = update_public_key().ok_or_else(|| MISSING_KEY_MESSAGE.to_string())?;
    verify_detached(key, data, signature_text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use minisign::{KeyPair, PublicKeyBox, SignatureBox};
    use std::io::Cursor;

    /// A real minisign key pair generated in the test, plus its signatures.
    ///
    /// Generating a key pair here (`minisign` dev-dependency) keeps the fixtures
    /// honest: the signatures have exactly the shape the release pipeline
    /// produces, so the parser is tested against the real format rather than a
    /// hand-built one. No key material is stored in the repository.
    struct Fixture {
        key: String,
        signature: String,
        payload: Vec<u8>,
    }

    fn fixture() -> Fixture {
        let KeyPair { pk, sk } = KeyPair::generate_unencrypted_keypair().unwrap();
        let payload = b"splitline update payload".to_vec();
        let signature_box = minisign::sign(None, &sk, Cursor::new(&payload), None, None).unwrap();
        let key = pk
            .to_box()
            .unwrap()
            .to_string()
            .lines()
            .nth(1)
            .unwrap()
            .to_string();
        Fixture {
            key,
            signature: signature_box.to_string(),
            payload,
        }
    }

    fn another_key() -> String {
        let KeyPair { pk, .. } = KeyPair::generate_unencrypted_keypair().unwrap();
        pk.to_box()
            .unwrap()
            .to_string()
            .lines()
            .nth(1)
            .unwrap()
            .to_string()
    }

    #[test]
    fn valid_signature_is_accepted() {
        let fixture = fixture();
        verify_detached(&fixture.key, &fixture.payload, &fixture.signature).unwrap();
    }

    #[test]
    fn signature_file_name_matches_the_updater_lookup() {
        let fixture = fixture();
        // The release pipeline names the signature exactly like this; the updater
        // looks the same name up in the release assets.
        assert_eq!(signature_file_name("Splitline.exe"), "Splitline.exe.sig");

        // The box the CI signing step writes must also parse with the minisign
        // crate, i.e. this is a real detached signature file, not a lookalike.
        SignatureBox::from_string(&fixture.signature).unwrap();
        verify_detached(&fixture.key, &fixture.payload, &fixture.signature).unwrap();
    }

    #[test]
    fn tampered_payload_is_rejected() {
        let fixture = fixture();
        let mut tampered = fixture.payload.clone();
        tampered[0] ^= 0xff;
        assert_eq!(
            verify_detached(&fixture.key, &tampered, &fixture.signature).unwrap_err(),
            SIGNATURE_MISMATCH_MESSAGE
        );
    }

    #[test]
    fn signature_from_another_key_is_rejected() {
        let fixture = fixture();
        assert_eq!(
            verify_detached(&another_key(), &fixture.payload, &fixture.signature).unwrap_err(),
            SIGNATURE_MISMATCH_MESSAGE
        );
    }

    #[test]
    fn empty_or_missing_signature_is_rejected() {
        let fixture = fixture();
        for empty in ["", "   ", "\n\n"] {
            assert_eq!(
                verify_detached(&fixture.key, &fixture.payload, empty).unwrap_err(),
                CORRUPT_SIGNATURE_MESSAGE
            );
        }
    }

    #[test]
    fn corrupt_signature_is_rejected() {
        let fixture = fixture();
        let mut lines: Vec<String> = fixture.signature.lines().map(str::to_string).collect();

        // Break the Ed25519 signature itself but keep the file well formed: the
        // bytes decode, so this must fail the cryptographic check, not the parser.
        let mut raw = BASE64.decode(&lines[1]).unwrap();
        let last = raw.len() - 1;
        raw[last] ^= 0xff;
        lines[1] = BASE64.encode(raw);
        assert_eq!(
            verify_detached(&fixture.key, &fixture.payload, &lines.join("\n")).unwrap_err(),
            SIGNATURE_MISMATCH_MESSAGE
        );

        // A truncated file (as if the download stopped early) must not parse.
        let truncated = lines[0..2].join("\n");
        assert_eq!(
            verify_detached(&fixture.key, &fixture.payload, &truncated).unwrap_err(),
            CORRUPT_SIGNATURE_MESSAGE
        );
    }

    #[test]
    fn tauri_signer_shapes_are_accepted() {
        // `tauri signer sign` writes base64 of the whole signature box, and
        // `tauri signer generate` stores the public key as base64 of the key
        // box. Releases are signed by the Tauri CLI, so both must verify.
        let fixture = fixture();
        let wrapped_signature = BASE64.encode(&fixture.signature);
        verify_detached(&fixture.key, &fixture.payload, &wrapped_signature).unwrap();

        let key_box = format!("untrusted comment: minisign public key\n{}\n", fixture.key);
        let wrapped_key = BASE64.encode(key_box);
        verify_detached(&wrapped_key, &fixture.payload, &wrapped_signature).unwrap();
        assert_eq!(
            fingerprint(&wrapped_key).unwrap(),
            fingerprint(&fixture.key).unwrap()
        );

        // A tampered payload must still fail when the shapes are wrapped.
        let mut tampered = fixture.payload.clone();
        tampered[0] ^= 0xff;
        assert_eq!(
            verify_detached(&wrapped_key, &tampered, &wrapped_signature).unwrap_err(),
            SIGNATURE_MISMATCH_MESSAGE
        );
    }

    #[test]
    fn corrupt_key_is_rejected() {
        let fixture = fixture();
        for key in ["", "not base64", "RWQ", &"A".repeat(40)] {
            assert_eq!(
                verify_detached(key, &fixture.payload, &fixture.signature).unwrap_err(),
                CORRUPT_KEY_MESSAGE
            );
        }
    }

    #[test]
    fn fingerprint_is_the_minisign_key_id() {
        let fixture = fixture();
        let bytes = BASE64.decode(&fixture.key).unwrap();
        let expected = bytes[2..10]
            .iter()
            .map(|byte| format!("{byte:02X}"))
            .collect::<String>();
        assert_eq!(fingerprint(&fixture.key).unwrap(), expected);
        assert_eq!(fingerprint(&fixture.key).unwrap().len(), 16);

        // The fingerprint identifies the key, so a second key must differ.
        assert_ne!(fingerprint(&another_key()).unwrap(), expected);
        assert!(fingerprint("not a key").is_none());
    }

    #[test]
    fn public_key_box_parses_as_the_pipeline_writes_it() {
        // `scripts/generate-signing-key.ps1` hands over the second line of the
        // public key file; make sure that is what verification expects.
        let fixture = fixture();
        let boxed = PublicKeyBox::from_string(&format!(
            "untrusted comment: minisign public key\n{}\n",
            fixture.key
        ))
        .unwrap();
        let pk = boxed.into_public_key().unwrap();
        assert_eq!(pk.to_base64(), fixture.key);
        verify_detached(&fixture.key, &fixture.payload, &fixture.signature).unwrap();
    }
}
