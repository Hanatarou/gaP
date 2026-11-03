//! Windows registry (native API). Replaces the `REG.EXE ADD/DELETE/COPY/QUERY`
//! calls made through `cmd /C` in the original (faster and without risk of command
//! injection). Only `REG EXPORT`/`REG IMPORT` still use reg.exe,
//! to keep exactly the format of the .reg files.

use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Storage::FileSystem::{DELETE, WRITE_DAC};
use windows_sys::Win32::System::Registry::*;

use crate::util::{from_w, w};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum View {
    /// Native view of the process.
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
    /// The same view as a flag of `RegGetValueW`.
    fn get_flag(self) -> u32 {
        match self {
            View::Native => 0,
            View::X86 => RRF_SUBKEY_WOW6432KEY,
            View::X64 => RRF_SUBKEY_WOW6464KEY,
        }
    }
    pub fn reg_exe_flag(self) -> &'static str {
        match self {
            View::X64 => "/reg:64",
            _ => "/reg:32",
        }
    }
}

/// "HKCU\Software\X" -> (root HKEY, "Software\X").
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

fn open(root: HKEY, sub: &str, access: u32, view: View) -> Result<Key, u32> {
    let mut h: HKEY = null_mut();
    let r = unsafe { RegOpenKeyExW(root, w(sub).as_ptr(), 0, access | view.sam(), &mut h) };
    if r == 0 {
        Ok(Key(h))
    } else {
        Err(r)
    }
}

/// Creates the key (or opens it if it exists) with the rights to add values and subkeys and no more: not the
/// right to change who owns the key or who can use it.
fn create(root: HKEY, sub: &str, view: View) -> Result<Key, u32> {
    create_access(root, sub, KEY_WRITE, view)
}

/// `create` with the rights `access`.
fn create_access(root: HKEY, sub: &str, access: u32, view: View) -> Result<Key, u32> {
    let mut h: HKEY = null_mut();
    let r = unsafe {
        RegCreateKeyExW(root, w(sub).as_ptr(), 0, null(), 0, access | view.sam(), null(), &mut h, null_mut())
    };
    if r == 0 {
        Ok(Key(h))
    } else {
        Err(r)
    }
}

/// 0 = does not exist; 1 = exists with content; 2 = exists and is empty.
pub fn key_state(path: &str, view: View) -> u8 {
    let Some((root, sub)) = parse(path) else { return 0 };
    let Ok(k) = open(root, &sub, KEY_READ, view) else { return 0 };
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
    let (wsub, wname) = (w(&sub), w(name));
    let psub = if sub.is_empty() { null() } else { wsub.as_ptr() };
    // Unexpanded, whatever the type; the system opens the key in the requested view by itself.
    let flags = RRF_RT_ANY | RRF_NOEXPAND | view.get_flag();
    unsafe {
        let (mut kind, mut size) = (0u32, 0u32);
        if RegGetValueW(root, psub, wname.as_ptr(), flags, &mut kind, null_mut(), &mut size) != 0 {
            return None;
        }
        // The value can grow between the two calls: ask again with the new size.
        for _ in 0..4 {
            let mut buf = vec![0u8; size as usize + 2];
            let mut got = buf.len() as u32;
            match RegGetValueW(root, psub, wname.as_ptr(), flags, &mut kind, buf.as_mut_ptr() as *mut _, &mut got) {
                0 => {
                    buf.truncate(got as usize);
                    return Some(Value { kind, data: buf });
                }
                ERROR_MORE_DATA => size = got,
                _ => return None,
            }
        }
        None
    }
}

pub fn value_exists(key: &str, name: &str, view: View) -> bool {
    read_value(key, name, view).is_some()
}

/// REG_SZ/EXPAND_SZ value as text (for versions, Shell Folders, etc.).
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

/// Writes the value (the key is created if it does not exist). The error is the system code.
pub fn write_value(key: &str, name: &str, kind: u32, data: &[u8], view: View) -> Result<(), u32> {
    let (root, sub) = parse(key).ok_or(ERROR_INVALID_PARAMETER)?;
    let k = create(root, &sub, view)?;
    match unsafe { RegSetValueExW(k.0, w(name).as_ptr(), 0, kind, data.as_ptr(), data.len() as u32) } {
        0 => Ok(()),
        e => Err(e),
    }
}

/// Deletes one value. The error is the system code (`ERROR_FILE_NOT_FOUND`: the key or the value is not there).
pub fn delete_value(key: &str, name: &str, view: View) -> Result<(), u32> {
    let (root, sub) = parse(key).ok_or(ERROR_INVALID_PARAMETER)?;
    let k = open(root, &sub, KEY_SET_VALUE, view)?;
    match unsafe { RegDeleteValueW(k.0, w(name).as_ptr()) } {
        0 => Ok(()),
        e => Err(e),
    }
}

