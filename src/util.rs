//! General utilities: wide strings, paths, wildcards, text I/O with
//! encoding detection, robust file copy/removal and dates.
//!
//! Replaces the AutoIt helpers: `_PathFull`, `_PathSplit`, `FileExists`
//! (with wildcards), `_FileListToArray`, `FileRead*`, `DirCopy`, `DirMove`,
//! `DirRemove`, `_ReplaceStringInFile`, etc.

use std::fs;
use std::io;
use std::path::Path;

use windows_sys::Win32::Globalization::{MultiByteToWideChar, WideCharToMultiByte};
use windows_sys::Win32::Storage::FileSystem::{
    GetFileAttributesW, SetFileAttributesW, FILE_ATTRIBUTE_READONLY, INVALID_FILE_ATTRIBUTES,
};

// ---------------------------------------------------------------- strings

/// `&str` -> zero-terminated UTF-16.
pub fn w(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// UTF-16 (up to the first zero) -> `String`.
pub fn from_w(buf: &[u16]) -> String {
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..n])
}

/// Case-insensitive comparison (the AutoIt `=` between strings).
pub fn eq_ic(a: &str, b: &str) -> bool {
    let mut x = a.chars().flat_map(|c| c.to_lowercase());
    let mut y = b.chars().flat_map(|c| c.to_lowercase());
    loop {
        match (x.next(), y.next()) {
            (None, None) => return true,
            (Some(p), Some(q)) if p == q => {}
            _ => return false,
        }
    }
}

fn lc(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// Case-insensitive `StringInStr(hay, needle)`: position (in chars) or None.
pub fn find_ic(hay: &str, needle: &str) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    let h: Vec<char> = hay.chars().map(lc).collect();
    let n: Vec<char> = needle.chars().map(lc).collect();
    if n.len() > h.len() {
        return None;
    }
    (0..=h.len() - n.len()).find(|&i| h[i..i + n.len()] == n[..])
}

pub fn contains_ic(hay: &str, needle: &str) -> bool {
    find_ic(hay, needle).is_some()
}

pub fn starts_with_ic(hay: &str, prefix: &str) -> bool {
    let h: Vec<char> = hay.chars().map(lc).collect();
    let p: Vec<char> = prefix.chars().map(lc).collect();
    h.len() >= p.len() && h[..p.len()] == p[..]
}

