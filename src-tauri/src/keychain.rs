use keyring::Entry;

const SERVICE: &str = "org.lostsheep.app";
const ACCOUNT: &str = "db-key";

/// Fetches the live DB's SQLCipher key from the OS keychain, generating
/// and storing a fresh one on first run. This is the ONLY place the app
/// touches Stronghold-alternative key storage — deliberately OS keychain,
/// not Stronghold, per the settled architecture decision (no extra
/// unlock-password layer for a single non-technical user).
pub fn get_or_create_db_key() -> anyhow::Result<String> {
    let entry = Entry::new(SERVICE, ACCOUNT)?;
    match entry.get_password() {
        Ok(key) => Ok(key),
        Err(keyring::Error::NoEntry) => {
            let key = crate::crypto::random_key_hex();
            entry.set_password(&key)?;
            Ok(key)
        }
        Err(e) => Err(anyhow::anyhow!(
            "OS keychain unavailable: {e}. Use Help > 'Keychain not working' to restore from backup instead."
        )),
    }
}
