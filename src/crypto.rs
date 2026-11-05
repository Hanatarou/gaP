//! Text encryption (`-crypt`) and hashes.
//!
//! Compatible with the format of the AutoIt gaP, which used `_Crypt_EncryptData` with AES-256: the key is derived
//! from MD5(ANSI password) through `CryptDeriveKey`, CBC, zero IV, PKCS#7 padding, output as upper-case hexadecimal
//! with the "0x" prefix and delimited by `!@@!`. Texts encrypted by the original gaP can still be decrypted. The
//! cipher is the one of Windows (CryptoAPI), as in the original: gaP carries no cipher code of its own.

use std::ptr::null;

use windows_sys::Win32::Security::Cryptography::*;

use crate::util;

pub const MARK: &str = "!@@!";

// ------------------------------------------------------------ CryptoAPI

/// A CryptoAPI provider, released when it goes out of scope.
struct Provider(usize);

impl Provider {
    /// The provider of AES-256 (the one `_Crypt_Startup` of AutoIt uses); it needs no key container.
    fn new() -> Option<Provider> {
        let mut p = 0usize;
        let ok = unsafe { CryptAcquireContextW(&mut p, null(), null(), PROV_RSA_AES, CRYPT_VERIFYCONTEXT) };
        (ok != 0).then_some(Provider(p))
    }
}

impl Drop for Provider {
    fn drop(&mut self) {
        unsafe { CryptReleaseContext(self.0, 0) };
    }
}

/// A CryptoAPI key, destroyed when it goes out of scope (always before its provider).
struct Key(usize);

impl Drop for Key {
    fn drop(&mut self) {
        unsafe { CryptDestroyKey(self.0) };
    }
}

/// The AES-256 key of a password: `CryptDeriveKey` over the MD5 of the ANSI password, as `_Crypt_DeriveKey` does.
fn derive_key(prov: &Provider, password: &str) -> Option<Key> {
    let pw = util::string_to_ansi(password);
    unsafe {
        let mut h = 0usize;
        if CryptCreateHash(prov.0, CALG_MD5, 0, 0, &mut h) == 0 {
            return None;
        }
        let mut k = 0usize;
        let ok = CryptHashData(h, pw.as_ptr(), pw.len() as u32, 0) != 0 && CryptDeriveKey(prov.0, CALG_AES_256, h, 0, &mut k) != 0;
        CryptDestroyHash(h);
        ok.then_some(Key(k))
    }
}

// ------------------------------------------------------------------ hex

pub fn to_hex(b: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789ABCDEF";
    let mut s = String::with_capacity(b.len() * 2);
    for &x in b {
        s.push(DIGITS[(x >> 4) as usize] as char);
        s.push(DIGITS[(x & 15) as usize] as char);
    }
    s
}

/// Hexadecimal text (with or without "0x") as bytes. `None` = odd length or a character that is not a hex digit,
/// whatever it is: the text is read byte by byte, never cut in the middle of a character.
pub fn from_hex(s: &str) -> Option<Vec<u8>> {
    let t = s.trim();
    let t = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")).unwrap_or(t);
    let nibble = |c: u8| -> Option<u8> { (c as char).to_digit(16).map(|d| d as u8) };
    let (pairs, rest) = t.as_bytes().as_chunks::<2>();
    if !rest.is_empty() {
        return None;
    }
    pairs.iter().map(|&[hi, lo]| Some(nibble(hi)? << 4 | nibble(lo)?)).collect()
}

// -------------------------------------------------------------- AES-256

/// Encrypts and returns `!@@!0x…!@@!`. `None` = Windows could not give the cipher (it does not happen on a normal
/// system).
pub fn encrypt(text: &str, password: &str) -> Option<String> {
    let prov = Provider::new()?;
    let key = derive_key(&prov, password)?;
    let mut buf = util::string_to_ansi(text);
    let mut len = buf.len() as u32;
    buf.resize(buf.len() + 16, 0); // room for the padding
    if unsafe { CryptEncrypt(key.0, 0, 1, 0, buf.as_mut_ptr(), &mut len, buf.len() as u32) } == 0 {
        return None;
    }
    buf.truncate(len as usize);
    Some(format!("{}0x{}{}", MARK, to_hex(&buf), MARK))
}

