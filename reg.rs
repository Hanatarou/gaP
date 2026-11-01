//! Registro do Windows (API nativa). Substitui os `REG.EXE ADD/DELETE/COPY/QUERY`
//! chamados via `cmd /C` no original (mais rápido e sem risco de injeção de
//! comandos). Somente `REG EXPORT`/`REG IMPORT` continuam usando o reg.exe,
//! para manter exatamente o formato dos arquivos .reg.

use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::System::Registry::*;

use crate::util::{from_w, w};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum View {
    /// Visão nativa do processo.
    Native,
    X86,
    X64,
}

impl View {
    pub fn from_flag(s: &str) -> View {
        if crate::util::eq_ic(s, "x64") {
            View::X64
        } else {
            View::X86
        }
    }
    fn sam(self) -> u32 {
        match self {
            View::Native => 0,
            View::X86 => KEY_WOW64_32KEY,
            View::X64 => KEY_WOW64_64KEY,
        }
    }
    pub fn reg_exe_flag(self) -> &'static str {
        match self {
            View::X64 => "/reg:64",
            _ => "/reg:32",
        }
    }
}

/// "HKCU\Software\X" -> (HKEY raiz, "Software\X").
pub fn parse(path: &str) -> Option<(HKEY, String)> {
    let p = path.trim().trim_start_matches('\\');
    let (root, sub) = match p.find('\\') {
        Some(i) => (&p[..i], &p[i + 1..]),
        None => (p, ""),
    };
    let r = root.to_ascii_uppercase();
    let h = match r.as_str() {
        "HKLM" | "HKEY_LOCAL_MACHINE" => HKEY_LOCAL_MACHINE,
        "HKCU" | "HKEY_CURRENT_USER" => HKEY_CURRENT_USER,
        "HKCR" | "HKEY_CLASSES_ROOT" => HKEY_CLASSES_ROOT,
        "HKU" | "HKEY_USERS" => HKEY_USERS,
        "HKCC" | "HKEY_CURRENT_CONFIG" => HKEY_CURRENT_CONFIG,
        _ => return None,
    };
    Some((h, sub.trim_end_matches('\\').to_string()))
}

struct Key(HKEY);
impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            RegCloseKey(self.0);
        }
    }
}

fn open(root: HKEY, sub: &str, access: u32, view: View) -> Option<Key> {
    let mut h: HKEY = null_mut();
    let r = unsafe { RegOpenKeyExW(root, w(sub).as_ptr(), 0, access | view.sam(), &mut h) };
    if r == 0 {
        Some(Key(h))
    } else {
        None
    }
}

fn create(root: HKEY, sub: &str, view: View) -> Option<Key> {
    let mut h: HKEY = null_mut();
    let r = unsafe {
        RegCreateKeyExW(root, w(sub).as_ptr(), 0, null(), 0, KEY_ALL_ACCESS | view.sam(), null(), &mut h, null_mut())
    };
    if r == 0 {
        Some(Key(h))
    } else {
        None
    }
}

/// 0 = não existe; 1 = existe com conteúdo; 2 = existe e está vazia.
pub fn key_state(path: &str, view: View) -> u8 {
    let Some((root, sub)) = parse(path) else { return 0 };
    let Some(k) = open(root, &sub, KEY_READ, view) else { return 0 };
    let (mut subkeys, mut values) = (0u32, 0u32);
    unsafe {
        RegQueryInfoKeyW(
            k.0, null_mut(), null_mut(), null(), &mut subkeys, null_mut(), null_mut(), &mut values,
            null_mut(), null_mut(), null_mut(), null_mut(),
        );
    }
    if subkeys == 0 && values == 0 {
        2
    } else {
        1
    }
}

pub fn key_exists(path: &str, view: View) -> bool {
    key_state(path, view) > 0
}

pub struct Value {
    pub kind: u32,
    pub data: Vec<u8>,
}

pub fn read_value(key: &str, name: &str, view: View) -> Option<Value> {
    let (root, sub) = parse(key)?;
    let k = open(root, &sub, KEY_QUERY_VALUE, view)?;
    unsafe {
        let wn = w(name);
        let (mut kind, mut size) = (0u32, 0u32);
        if RegQueryValueExW(k.0, wn.as_ptr(), null(), &mut kind, null_mut(), &mut size) != 0 {
            return None;
        }
        let mut buf = vec![0u8; size as usize + 2];
        let mut sz2 = size;
        let mut kind2 = kind;
        let r = RegQueryValueExW(k.0, wn.as_ptr(), null(), &mut kind2, buf.as_mut_ptr(), &mut sz2);
        if r != 0 {
            return None;
        }
        buf.truncate(sz2 as usize);
        Some(Value { kind: kind2, data: buf })
    }
}

