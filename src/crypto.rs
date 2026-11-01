//! Criptografia de texto (`-crypt`) e hashes.
//!
//! Compatível com o formato do gaP em AutoIt, que usava `_Crypt_EncryptData`
//! com AES-256: chave derivada de MD5(senha ANSI) via `CryptDeriveKey`
//! (expansão ipad/opad do CryptoAPI), CBC, IV zero, padding PKCS#7, saída em
//! hexadecimal maiúsculo com prefixo "0x" e delimitada por `!@@!`.
//! Textos cifrados pelo gaP original continuam decifráveis.

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

/// Cifra e devolve `!@@!0x…!@@!`.
pub fn encrypt(text: &str, password: &str) -> String {
    let key = derive_key(password);
    let enc = cbc::Encryptor::<Aes256>::new((&key).into(), (&[0u8; 16]).into());
    let ct = enc.encrypt_padded_vec::<Pkcs7>(&util::string_to_ansi(text));
    format!("{}0x{}{}", MARK, to_hex(&ct), MARK)
}

/// Decifra `!@@!0x…!@@!` (ou só o hex). `None` = senha errada / dado inválido.
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

/// SHA-1 em hexadecimal maiúsculo (como `_Crypt_HashData` sem "0x").
pub fn sha1_hex(data: &[u8]) -> String {
    to_hex(&Sha1::digest(data))
}

/// SHA-1 de um arquivo; "-1" se não existir (como o `HashFile` original).
pub fn sha1_file(path: &str) -> String {
    match std::fs::read(path) {
        Ok(b) => sha1_hex(&b),
        Err(_) => "-1".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let e = encrypt("Olá mundo 123", "segredo");
        assert!(e.starts_with("!@@!0x") && e.ends_with("!@@!"));
        assert_eq!(decrypt(&e, "segredo").as_deref(), Some("Olá mundo 123"));
        assert!(decrypt(&e, "errada").is_none() || decrypt(&e, "errada").as_deref() != Some("Olá mundo 123"));
    }

    #[test]
    fn sha1_known() {
        assert_eq!(sha1_hex(b"abc"), "A9993E364706816ABA3E25717850C26C9CD0D89D");
    }
}
