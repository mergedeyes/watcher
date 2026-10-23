//! SSH credentials and their encryption at rest.
//!
//! Credentials are stored sealed with ChaCha20-Poly1305 under a key from
//! `secret.key` in the data directory, created on first use. The secret's
//! name is authenticated along with it, so a sealed value can't be moved
//! to another name.

use ring::aead::{Aad, CHACHA20_POLY1305, LessSafeKey, NONCE_LEN, Nonce, UnboundKey};
use russh::keys::ssh_key::LineEnding;
use russh::keys::ssh_key::private::Ed25519Keypair;
use russh::keys::{HashAlg, PrivateKey};
use std::fmt;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// What a secret holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Key,
    Password,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Key => "key",
            Kind::Password => "password",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "key" => Some(Kind::Key),
            "password" => Some(Kind::Password),
            _ => None,
        }
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How to log in: a private key or a password.
#[derive(Clone)]
pub enum Credential {
    Key(Box<PrivateKey>),
    Password(String),
}

/// Never prints the secret.
impl fmt::Debug for Credential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Credential::Key(key) => write!(f, "Key({})", key.fingerprint(HashAlg::Sha256)),
            Credential::Password(_) => f.write_str("Password(..)"),
        }
    }
}

impl Credential {
    /// A new ed25519 key.
    pub fn generate(comment: &str) -> Result<Self, String> {
        let mut seed = [0u8; 32];
        getrandom::fill(&mut seed).map_err(|e| format!("random: {e}"))?;
        let mut key = PrivateKey::from(Ed25519Keypair::from_seed(&seed));
        key.set_comment(comment);
        Ok(Credential::Key(Box::new(key)))
    }

    /// A private key in OpenSSH, PEM (PKCS#8) or PuTTY format, decrypted
    /// with `passphrase` if it is encrypted. Not RSA: the `rsa` crate has
    /// an unfixed timing side channel (RUSTSEC-2023-0071).
    pub fn import(text: &str, passphrase: Option<&str>) -> Result<Self, String> {
        match russh::keys::decode_secret_key(text, passphrase) {
            Ok(key) if key.algorithm().is_rsa() => {
                Err("RSA keys are not supported: use ed25519 or ECDSA (or a new key)".into())
            }
            Ok(key) => Ok(Credential::Key(Box::new(key))),
            Err(russh::keys::Error::KeyIsEncrypted) => {
                Err("the key is encrypted: give its passphrase".into())
            }
            Err(err) => Err(format!("cannot read the key: {err}")),
        }
    }

    pub fn kind(&self) -> Kind {
        match self {
            Credential::Key(_) => Kind::Key,
            Credential::Password(_) => Kind::Password,
        }
    }

    /// The public key as a line for `authorized_keys`; keys only.
    pub fn public_key(&self) -> Option<String> {
        match self {
            Credential::Key(key) => key.public_key().to_openssh().ok(),
            Credential::Password(_) => None,
        }
    }

    /// `SHA256:...`; keys only.
    pub fn fingerprint(&self) -> Option<String> {
        match self {
            Credential::Key(key) => Some(key.fingerprint(HashAlg::Sha256).to_string()),
            Credential::Password(_) => None,
        }
    }

    fn to_bytes(&self) -> Result<Vec<u8>, String> {
        match self {
            Credential::Key(key) => key
                .to_openssh(LineEnding::LF)
                .map(|text| text.as_bytes().to_vec())
                .map_err(|e| format!("cannot encode the key: {e}")),
            Credential::Password(p) => Ok(p.as_bytes().to_vec()),
        }
    }

    fn from_bytes(kind: Kind, bytes: &[u8]) -> Result<Self, String> {
        match kind {
            Kind::Key => PrivateKey::from_openssh(bytes)
                .map(|key| Credential::Key(Box::new(key)))
                .map_err(|e| format!("stored key is invalid: {e}")),
            Kind::Password => String::from_utf8(bytes.to_vec())
                .map(Credential::Password)
                .map_err(|_| "stored password is not UTF-8".into()),
        }
    }
}

/// Seals and opens credentials with the key in `secret.key`.
pub struct Sealer {
    key: LessSafeKey,
}

/// Format version of sealed values, their first byte.
const VERSION: u8 = 1;

impl Sealer {
    pub const FILE: &str = "secret.key";

    pub fn path(data_dir: &Path) -> PathBuf {
        data_dir.join(Self::FILE)
    }