pub fn value_exists(key: &str, name: &str, view: View) -> bool {
    read_value(key, name, view).is_some()
}

/// Valor REG_SZ/EXPAND_SZ como texto (para versões, Shell Folders, etc.).
pub fn read_string(key: &str, name: &str, view: View) -> Option<String> {
    let v = read_value(key, name, view)?;
    match v.kind {
        REG_SZ | REG_EXPAND_SZ => {
            let u: Vec<u16> = v.data.as_chunks::<2>().0.iter().map(|&c| u16::from_le_bytes(c)).collect();
            Some(from_w(&u))
        }
        REG_DWORD if v.data.len() >= 4 => {
            Some(u32::from_le_bytes([v.data[0], v.data[1], v.data[2], v.data[3]]).to_string())
        }
        _ => None,
    }
}

pub fn read_dword(key: &str, name: &str, view: View) -> Option<u32> {
    let v = read_value(key, name, view)?;
    if v.kind == REG_DWORD && v.data.len() >= 4 {
        Some(u32::from_le_bytes([v.data[0], v.data[1], v.data[2], v.data[3]]))
    } else {
        None
    }
}

pub fn write_value(key: &str, name: &str, kind: u32, data: &[u8], view: View) -> bool {
    let Some((root, sub)) = parse(key) else { return false };
    let Some(k) = create(root, &sub, view) else { return false };
    unsafe { RegSetValueExW(k.0, w(name).as_ptr(), 0, kind, data.as_ptr(), data.len() as u32) == 0 }
}

pub fn delete_value(key: &str, name: &str, view: View) -> bool {
    let Some((root, sub)) = parse(key) else { return false };
    let Some(k) = open(root, &sub, KEY_SET_VALUE, view) else { return false };
    unsafe { RegDeleteValueW(k.0, w(name).as_ptr()) == 0 }
}

pub fn create_key(path: &str, view: View) -> bool {
    let Some((root, sub)) = parse(path) else { return false };
    create(root, &sub, view).is_some()
}

/// Apaga a chave e toda a sua subárvore.
pub fn delete_tree(path: &str, view: View) -> bool {
    let Some((root, sub)) = parse(path) else { return false };
    if sub.is_empty() {
        return false;
    }
    let (parent, name) = match sub.rfind('\\') {
        Some(i) => (sub[..i].to_string(), sub[i + 1..].to_string()),
        None => (String::new(), sub.clone()),
    };
    let pk = if parent.is_empty() {
        // chave de primeiro nível: abre a própria raiz com a visão desejada
        let mut h: HKEY = null_mut();
        let r = unsafe { RegOpenKeyExW(root, null(), 0, KEY_ALL_ACCESS | view.sam(), &mut h) };
        if r == 0 { Some(Key(h)) } else { None }
    } else {
        open(root, &parent, KEY_ALL_ACCESS, view)
    };
    let Some(pk) = pk else { return false };
    unsafe { RegDeleteTreeW(pk.0, w(&name).as_ptr()) == 0 }
}

/// Copia valores e subchaves de `src` para `dst` (REG COPY /s /f).
pub fn copy_tree(src: &str, dst: &str, view: View) -> bool {
    let (Some((sr, ss)), Some((dr, ds))) = (parse(src), parse(dst)) else { return false };
    let Some(sk) = open(sr, &ss, KEY_READ, view) else { return false };
    let Some(dk) = create(dr, &ds, view) else { return false };
    unsafe { RegCopyTreeW(sk.0, null(), dk.0) == 0 }
}

pub fn enum_subkeys(path: &str, view: View) -> Vec<String> {
    let mut out = Vec::new();
    let Some((root, sub)) = parse(path) else { return out };
    let Some(k) = open(root, &sub, KEY_READ, view) else { return out };
    let mut i = 0;
    loop {
        let mut buf = [0u16; 256];
        let mut len = buf.len() as u32;
        let r = unsafe {
            RegEnumKeyExW(k.0, i, buf.as_mut_ptr(), &mut len, null(), null_mut(), null_mut(), null_mut())
        };
        if r != 0 {
            break;
        }
        out.push(from_w(&buf[..len as usize]));
        i += 1;
    }
    out
}

