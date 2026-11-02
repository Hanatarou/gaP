//! Text encryption (`-crypt`) and hashes.
//!
//! Compatible with the format of the AutoIt gaP, which used `_Crypt_EncryptData`
//! with AES-256: key derived from MD5(ANSI password) through `CryptDeriveKey`
//! (ipad/opad expansion of the CryptoAPI), CBC, zero IV, PKCS#7 padding, output as
//! upper-case hexadecimal with the "0x" prefix and delimited by `!@@!`.
//! Texts encrypted by the original gaP can still be decrypted.

use aes::Aes256;
use cipher::block_padding::Pkcs7;
use cipher::{BlockModeDecrypt, BlockModeEncrypt, KeyIvInit};
use md5::Md5;
use sha1::{Digest, Sha1};

use crate::util;

pub const MARK: &str = "!@@!";

fn derive_key(password: &str) -> [u8; 32] {
    let pw = util::string_to_ansi(password);
    let h = Md5::digest(&pw);
    let mut b1 = [0x36u8; 64];
    let mut b2 = [0x5Cu8; 64];
    for i in 0..16 {
        b1[i] ^= h[i];
        b2[i] ^= h[i];
    }
    let x1 = Md5::digest(b1);
    let x2 = Md5::digest(b2);
    let mut key = [0u8; 32];
    key[..16].copy_from_slice(&x1);
    key[16..].copy_from_slice(&x2);
    key
}

pub fn to_hex(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for x in b {
        s.push_str(&format!("{:02X}", x));
    }
    s
}

pub fn from_hex(s: &str) -> Option<Vec<u8>> {
    let t = s.trim();
    let t = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")).unwrap_or(t);
    if !t.len().is_multiple_of(2) {
        return None;
    }
    (0..t.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&t[i..i + 2], 16).ok())
        .collect()
}

/// Encrypts and returns `!@@!0x…!@@!`.
pub fn encrypt(text: &str, password: &str) -> String {
    let key = derive_key(password);
    let enc = cbc::Encryptor::<Aes256>::new((&key).into(), (&[0u8; 16]).into());
    let ct = enc.encrypt_padded_vec::<Pkcs7>(&util::string_to_ansi(text));
    format!("{}0x{}{}", MARK, to_hex(&ct), MARK)
}

/// Decrypts `!@@!0x…!@@!` (or just the hex). `None` = wrong password / invalid data.
pub fn decrypt(data: &str, password: &str) -> Option<String> {
    let inner = data.replace(MARK, "");
    let ct = from_hex(&inner)?;
    if ct.is_empty() || ct.len() % 16 != 0 {
        return None;
    }
    let key = derive_key(password);
    let dec = cbc::Decryptor::<Aes256>::new((&key).into(), (&[0u8; 16]).into());
    let pt = dec.decrypt_padded_vec::<Pkcs7>(&ct).ok()?;
    Some(util::ansi_to_string(&pt))
}

/// SHA-1 as upper-case hexadecimal (like `_Crypt_HashData` without "0x").
pub fn sha1_hex(data: &[u8]) -> String {
    to_hex(&Sha1::digest(data))
}

/// SHA-1 of a file; "-1" if it does not exist (like the original `HashFile`). The file is read in blocks, so
/// the whole executable is never held in memory.
pub fn sha1_file(path: &str) -> String {
    use std::io::Read;
    let Ok(mut f) = std::fs::File::open(path) else { return "-1".to_string() };
    let mut h = Sha1::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        match f.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => h.update(&buf[..n]),
            Err(_) => return "-1".to_string(),
        }
    }
    to_hex(&h.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let e = encrypt("Hello, café 123", "secret");
        assert!(e.starts_with("!@@!0x") && e.ends_with("!@@!"));
        assert_eq!(decrypt(&e, "secret").as_deref(), Some("Hello, café 123"));
        assert!(decrypt(&e, "wrong").is_none() || decrypt(&e, "wrong").as_deref() != Some("Hello, café 123"));
    }

    #[test]
    fn sha1_known() {
        assert_eq!(sha1_hex(b"abc"), "A9993E364706816ABA3E25717850C26C9CD0D89D");
    }
}