/// `StringReplace(s, find, rep, 0, 0)`: replaces all, ignoring case.
pub fn replace_all_ic(s: &str, find: &str, rep: &str) -> String {
    if find.is_empty() {
        return s.to_string();
    }
    let chars: Vec<char> = s.chars().collect();
    let low: Vec<char> = chars.iter().map(|&c| lc(c)).collect();
    let f: Vec<char> = find.chars().map(lc).collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        if i + f.len() <= chars.len() && low[i..i + f.len()] == f[..] {
            out.push_str(rep);
            i += f.len();
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// AutoIt-style numeric conversion: empty/non-numeric = 0.
pub fn auto_num(s: &str) -> i64 {
    let t = s.trim();
    let b = t.as_bytes();
    let mut end = 0;
    if end < b.len() && (b[end] == b'-' || b[end] == b'+') {
        end += 1;
    }
    let ds = end;
    while end < b.len() && b[end].is_ascii_digit() {
        end += 1;
    }
    if end == ds {
        return 0;
    }
    t[..end].parse::<i64>().unwrap_or(0)
}

/// Segments "a :: b :: c" (StringSplit with the whole " :: " delimiter).
pub fn split_args(s: &str) -> Vec<String> {
    s.split(" :: ").map(|x| x.to_string()).collect()
}

// ------------------------------------------------------------------- paths

fn is_sep(c: char) -> bool {
    c == '\\' || c == '/'
}

/// Equivalent to `_PathFull($p, $base)`: resolves relative to `base`, normalizes
/// `.`/`..` and slashes. Does not touch the disk.
pub fn path_full(p: &str, base: &str) -> String {
    let p = p.trim();
    let norm = p.replace('/', "\\");
    let bytes: Vec<char> = norm.chars().collect();
    let has_drive = bytes.len() >= 2 && bytes[1] == ':' && bytes[0].is_ascii_alphabetic();
    let is_unc = norm.starts_with("\\\\");
    let joined = if has_drive || is_unc {
        norm
    } else if norm.starts_with('\\') {
        let b = base.replace('/', "\\");
        let drive: String = b.chars().take(2).collect();
        format!("{}{}", drive, norm)
    } else if norm.is_empty() {
        base.replace('/', "\\")
    } else {
        format!("{}\\{}", base.trim_end_matches(is_sep), norm)
    };
    let trailing = joined.ends_with('\\');
    let (prefix, rest) = if let Some(unc) = joined.strip_prefix("\\\\") {
        // UNC: \\server\share
        let parts: Vec<&str> = unc.split('\\').collect();
        if parts.len() >= 2 {
            let pre = format!("\\\\{}\\{}", parts[0], parts[1]);
            let r = unc
                .splitn(3, '\\')
                .nth(2)
                .unwrap_or("")
                .to_string();
            (pre, r)
        } else {
            (joined.clone(), String::new())
        }
    } else {
        (joined.chars().take(2).collect::<String>(), joined.chars().skip(2).collect::<String>())
    };
    let mut stack: Vec<&str> = Vec::new();
    for comp in rest.split('\\') {
        match comp {
            "" | "." => {}
            ".." => {
                stack.pop();
            }
            c => stack.push(c),
        }
    }
    let mut out = prefix;
    for c in &stack {
        out.push('\\');
        out.push_str(c);
    }
    if stack.is_empty() || trailing {
        out.push('\\');
    }
    // "C:\Dir\" -> keeps the trailing slash only if the original had it
    if !trailing && stack.is_empty() {
        // drive root
    }
    out
}

/// Removes ONE trailing slash (not the "C:\" root).
pub fn trim_trailing_sep(p: &str) -> String {
    if p.ends_with('\\') && p.len() > 3 {
        p[..p.len() - 1].to_string()
    } else {
        p.to_string()
    }
}

/// (parent directory, name) of an already normalized path.
pub fn split_parent(p: &str) -> (String, String) {
    let t = trim_trailing_sep(p);
    match t.rfind('\\') {
        Some(i) => (t[..i].to_string(), t[i + 1..].to_string()),
        None => (String::new(), t),
    }
}

/// Last component of the path.
pub fn file_name(p: &str) -> String {
    split_parent(p).1
}

/// Parent directory with the trailing slash preserved for roots ("C:" -> "C:\").
pub fn parent_dir(p: &str) -> String {
    let (mut par, _) = split_parent(p);
    if par.len() == 2 && par.ends_with(':') {
        par.push('\\');
    }
    par
}

pub fn has_extension(name: &str) -> bool {
    match name.rfind('.') {
        Some(i) => i > 0 && i + 1 < name.len(),
        None => false,
    }
}

pub fn has_wildcard(s: &str) -> bool {
    s.contains('*') || s.contains('?')
}

/// Windows-style wildcard (`*` and `?`), case-insensitive.
pub fn wildcard_match(pat: &str, name: &str) -> bool {
    let p: Vec<char> = pat.chars().map(lc).collect();
    let n: Vec<char> = name.chars().map(lc).collect();
    let (mut pi, mut ni) = (0usize, 0usize);
    let (mut star, mut mark) = (None::<usize>, 0usize);
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ni;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ni = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Files,
    Dirs,
    Both,
}

/// `_FileListToArray($dir, $filter, $flag, True)`: full paths.
/// `filter` accepts several patterns separated by `;`.
pub fn list_dir(dir: &str, filter: &str, kind: Kind) -> Vec<String> {
    let pats: Vec<&str> = filter.split(';').map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
    let pats = if pats.is_empty() { vec!["*"] } else { pats };
    let mut out = Vec::new();
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
            let ok_kind = match kind {
                Kind::Files => !is_dir,
                Kind::Dirs => is_dir,
                Kind::Both => true,
            };
            if ok_kind && pats.iter().any(|p| wildcard_match(p, &name)) {
                out.push(format!("{}\\{}", dir.trim_end_matches('\\'), name));
            }
        }
    }
    out.sort_by_key(|s| s.to_lowercase());
    out
}

/// AutoIt `FileExists`: accepts wildcards in the last component.
pub fn exists(p: &str) -> bool {
    if p.is_empty() {
        return false;
    }
    if has_wildcard(p) {
        let (par, name) = split_parent(p);
        return !list_dir(&par, &name, Kind::Both).is_empty();
    }
    Path::new(p).exists()
}

pub fn is_dir(p: &str) -> bool {
    Path::new(p).is_dir()
}

pub fn is_file(p: &str) -> bool {
    Path::new(p).is_file()
}

// ------------------------------------------------------------------- files