    /// The key in `data_dir`; an error if there is none.
    pub fn open(data_dir: &Path) -> Result<Self, String> {
        let path = Self::path(data_dir);
        let text = std::fs::read_to_string(&path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => format!(
                "{} is missing: without it, stored secrets can't be decrypted",
                path.display()
            ),
            _ => format!("{}: {e}", path.display()),
        })?;
        let bytes = hex_decode(text.trim())
            .filter(|b| b.len() == 32)
            .ok_or_else(|| format!("{}: expected 64 hex digits", path.display()))?;
        Self::new(&bytes)
    }

    /// The key in `data_dir`, created (mode 0600) if there is none yet.
    pub fn open_or_create(data_dir: &Path) -> Result<Self, String> {
        let path = Self::path(data_dir);
        if !path.exists() {
            let mut bytes = [0u8; 32];
            getrandom::fill(&mut bytes).map_err(|e| format!("random: {e}"))?;
            std::fs::create_dir_all(data_dir)
                .map_err(|e| format!("{}: {e}", data_dir.display()))?;
            let created = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path);
            match created {
                Ok(mut file) => {
                    file.set_permissions(std::fs::Permissions::from_mode(0o600))
                        .and_then(|()| writeln!(file, "{}", hex_encode(&bytes)))
                        .map_err(|e| format!("{}: {e}", path.display()))?;
                }
                // Created at the same time by someone else: use theirs.
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(format!("{}: {e}", path.display())),
            }
        }
        Self::open(data_dir)
    }

    fn new(bytes: &[u8]) -> Result<Self, String> {
        let key = UnboundKey::new(&CHACHA20_POLY1305, bytes).map_err(|_| "invalid key")?;
        Ok(Sealer {
            key: LessSafeKey::new(key),
        })
    }

    /// The credential `name`, encrypted.
    pub fn seal(&self, name: &str, credential: &Credential) -> Result<Vec<u8>, String> {
        let mut nonce = [0u8; NONCE_LEN];
        getrandom::fill(&mut nonce).map_err(|e| format!("random: {e}"))?;
        let mut data = credential.to_bytes()?;
        self.key
            .seal_in_place_append_tag(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(name.as_bytes()),
                &mut data,
            )
            .map_err(|_| "encryption failed")?;
        let mut out = Vec::with_capacity(1 + NONCE_LEN + data.len());
        out.push(VERSION);
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&data);
        Ok(out)
    }

    /// Decrypts what [`Sealer::seal`] made for `name`.
    pub fn unseal(&self, name: &str, kind: Kind, sealed: &[u8]) -> Result<Credential, String> {
        let wrong = || "cannot decrypt: sealed with another secret.key, or damaged".to_string();
        let (&version, rest) = sealed.split_first().ok_or_else(wrong)?;
        if version != VERSION || rest.len() < NONCE_LEN {
            return Err(wrong());
        }
        let (nonce, data) = rest.split_at(NONCE_LEN);
        let nonce = Nonce::try_assume_unique_for_key(nonce).map_err(|_| wrong())?;
        let mut data = data.to_vec();
        let plain = self
            .key
            .open_in_place(nonce, Aad::from(name.as_bytes()), &mut data)
            .map_err(|_| wrong())?;
        Credential::from_bytes(kind, plain)
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn hex_decode(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("watcher-secret-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn seal_and_unseal() {
        let dir = dir("seal");
        let sealer = Sealer::open_or_create(&dir).unwrap();
        let mode = std::fs::metadata(Sealer::path(&dir))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);

        let key = Credential::generate("watcher test").unwrap();
        let sealed = sealer.seal("monitor", &key).unwrap();
        let back = sealer.unseal("monitor", Kind::Key, &sealed).unwrap();
        assert_eq!(back.public_key(), key.public_key());
        assert!(key.public_key().unwrap().starts_with("ssh-ed25519 "));
        assert!(key.public_key().unwrap().ends_with(" watcher test"));

        // Bound to the name, and to this key file.
        assert!(sealer.unseal("other", Kind::Key, &sealed).is_err());
        let again = Sealer::open_or_create(&dir).unwrap();
        assert!(again.unseal("monitor", Kind::Key, &sealed).is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
        let fresh = Sealer::open_or_create(&dir).unwrap();
        assert!(fresh.unseal("monitor", Kind::Key, &sealed).is_err());

        let password = Credential::Password("hunter2".into());
        let sealed = fresh.seal("pw", &password).unwrap();
        assert!(!sealed.windows(7).any(|w| w == b"hunter2"));
        match fresh.unseal("pw", Kind::Password, &sealed).unwrap() {
            Credential::Password(p) => assert_eq!(p, "hunter2"),
            other => panic!("{other:?}"),
        }
        assert_eq!(format!("{password:?}"), "Password(..)");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_key_file() {
        let dir = dir("missing");
        let err = Sealer::open(&dir).err().unwrap();
        assert!(err.contains("secret.key is missing"), "{err}");
    }

    #[test]
    fn imports_keys() {
        let key = Credential::generate("x").unwrap();
        let Credential::Key(private) = &key else {
            unreachable!()
        };
        let text = private.to_openssh(LineEnding::LF).unwrap();
        let imported = Credential::import(&text, None).unwrap();
        assert_eq!(imported.fingerprint(), key.fingerprint());
        assert!(Credential::import("not a key", None).is_err());

        // Keys from ssh-keygen, if installed.
        let dir = dir("keygen");
        std::fs::create_dir_all(&dir).unwrap();
        let keygen = |kind: &str| {
            let path = dir.join(kind);
            let made = std::process::Command::new("ssh-keygen")
                .args(["-q", "-t", kind, "-N", "", "-f"])
                .arg(&path)
                .status();
            made.is_ok_and(|s| s.success())
                .then(|| std::fs::read_to_string(&path).unwrap())
        };
        if let (Some(ecdsa), Some(rsa)) = (keygen("ecdsa"), keygen("rsa")) {
            assert!(Credential::import(&ecdsa, None).is_ok());
            let err = Credential::import(&rsa, None).unwrap_err();
            assert!(err.contains("RSA keys are not supported"), "{err}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