/// Decrypts `!@@!0x…!@@!` (or just the hex). `None` = wrong password / invalid data.
pub fn decrypt(data: &str, password: &str) -> Option<String> {
    let inner = data.replace(MARK, "");
    let mut buf = from_hex(&inner)?;
    if buf.is_empty() || buf.len() % 16 != 0 {
        return None;
    }
    let prov = Provider::new()?;
    let key = derive_key(&prov, password)?;
    let mut len = buf.len() as u32;
    if unsafe { CryptDecrypt(key.0, 0, 1, 0, buf.as_mut_ptr(), &mut len) } == 0 {
        return None;
    }
    buf.truncate(len as usize);
    Some(util::ansi_to_string(&buf))
}

// ----------------------------------------------------------------- SHA-1

/// SHA-1 (RFC 3174). It names things (the single-instance mutex, the pipe, the hash of the executable) and is
/// never used as a security measure, so it is done here and does not depend on a provider that could fail.
struct Sha1 {
    state: [u32; 5],
    block: [u8; 64],
    filled: usize,
    total: u64,
}

impl Sha1 {
    fn new() -> Sha1 {
        Sha1 { state: [0x6745_2301, 0xEFCD_AB89, 0x98BA_DCFE, 0x1032_5476, 0xC3D2_E1F0], block: [0; 64], filled: 0, total: 0 }
    }

    fn update(&mut self, mut data: &[u8]) {
        self.total += data.len() as u64;
        while !data.is_empty() {
            let n = data.len().min(64 - self.filled);
            self.block[self.filled..self.filled + n].copy_from_slice(&data[..n]);
            self.filled += n;
            data = &data[n..];
            if self.filled == 64 {
                let b = self.block;
                self.compress(&b);
                self.filled = 0;
            }
        }
    }

    fn finish(mut self) -> [u8; 20] {
        let bits = self.total * 8;
        self.update(&[0x80]);
        while self.filled != 56 {
            self.update(&[0]);
        }
        self.update(&bits.to_be_bytes());
        let mut out = [0u8; 20];
        for (o, s) in out.as_chunks_mut::<4>().0.iter_mut().zip(self.state) {
            *o = s.to_be_bytes();
        }
        out
    }

    fn compress(&mut self, b: &[u8; 64]) {
        let mut w = [0u32; 80];
        for (wi, c) in w.iter_mut().zip(b.as_chunks::<4>().0) {
            *wi = u32::from_be_bytes(*c);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = self.state;
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A82_7999),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let t = a.rotate_left(5).wrapping_add(f).wrapping_add(e).wrapping_add(k).wrapping_add(*wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = t;
        }
        for (s, v) in self.state.iter_mut().zip([a, b, c, d, e]) {
            *s = s.wrapping_add(v);
        }
    }
}