/// Remove chaves-pai vazias abaixo de HKCU\Software / HKLM\Software
/// (conforme descrito na ajuda; o original só removia a própria chave).
pub fn delete_empty_parents(path: &str, view: View) {
    let Some((root, sub)) = parse(path) else { return };
    if root != HKEY_CURRENT_USER && root != HKEY_LOCAL_MACHINE {
        return;
    }
    let rootname = if root == HKEY_CURRENT_USER { "HKCU" } else { "HKLM" };
    let mut parts: Vec<&str> = sub.split('\\').collect();
    if parts.is_empty() || !crate::util::eq_ic(parts[0], "Software") {
        return;
    }
    // parts = ["Software", "Vendor", "App"]; começa pelo pai direto de "App".
    parts.pop();
    while parts.len() > 1 {
        let p = format!("{}\\{}", rootname, parts.join("\\"));
        if key_state(&p, view) == 2 {
            delete_tree(&p, view);
            parts.pop();
        } else {
            break;
        }
    }
}

/// Converte o texto do `+regvalue` para (tipo, bytes).
pub fn build_value(kind_name: &str, data: &str, sep: &str) -> Option<(u32, Vec<u8>)> {
    let k = kind_name.to_ascii_uppercase();
    let utf16z = |s: &str| -> Vec<u8> {
        s.encode_utf16().chain(std::iter::once(0)).flat_map(|c| c.to_le_bytes()).collect()
    };
    let parse_num = |s: &str| -> Option<u64> {
        let t = s.trim();
        if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
            u64::from_str_radix(h, 16).ok()
        } else {
            t.parse::<u64>().ok()
        }
    };
    match k.as_str() {
        "REG_SZ" => Some((REG_SZ, utf16z(data))),
        "REG_EXPAND_SZ" => Some((REG_EXPAND_SZ, utf16z(data))),
        "REG_MULTI_SZ" => {
            let sep = if sep.is_empty() || sep == "NULL" { "\\0" } else { sep };
            let mut u: Vec<u16> = Vec::new();
            if !data.is_empty() {
                for part in data.split(sep) {
                    u.extend(part.encode_utf16());
                    u.push(0);
                }
            }
            u.push(0);
            if data.is_empty() {
                u.push(0);
            }
            Some((REG_MULTI_SZ, u.iter().flat_map(|c| c.to_le_bytes()).collect()))
        }
        "REG_DWORD" | "REG_DWORD_LITTLE_ENDIAN" => {
            Some((REG_DWORD, (parse_num(data)? as u32).to_le_bytes().to_vec()))
        }
        "REG_DWORD_BIG_ENDIAN" => Some((REG_DWORD_BIG_ENDIAN, (parse_num(data)? as u32).to_be_bytes().to_vec())),
        "REG_QWORD" | "REG_QWORD_LITTLE_ENDIAN" => Some((REG_QWORD, parse_num(data)?.to_le_bytes().to_vec())),
        "REG_BINARY" | "REG_NONE" => {
            let hex: String = data.chars().filter(|c| c.is_ascii_hexdigit()).collect();
            let bytes = crate::crypto::from_hex(&hex)?;
            Some((if k == "REG_NONE" { REG_NONE } else { REG_BINARY }, bytes))
        }
        _ => None,
    }
}

#[allow(dead_code)]
pub fn type_name(kind: u32) -> &'static str {
    match kind {
        REG_NONE => "REG_NONE",
        REG_SZ => "REG_SZ",
        REG_EXPAND_SZ => "REG_EXPAND_SZ",
        REG_BINARY => "REG_BINARY",
        REG_DWORD => "REG_DWORD",
        REG_DWORD_BIG_ENDIAN => "REG_DWORD_BIG_ENDIAN",
        REG_LINK => "REG_LINK",
        REG_MULTI_SZ => "REG_MULTI_SZ",
        REG_QWORD => "REG_QWORD",
        _ => "REG_BINARY",
    }
}

#[allow(dead_code)]
pub const ERROR_OK: u32 = ERROR_SUCCESS;
