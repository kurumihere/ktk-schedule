use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, Generate, Payload},
};
use anyhow::{Result, anyhow, ensure};
use base64::{Engine, engine::general_purpose::STANDARD_NO_PAD};
use sha2::{Digest, Sha256};

pub struct Cipher(Aes256Gcm);

impl Cipher {
    pub fn new(secret: &str) -> Result<Self> {
        ensure!(secret.trim().len() >= 32, "credential secret is too short");
        Ok(Self(Aes256Gcm::new(&Sha256::digest(
            secret.trim().as_bytes(),
        ))))
    }

    pub fn encrypt(&self, account_id: i64, value: &str) -> Result<String> {
        let nonce =
            Nonce::try_generate().map_err(|_| anyhow!("credential randomness unavailable"))?;
        let sealed = self
            .0
            .encrypt(
                &nonce,
                Payload {
                    msg: value.as_bytes(),
                    aad: &account_id.to_be_bytes(),
                },
            )
            .map_err(|_| anyhow!("credential encryption failed"))?;
        let mut data = nonce.to_vec();
        data.extend(sealed);
        Ok(format!("v2:{}", STANDARD_NO_PAD.encode(data)))
    }

    pub fn decrypt(&self, account_id: i64, value: &str) -> Result<String> {
        let encoded = value
            .strip_prefix("v2:")
            .ok_or_else(|| anyhow!("unsupported credential format"))?;
        let bytes = STANDARD_NO_PAD.decode(encoded)?;
        ensure!(bytes.len() >= 28, "truncated credential");
        let plaintext = self
            .0
            .decrypt(
                Nonce::try_from(&bytes[..12])
                    .map_err(|_| anyhow!("invalid credential nonce"))?
                    .as_ref(),
                Payload {
                    msg: &bytes[12..],
                    aad: &account_id.to_be_bytes(),
                },
            )
            .map_err(|_| anyhow!("credential authentication failed; check CREDENTIALS_SECRET"))?;
        Ok(String::from_utf8(plaintext)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn existing_v2_credentials_survive_crypto_dependency_updates() {
        // Synthetic fixture from aes-gcm 0.10.3: key = "a" x 32, nonce = [7; 12], ID = 42.
        let cipher = Cipher::new(&"a".repeat(32)).unwrap();
        let encoded = "v2:BwcHBwcHBwcHBwcHTxoRFaKs0mRPf3uM9BmjUs8X9aMjghx+JvQO5w";
        assert_eq!(cipher.decrypt(42, encoded).unwrap(), "пароль");
        assert!(cipher.decrypt(43, encoded).is_err());
    }

    #[test]
    fn credentials_are_randomized_and_bound_to_the_account() {
        let cipher = Cipher::new(&"a".repeat(32)).unwrap();
        let a = cipher.encrypt(42, "пароль").unwrap();
        assert_ne!(a, cipher.encrypt(42, "пароль").unwrap());
        assert_eq!(cipher.decrypt(42, &a).unwrap(), "пароль");
        assert!(cipher.decrypt(43, &a).is_err());
        assert!(
            Cipher::new(&"b".repeat(32))
                .unwrap()
                .decrypt(42, &a)
                .is_err()
        );
        assert!(cipher.decrypt(42, "plaintext").is_err());
        assert!(cipher.decrypt(42, "v2:AA").is_err());
    }
}