/// Deletes the values `names` of the key, opening it once. Returns how many were deleted (a value that does not
/// exist is not an error, it just does not count).
pub fn delete_values(key: &str, names: &[String], view: View) -> usize {
    let Some((root, sub)) = parse(key) else { return 0 };
    let Ok(k) = open(root, &sub, KEY_SET_VALUE, view) else { return 0 };
    names.iter().filter(|n| unsafe { RegDeleteValueW(k.0, w(n).as_ptr()) } == 0).count()
}

/// Creates the key (nothing happens if it exists). The error is the system code.
pub fn create_key(path: &str, view: View) -> Result<(), u32> {
    let (root, sub) = parse(path).ok_or(ERROR_INVALID_PARAMETER)?;
    create(root, &sub, view).map(|_| ())
}

/// Deletes the key and its whole subtree. The error is the system code.
pub fn delete_tree(path: &str, view: View) -> Result<(), u32> {
    let (root, sub) = parse(path).ok_or(ERROR_INVALID_PARAMETER)?;
    if sub.is_empty() {
        return Err(ERROR_INVALID_PARAMETER);
    }
    let wsub = w(&sub);
    // The root, opened in the wanted view, takes the whole path: the parent of the key is never opened. The rights
    // that the documentation of RegDeleteTree lists come first, so the usual case needs one open and one call;
    // KEY_READ is the fallback for a root that cannot be opened with them (HKLM, HKCR for a standard user).
    let mut last = ERROR_ACCESS_DENIED;
    for access in [DELETE | KEY_ENUMERATE_SUB_KEYS | KEY_QUERY_VALUE | KEY_SET_VALUE, KEY_READ] {
        let mut h: HKEY = null_mut();
        let r = unsafe { RegOpenKeyExW(root, null(), 0, access | view.sam(), &mut h) };
        if r != 0 {
            last = r;
            continue;
        }
        let k = Key(h);
        match unsafe { RegDeleteTreeW(k.0, wsub.as_ptr()) } {
            0 => return Ok(()),
            ERROR_ACCESS_DENIED => last = ERROR_ACCESS_DENIED,
            e => return Err(e),
        }
    }
    Err(last)
}

/// Deletes every value and subkey of the key but not the key itself (`RegDeleteTree` with no subkey). A key that is
/// not there is not an error. Unlike `delete_tree`, the name of the key is never left in the "being deleted" state
/// that makes the next create of it fail with "access denied" while another program still has the key open.
pub fn clear_tree(path: &str, view: View) -> Result<(), u32> {
    let (root, sub) = parse(path).ok_or(ERROR_INVALID_PARAMETER)?;
    if sub.is_empty() {
        return Err(ERROR_INVALID_PARAMETER);
    }
    let access = DELETE | KEY_ENUMERATE_SUB_KEYS | KEY_QUERY_VALUE | KEY_SET_VALUE;
    let k = match open(root, &sub, access, view) {
        Ok(k) => k,
        Err(ERROR_FILE_NOT_FOUND) => return Ok(()),
        Err(e) => return Err(e),
    };
    match unsafe { RegDeleteTreeW(k.0, null()) } {
        0 => Ok(()),
        e => Err(e),
    }
}

/// Why a copy of a key failed: the system code and the step that returned it.
#[derive(Clone, Copy, Debug)]
pub struct CopyError {
    pub code: u32,
    pub step: &'static str,
}

/// The step of `copy_tree` that opens the source: its `ERROR_FILE_NOT_FOUND` means "nothing to copy".
pub const STEP_OPEN_SOURCE: &str = "open source";

/// A key that another program still has open after it was deleted stays in the system until that program closes it,
/// and in the meantime the system answers "access denied" (or "key deleted") to whoever creates it again. The program
/// usually closes it within moments, so a copy that fails this way is tried again for a short time.
const COPY_RETRIES: u32 = 25;
const COPY_RETRY_MS: u64 = 100;

fn copy_busy(code: u32) -> bool {
    code == ERROR_ACCESS_DENIED || code == ERROR_KEY_DELETED
}

/// `RegCopyTree` also copies the security descriptor of the key, and the documentation lists only `KEY_CREATE_SUB_KEY`
/// among the rights the target needs. Reports of programs that open the target with the documented rights only say it
/// answers "access denied" for keys whose descriptor is not the inherited one, and that the full rights fix it. The
/// widest rights the user has come first and the narrow `KEY_WRITE` last, so a target that cannot be opened with the
/// wide ones is copied as before.
const TARGET_ACCESS: [u32; 3] = [KEY_ALL_ACCESS, KEY_WRITE | WRITE_DAC, KEY_WRITE];