fn clear_readonly(p: &str) {
    unsafe {
        let wp = w(p);
        let a = GetFileAttributesW(wp.as_ptr());
        if a != INVALID_FILE_ATTRIBUTES && a & FILE_ATTRIBUTE_READONLY != 0 {
            SetFileAttributesW(wp.as_ptr(), a & !FILE_ATTRIBUTE_READONLY);
        }
    }
}

/// `FileDelete`: deletes file(s), with wildcards; removes the read-only attribute.
pub fn delete_files(p: &str) {
    if has_wildcard(p) {
        let (par, name) = split_parent(p);
        for f in list_dir(&par, &name, Kind::Files) {
            clear_readonly(&f);
            let _ = fs::remove_file(&f);
        }
    } else if Path::new(p).is_file() {
        clear_readonly(p);
        let _ = fs::remove_file(p);
    }
}

/// `DirRemove($d, 1)`: recursive removal tolerant of read-only.
pub fn remove_dir_recursive(p: &str) -> bool {
    if !Path::new(p).exists() {
        return false;
    }
    if fs::remove_dir_all(p).is_ok() {
        return true;
    }
    fn strip(dir: &Path) {
        if let Ok(rd) = fs::read_dir(dir) {
            for e in rd.flatten() {
                let path = e.path();
                clear_readonly(&path.to_string_lossy());
                if e.file_type().map(|t| t.is_dir() && !t.is_symlink()).unwrap_or(false) {
                    strip(&path);
                }
            }
        }
    }
    strip(Path::new(p));
    clear_readonly(p);
    fs::remove_dir_all(p).is_ok()
}

/// `DirRemove($d, 0)`: only removes if empty.
pub fn remove_dir_if_empty(p: &str) -> bool {
    fs::remove_dir(p).is_ok()
}

/// `DirCopy`: copies the content merging; does not overwrite if `overwrite` = false.
pub fn copy_dir(src: &Path, dst: &Path, overwrite: bool) -> io::Result<()> {
    fs::create_dir_all(dst)?;
    for e in fs::read_dir(src)? {
        let e = e?;
        let from = e.path();
        let to = dst.join(e.file_name());
        let ft = e.file_type()?;
        if ft.is_dir() {
            copy_dir(&from, &to, overwrite)?;
        } else if overwrite || !to.exists() {
            if to.exists() {
                clear_readonly(&to.to_string_lossy());
            }
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// Moves a directory (rename, or copy+delete across volumes).
pub fn move_dir(src: &str, dst: &str) -> io::Result<()> {
    if let Some(par) = Path::new(dst).parent() {
        let _ = fs::create_dir_all(par);
    }
    match fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(_) => {
            copy_dir(Path::new(src), Path::new(dst), true)?;
            if remove_dir_recursive(src) {
                Ok(())
            } else {
                Err(io::Error::other("cannot remove source dir"))
            }
        }
    }
}

/// `FileCopy(…, 8|1)`: creates the destination structure; overwrites if requested.
pub fn copy_file(src: &str, dst: &str, overwrite: bool) -> io::Result<()> {
    if !overwrite && Path::new(dst).exists() {
        return Ok(());
    }
    if let Some(par) = Path::new(dst).parent() {
        let _ = fs::create_dir_all(par);
    }
    if Path::new(dst).exists() {
        clear_readonly(dst);
    }
    fs::copy(src, dst).map(|_| ())
}

/// `FileMove(…, 8|1)`.
pub fn move_file(src: &str, dst: &str, overwrite: bool) -> io::Result<()> {
    if !overwrite && Path::new(dst).exists() {
        return Ok(());
    }
    if let Some(par) = Path::new(dst).parent() {
        let _ = fs::create_dir_all(par);
    }
    if Path::new(dst).exists() {
        clear_readonly(dst);
        let _ = fs::remove_file(dst);
    }
    match fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(_) => {
            fs::copy(src, dst)?;
            clear_readonly(src);
            fs::remove_file(src)
        }
    }
}

// --------------------------------------------------------- text and encoding

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Enc {
    Ansi,
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
}

pub fn ansi_to_string(b: &[u8]) -> String {
    if b.is_empty() {
        return String::new();
    }
    unsafe {
        let n = MultiByteToWideChar(0, 0, b.as_ptr(), b.len() as i32, std::ptr::null_mut(), 0);
        if n <= 0 {
            return String::from_utf8_lossy(b).into_owned();
        }
        let mut buf = vec![0u16; n as usize];
        MultiByteToWideChar(0, 0, b.as_ptr(), b.len() as i32, buf.as_mut_ptr(), n);
        String::from_utf16_lossy(&buf)
    }
}

pub fn string_to_ansi(s: &str) -> Vec<u8> {
    if s.is_empty() {
        return Vec::new();
    }
    let wide: Vec<u16> = s.encode_utf16().collect();
    unsafe {
        let n = WideCharToMultiByte(
            0, 0, wide.as_ptr(), wide.len() as i32, std::ptr::null_mut(), 0,
            std::ptr::null(), std::ptr::null_mut(),
        );
        if n <= 0 {
            return s.as_bytes().to_vec();
        }
        let mut buf = vec![0u8; n as usize];
        WideCharToMultiByte(
            0, 0, wide.as_ptr(), wide.len() as i32, buf.as_mut_ptr(), n,
            std::ptr::null(), std::ptr::null_mut(),
        );
        buf
    }
}

pub fn decode_bytes(b: &[u8]) -> (String, Enc) {
    if b.starts_with(&[0xFF, 0xFE]) {
        let u: Vec<u16> = b[2..].as_chunks::<2>().0.iter().map(|&c| u16::from_le_bytes(c)).collect();
        return (String::from_utf16_lossy(&u), Enc::Utf16Le);
    }
    if b.starts_with(&[0xFE, 0xFF]) {
        let u: Vec<u16> = b[2..].as_chunks::<2>().0.iter().map(|&c| u16::from_be_bytes(c)).collect();
        return (String::from_utf16_lossy(&u), Enc::Utf16Be);
    }
    if b.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return (String::from_utf8_lossy(&b[3..]).into_owned(), Enc::Utf8Bom);
    }
    match std::str::from_utf8(b) {
        Ok(s) if b.iter().any(|&x| x >= 0x80) => (s.to_string(), Enc::Utf8),
        _ => (ansi_to_string(b), Enc::Ansi),
    }
}