/// SHA-1 as upper-case hexadecimal (like `_Crypt_HashData` without "0x").
pub fn sha1_hex(data: &[u8]) -> String {
    let mut h = Sha1::new();
    h.update(data);
    to_hex(&h.finish())
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
    to_hex(&h.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let e = encrypt("Hello, café 123", "secret").unwrap();
        assert!(e.starts_with("!@@!0x") && e.ends_with("!@@!"));
        assert_eq!(decrypt(&e, "secret").as_deref(), Some("Hello, café 123"));
        assert!(decrypt(&e, "wrong").is_none() || decrypt(&e, "wrong").as_deref() != Some("Hello, café 123"));
    }

    /// Texts encrypted by gaP 2.1.3.0 (the RustCrypto implementation, which was written to read the texts of the
    /// AutoIt gaP): the CryptoAPI of Windows must give exactly the same bytes. ASCII only, so that the code page of
    /// the machine does not matter. The lengths go around the 16-byte block: 0, 1, 15, 16 and 17 bytes.
    const VECTORS: &[(&str, &str, &str)] = &[
        // (text, password, encrypted)
        ("", "secret", "!@@!0x7B8A4D3FC115DBC79BE2ACB2AF8C653E!@@!"),
        ("a", "k", "!@@!0x4CACF58BCCADDDD4D55EC74CE6ED56A3!@@!"),
        ("0123456789abcde", "pw", "!@@!0x3678769F2BCCA07B0BBD6E37215FE324!@@!"),
        ("0123456789abcdef", "pw", "!@@!0x816FB3614847A91665A5168295C8D68574A01F2B231075664CC7EEC0CC4B1006!@@!"),
        ("0123456789abcdefg", "pw", "!@@!0x816FB3614847A91665A5168295C8D68519B4D693C4BC21DA0D17D6F6FB8C16E8!@@!"),
        ("C:\\Program Files\\My App\\app.exe", "my pass phrase", "!@@!0x844887AE2AF59E27ED54EFCADC6242C3AD689E85CFED99C04F4F1684FB8300D1!@@!"),
    ];

    #[test]
    fn known_vectors() {
        for &(text, pass, enc) in VECTORS {
            assert_eq!(encrypt(text, pass).as_deref(), Some(enc), "encrypt {text:?}");
            assert_eq!(decrypt(enc, pass).as_deref(), Some(text), "decrypt {text:?}");
        }
    }

    #[test]
    fn from_hex_is_strict_and_never_panics() {
        assert_eq!(from_hex("0x0aFf"), Some(vec![0x0A, 0xFF]));
        assert_eq!(from_hex("  0X00 "), Some(vec![0]));
        assert_eq!(from_hex(""), Some(vec![]));
        assert_eq!(from_hex("abc"), None); // odd length
        assert_eq!(from_hex("zz"), None);
        assert_eq!(from_hex("+f"), None); // from_str_radix would take the sign
        // not ASCII: before, this cut a character in the middle and panicked
        assert_eq!(from_hex("ação"), None);
        assert_eq!(from_hex("é1"), None);
        assert!(decrypt("!@@!0xção!@@!", "x").is_none());
    }

    #[test]
    fn to_hex_is_upper_case() {
        assert_eq!(to_hex(&[0x00, 0x0F, 0xA5, 0xFF]), "000FA5FF");
        assert_eq!(to_hex(&[]), "");
    }

    #[test]
    fn sha1_known() {
        // The test vectors of RFC 3174 and of FIPS 180: "abc", the empty text, the 56-byte text (the padding spills into
        // a second block), and a million "a" fed in pieces of 997 bytes (the pieces never line up with the 64-byte block).
        assert_eq!(sha1_hex(b"abc"), "A9993E364706816ABA3E25717850C26C9CD0D89D");
        assert_eq!(sha1_hex(b""), "DA39A3EE5E6B4B0D3255BFEF95601890AFD80709");
        assert_eq!(
            sha1_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "84983E441C3BD26EBAAE4AA1F95129E5E54670F1"
        );
        let mut h = Sha1::new();
        let chunk = [b'a'; 997];
        let mut left = 1_000_000usize;
        while left > 0 {
            let n = left.min(chunk.len());
            h.update(&chunk[..n]);
            left -= n;
        }
        assert_eq!(to_hex(&h.finish()), "34AA973CD4C4DAA4F61EEB2BDBAD27316534016F");
    }

    #[test]
    fn sha1_of_a_file() {
        let p = std::env::temp_dir().join(format!("gap_test_sha1_{}.bin", std::process::id()));
        std::fs::write(&p, b"abc").unwrap();
        assert_eq!(sha1_file(&p.to_string_lossy()), "A9993E364706816ABA3E25717850C26C9CD0D89D");
        let _ = std::fs::remove_file(&p);
        assert_eq!(sha1_file(&p.to_string_lossy()), "-1");
    }
}
