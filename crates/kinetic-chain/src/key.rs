//! Software device key. A secure-element signer can implement the same trait later.
//! The private key is encrypted with the install secret and is never logged.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use alloy::primitives::{Address, B256, Signature};
use alloy::signers::SignerSync;
use alloy::signers::local::PrivateKeySigner;
use anyhow::{Context, Result};
use kinetic_config::security::SecretStore;
use zeroize::Zeroize;

const DEVICE_KEY_FILE: &str = "device.key";

/// Signs the digest the chain contract already hashed.
pub trait DeviceSigner: Send + Sync {
    fn address(&self) -> Address;
    fn sign_hash(&self, hash: &B256) -> Result<Signature>;
}

/// secp256k1 key created on first boot and stored encrypted.
pub struct SoftwareKey {
    signer: PrivateKeySigner,
}

impl std::fmt::Debug for SoftwareKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SoftwareKey")
            .field("address", &self.address())
            .finish()
    }
}

impl SoftwareKey {
    /// Load the encrypted device key, or create one when the file is absent.
    pub fn load_or_create(dir: &Path) -> Result<Self> {
        fs::create_dir_all(dir).context("failed to create the device key directory")?;
        let path = dir.join(DEVICE_KEY_FILE);
        if path.exists() {
            return Self::load(&path, dir);
        }
        let signer = PrivateKeySigner::random();
        match Self::store(&path, dir, &signer) {
            Ok(()) => Ok(Self { signer }),
            Err(error) if path.exists() => {
                ::kinetic_log::record!(
                    WARN,
                    ::kinetic_log::Event::new(module_path!(), ::kinetic_log::Action::Note)
                        .with_outcome(::kinetic_log::EventOutcome::Unknown)
                        .with_attrs(::serde_json::json!({"error": format!("{error}")})),
                    "device key appeared while it was being created; loading the stored key"
                );
                Self::load(&path, dir)
            }
            Err(error) => Err(error),
        }
    }

    pub fn address(&self) -> Address {
        self.signer.address()
    }

    fn load(path: &Path, dir: &Path) -> Result<Self> {
        let stored = fs::read_to_string(path).context("failed to read the device key file")?;
        let store = SecretStore::new(dir, true);
        let mut encoded = store
            .decrypt(stored.trim())
            .context("failed to decrypt the device key")?;
        let decode_result = hex::decode(encoded.trim())
            .map_err(|_| anyhow::Error::msg("device key file does not contain a signing key"));
        encoded.zeroize();
        let mut decoded = decode_result?;
        if decoded.len() != 32 {
            decoded.zeroize();
            return Err(anyhow::Error::msg(
                "device key file does not contain a signing key",
            ));
        }
        let mut raw = [0u8; 32];
        raw.copy_from_slice(&decoded);
        decoded.zeroize();
        let signer = PrivateKeySigner::from_bytes(&B256::from(raw))
            .map_err(|_| anyhow::Error::msg("device key file does not contain a signing key"));
        raw.zeroize();
        Ok(Self { signer: signer? })
    }

    fn store(path: &Path, dir: &Path, signer: &PrivateKeySigner) -> Result<()> {
        let store = SecretStore::new(dir, true);
        let mut bytes = signer.to_bytes().to_vec();
        let mut encoded = hex::encode(&bytes);
        bytes.zeroize();
        let encrypted = store.encrypt(&encoded);
        encoded.zeroize();
        let encrypted = encrypted.context("failed to encrypt the device key")?;
        if !encrypted.starts_with("enc2:") {
            return Err(anyhow::Error::msg(
                "device key encryption did not produce an encrypted file",
            ));
        }
        write_private(path, encrypted.as_bytes()).context("failed to store the device key")
    }
}

impl DeviceSigner for SoftwareKey {
    fn address(&self) -> Address {
        self.signer.address()
    }

    fn sign_hash(&self, hash: &B256) -> Result<Signature> {
        self.signer
            .sign_hash_sync(hash)
            .context("failed to sign the chain digest")
    }
}

fn write_private(path: &Path, contents: &[u8]) -> Result<()> {
    let tmp = temp_path(path);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&tmp)?;
    file.write_all(contents)?;
    file.sync_all()?;
    if let Err(error) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(error.into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn temp_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_else(|| "device.key".into());
    name.push(".tmp");
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_boot_creates_an_encrypted_key_and_the_next_boot_keeps_it() {
        let dir = tempfile::tempdir().expect("temp dir");
        let created = SoftwareKey::load_or_create(dir.path()).expect("create");
        let address = created.address();
        let stored = fs::read_to_string(dir.path().join(DEVICE_KEY_FILE)).expect("read");
        assert!(stored.starts_with("enc2:"), "key file must be encrypted");
        let loaded = SoftwareKey::load_or_create(dir.path()).expect("load");
        assert_eq!(loaded.address(), address);
        let debug = format!("{created:?}");
        assert!(!debug.contains("enc2:"));
        assert_eq!(debug.matches("0x").count(), 1);
    }

    #[test]
    fn signature_recovers_to_the_device_address() {
        let dir = tempfile::tempdir().expect("temp dir");
        let key = SoftwareKey::load_or_create(dir.path()).expect("create");
        let hash = B256::from([7u8; 32]);
        let signature = key.sign_hash(&hash).expect("sign");
        let recovered = signature
            .recover_address_from_prehash(&hash)
            .expect("recover");
        assert_eq!(recovered, key.address());
    }

    #[cfg(unix)]
    #[test]
    fn key_file_is_owner_readable_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("temp dir");
        SoftwareKey::load_or_create(dir.path()).expect("create");
        let mode = fs::metadata(dir.path().join(DEVICE_KEY_FILE))
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }
}