pub fn encode_string(s: &str, enc: Enc) -> Vec<u8> {
    match enc {
        Enc::Ansi => string_to_ansi(s),
        Enc::Utf8 => s.as_bytes().to_vec(),
        Enc::Utf8Bom => {
            let mut v = vec![0xEF, 0xBB, 0xBF];
            v.extend_from_slice(s.as_bytes());
            v
        }
        Enc::Utf16Le => {
            let mut v = vec![0xFF, 0xFE];
            for u in s.encode_utf16() {
                v.extend_from_slice(&u.to_le_bytes());
            }
            v
        }
        Enc::Utf16Be => {
            let mut v = vec![0xFE, 0xFF];
            for u in s.encode_utf16() {
                v.extend_from_slice(&u.to_be_bytes());
            }
            v
        }
    }
}

/// Reads a text file detecting the encoding.
pub fn read_text(path: &str) -> Option<(String, Enc)> {
    fs::read(path).ok().map(|b| decode_bytes(&b))
}

pub fn write_text(path: &str, s: &str, enc: Enc) -> io::Result<()> {
    if let Some(par) = Path::new(path).parent() {
        if !par.as_os_str().is_empty() {
            let _ = fs::create_dir_all(par);
        }
    }
    fs::write(path, encode_string(s, enc))
}

/// Splits into lines preserving the terminator of each one.
pub fn split_lines_keep(s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '\r' => {
                if it.peek() == Some(&'\n') {
                    it.next();
                    out.push((std::mem::take(&mut cur), "\r\n".to_string()));
                } else {
                    out.push((std::mem::take(&mut cur), "\r".to_string()));
                }
            }
            '\n' => out.push((std::mem::take(&mut cur), "\n".to_string())),
            _ => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push((cur, String::new()));
    }
    out
}

// -------------------------------------------------------------------- dates

#[derive(Clone, Copy, Debug)]
pub struct LocalTime {
    pub year: u16,
    pub month: u16,
    pub day: u16,
    pub hour: u16,
    pub min: u16,
    pub sec: u16,
}

pub fn local_now() -> LocalTime {
    use windows_sys::Win32::System::SystemInformation::GetLocalTime;
    unsafe {
        let mut st = std::mem::zeroed();
        GetLocalTime(&mut st);
        LocalTime {
            year: st.wYear,
            month: st.wMonth,
            day: st.wDay,
            hour: st.wHour,
            min: st.wMinute,
            sec: st.wSecond,
        }
    }
}

/// Seconds since 1970-01-01 00:00:00 UTC (fixes the `{U}` of the original, which
/// used local time).
pub fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