fn copy_once(sk: &Key, dr: HKEY, ds: &str, view: View) -> Result<(), CopyError> {
    let mut last = ERROR_ACCESS_DENIED;
    for access in TARGET_ACCESS {
        match create_access(dr, ds, access, view) {
            Ok(dk) => {
                return match unsafe { RegCopyTreeW(sk.0, null(), dk.0) } {
                    0 => Ok(()),
                    code => Err(CopyError { code, step: "copy tree" }),
                };
            }
            Err(code) => last = code,
        }
    }
    Err(CopyError { code: last, step: "create target" })
}

/// Copies values and subkeys from `src` to `dst` (REG COPY /s /f). The error has the system code and the step that
/// failed; `ERROR_FILE_NOT_FOUND` in the step `STEP_OPEN_SOURCE` means the source does not exist.
pub fn copy_tree(src: &str, dst: &str, view: View) -> Result<(), CopyError> {
    let (Some((sr, ss)), Some((dr, ds))) = (parse(src), parse(dst)) else {
        return Err(CopyError { code: ERROR_INVALID_PARAMETER, step: "read the paths" });
    };
    let sk = open(sr, &ss, KEY_READ, view).map_err(|code| CopyError { code, step: STEP_OPEN_SOURCE })?;
    let mut tries = 0;
    loop {
        match copy_once(&sk, dr, &ds, view) {
            Err(e) if copy_busy(e.code) && tries < COPY_RETRIES => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(COPY_RETRY_MS));
            }
            other => return other,
        }
    }
}

/// The path is under HKEY_CURRENT_USER, where an administrator right is not what a change needs.
pub fn is_hkcu(path: &str) -> bool {
    parse(path).is_some_and(|(root, _)| root == HKEY_CURRENT_USER)
}

pub fn enum_subkeys(path: &str, view: View) -> Vec<String> {
    let mut out = Vec::new();
    let Some((root, sub)) = parse(path) else { return out };
    let Ok(k) = open(root, &sub, KEY_READ, view) else { return out };
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

/// Removes empty parent keys below HKCU\Software / HKLM\Software
/// (as described in the help; the original removed only the key itself).
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
    // parts = ["Software", "Vendor", "App"]; starts at the direct parent of "App".
    parts.pop();
    while parts.len() > 1 {
        let p = format!("{}\\{}", rootname, parts.join("\\"));
        if key_state(&p, view) == 2 {
            let _ = delete_tree(&p, view);
            parts.pop();
        } else {
            break;
        }
    }
}

/// Converts the text of `+regvalue` into (type, bytes).
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
            let sep = if sep.is_empty() || crate::util::is_null(sep) { "\\0" } else { sep };
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

#[cfg(test)]
mod tests {
    use super::*;

    fn units(bytes: &[u8]) -> Vec<u16> {
        bytes.chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect()
    }

    /// `+regvalue` with REG_SZ writes the text exactly as typed: quotes, backslashes and `%1`
    /// are kept (no `.reg`-style escaping), ended by a single terminating NUL.
    #[test]
    fn reg_sz_keeps_quotes_and_backslashes() {
        let text = r#""C:\Program Files (x86)\MyApp\MyApp.exe" "%1""#;
        let (kind, bytes) = build_value("REG_SZ", text, "").unwrap();
        assert_eq!(kind, REG_SZ);
        let u = units(&bytes);
        assert_eq!(u.last(), Some(&0));
        assert_eq!(String::from_utf16(&u[..u.len() - 1]).unwrap(), text);
    }

    #[test]
    fn reg_expand_sz_keeps_percent_signs() {
        let (kind, bytes) = build_value("REG_EXPAND_SZ", r"%SystemRoot%\System32", "").unwrap();
        assert_eq!(kind, REG_EXPAND_SZ);
        let u = units(&bytes);
        assert_eq!(String::from_utf16(&u[..u.len() - 1]).unwrap(), r"%SystemRoot%\System32");
    }

    /// Binary data is written with the type REG_BINARY; "hex:" is `.reg` syntax, not gaP syntax.
    #[test]
    fn reg_binary_from_comma_separated_hex() {
        let (kind, bytes) = build_value("REG_BINARY", "01,00,00", "").unwrap();
        assert_eq!(kind, REG_BINARY);
        assert_eq!(bytes, vec![1u8, 0, 0]);
    }
}
