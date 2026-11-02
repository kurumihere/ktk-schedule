use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{self, Aead, Generate, Payload},
};
use anyhow::{Result, anyhow, ensure};
use base64::{Engine, engine::general_purpose::STANDARD_NO_PAD};
use sha2::{Digest, Sha256};

/// Sealed values: `cache:v1:` + base64(nonce ‖ AES-256-GCM ciphertext ‖ tag).
const CACHE: &str = "cache:v1:";
const NONCE: usize = 12;
const TAG: usize = 16;

/// AES-256-GCM keyed by SHA-256(CREDENTIALS_SECRET). Every value is bound to a context
/// (associated data naming its table, user and row), so ciphertext cannot be moved between
/// rows or users. Passwords are never sealed: the bot does not keep them at all.
pub struct Cipher(Aes256Gcm);

impl Cipher {
    pub fn new(secret: &str) -> Result<Self> {
        ensure!(secret.trim().len() >= 32, "credential secret is too short");
        Ok(Self(Aes256Gcm::new(&Sha256::digest(
            secret.trim().as_bytes(),
        ))))
    }

    pub fn encrypt_cache(&self, context: &str, value: &str) -> Result<String> {
        let nonce = Nonce::try_generate().map_err(|_| anyhow!("randomness unavailable"))?;
        self.seal(&nonce, context, value)
    }

    fn seal(&self, nonce: &aead::Nonce<Aes256Gcm>, context: &str, value: &str) -> Result<String> {
        let sealed = self
            .0
            .encrypt(
                nonce,
                Payload {
                    msg: value.as_bytes(),
                    aad: context.as_bytes(),
                },
            )
            .map_err(|_| anyhow!("encryption failed"))?;
        let mut data = nonce.to_vec();
        data.extend(sealed);
        Ok(format!("{CACHE}{}", STANDARD_NO_PAD.encode(data)))
    }

    pub fn decrypt_cache(&self, context: &str, value: &str) -> Result<String> {
        let encoded = value
            .strip_prefix(CACHE)
            .ok_or_else(|| anyhow!("unsupported sealed value format"))?;
        let bytes = STANDARD_NO_PAD.decode(encoded)?;
        ensure!(bytes.len() >= NONCE + TAG, "truncated sealed value");
        let (nonce, sealed) = bytes.split_at(NONCE);
        let plaintext = self
            .0
            .decrypt(
                Nonce::try_from(nonce)
                    .map_err(|_| anyhow!("invalid nonce"))?
                    .as_ref(),
                Payload {
                    msg: sealed,
                    aad: context.as_bytes(),
                },
            )
            .map_err(|_| anyhow!("authentication failed; check CREDENTIALS_SECRET"))?;
        Ok(String::from_utf8(plaintext)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONTEXT: &str = r#"["sessions",42]"#;

    #[test]
    fn sealed_values_survive_crypto_dependency_updates() {
        // Key "a" x 32, nonce [7; 12]. Its keystream also reproduces the aes-gcm 0.10.3
        // password fixture this format replaced, so the cipher has never changed.
        let cipher = Cipher::new(&"a".repeat(32)).unwrap();
        let sealed = "cache:v1:BwcHBwcHBwcHBwcHTiQREKKt01tPfHuPPXag3nYadU1HRAWSJGvOCg";
        assert_eq!(cipher.decrypt_cache(CONTEXT, sealed).unwrap(), "сессия");
        let nonce = aead::Nonce::<Aes256Gcm>::try_from(&[7u8; NONCE][..]).unwrap();
        assert_eq!(cipher.seal(&nonce, CONTEXT, "сессия").unwrap(), sealed);
    }

    #[test]
    fn sealed_values_are_randomized_and_bound_to_their_context() {
        let cipher = Cipher::new(&"a".repeat(32)).unwrap();
        let a = cipher.encrypt_cache(CONTEXT, "сессия").unwrap();
        assert_ne!(a, cipher.encrypt_cache(CONTEXT, "сессия").unwrap());
        assert_eq!(cipher.decrypt_cache(CONTEXT, &a).unwrap(), "сессия");
        assert!(cipher.decrypt_cache(r#"["sessions",43]"#, &a).is_err());
        assert!(
            Cipher::new(&"b".repeat(32))
                .unwrap()
                .decrypt_cache(CONTEXT, &a)
                .is_err()
        );
        assert!(cipher.decrypt_cache(CONTEXT, "plaintext").is_err());
        assert!(cipher.decrypt_cache(CONTEXT, "cache:v1:AA").is_err());
        // The retired password format is no longer accepted anywhere.
        assert!(
            cipher
                .decrypt_cache(
                    CONTEXT,
                    "v2:BwcHBwcHBwcHBwcHTxoRFaKs0mRPf3uM9BmjUs8X9aMjghx+JvQO5w"
                )
                .is_err()
        );
        assert!(Cipher::new("short").is_err());
    }
}
