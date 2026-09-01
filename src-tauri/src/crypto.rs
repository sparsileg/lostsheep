use argon2::{Argon2, Params};
use rand::RngCore;

/// 32 random bytes, hex-encoded — used as the live DB's SQLCipher key.
pub fn random_key_hex() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

/// 16-byte random salt, hex-encoded, stored alongside a backup file so
/// restore can re-derive the same key from the passphrase later.
pub fn random_salt_hex() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

/// Derives a 32-byte SQLCipher key (hex) from a user passphrase + salt via
/// Argon2id. Deliberately slow (interactive-strength params) — this runs
/// once per backup/restore, not on the hot path.
pub fn derive_key_hex(passphrase: &str, salt_hex: &str) -> anyhow::Result<String> {
    let salt = hex::decode(salt_hex)?;
    let params = Params::new(19 * 1024, 2, 1, Some(32))
        .map_err(|e| anyhow::anyhow!("argon2 params: {e}"))?;
    let argon2 = Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let mut out = [0u8; 32];
    argon2
        .hash_password_into(passphrase.as_bytes(), &salt, &mut out)
        .map_err(|e| anyhow::anyhow!("argon2 derive: {e}"))?;
    Ok(hex::encode(out))
}

mod hex {
    pub fn encode(bytes: impl AsRef<[u8]>) -> String {
        bytes.as_ref().iter().map(|b| format!("{:02x}", b)).collect()
    }
    pub fn decode(s: &str) -> anyhow::Result<Vec<u8>> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(anyhow::Error::from))
            .collect()
    }
}
