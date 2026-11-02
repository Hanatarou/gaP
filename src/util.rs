//! General utilities: wide strings, paths, wildcards, text I/O with
//! encoding detection, robust file copy/removal and dates.
//!
//! Replaces the AutoIt helpers: `_PathFull`, `_PathSplit`, `FileExists`
//! (with wildcards), `_FileListToArray`, `FileRead*`, `DirCopy`, `DirMove`,
//! `DirRemove`, `_ReplaceStringInFile`, etc.

use std::fs;
use std::io;
use std::path::Path;

use windows_sys::Win32::Globalization::{
    CompareStringOrdinal, FindStringOrdinal, MultiByteToWideChar, WideCharToMultiByte, CSTR_EQUAL, FIND_FROMSTART,
};
use windows_sys::Win32::Storage::FileSystem::{
    GetFileAttributesW, MoveFileExW, SetFileAttributesW, FILE_ATTRIBUTE_READONLY, INVALID_FILE_ATTRIBUTES,
    MOVEFILE_COPY_ALLOWED, MOVEFILE_REPLACE_EXISTING,
};

// ---------------------------------------------------------------- strings

/// The word the `.ini` uses for "no value" in an optional segment.
pub const NULL: &str = "NULL";

/// `$x = "NULL"` of the AutoIt original: the `=` between strings ignores the case, so `null` and
/// `Null` mean "no value" too.
pub fn is_null(s: &str) -> bool {
    eq_ic(s, NULL)
}

/// Segment `i` of a keyword, or "NULL" when the segment was not written.
pub fn arg(a: &[String], i: usize) -> &str {
    a.get(i).map(|s| s.as_str()).unwrap_or(NULL)
}

/// `&str` -> zero-terminated UTF-16.
pub fn w(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// UTF-16 (up to the first zero) -> `String`.
pub fn from_w(buf: &[u16]) -> String {
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..n])
}

fn utf16(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

/// Case-insensitive comparison (the AutoIt `=` between strings). ASCII is compared directly; any other
/// text goes to the operating system, which folds the case the way the file systems do.
pub fn eq_ic(a: &str, b: &str) -> bool {
    if a.is_ascii() && b.is_ascii() {
        return a.eq_ignore_ascii_case(b);
    }
    let (x, y) = (utf16(a), utf16(b));
    unsafe { CompareStringOrdinal(x.as_ptr(), x.len() as i32, y.as_ptr(), y.len() as i32, 1) == CSTR_EQUAL }
}

/// Position (in UTF-16 units) of the first case-insensitive `needle` in `hay[from..]`.
fn find_u16(hay: &[u16], from: usize, needle: &[u16]) -> Option<usize> {
    if needle.is_empty() || from >= hay.len() {
        return None;
    }
    let r = unsafe {
        FindStringOrdinal(FIND_FROMSTART, hay[from..].as_ptr(), (hay.len() - from) as i32, needle.as_ptr(), needle.len() as i32, 1)
    };
    (r >= 0).then(|| from + r as usize)
}

/// Case-insensitive `StringInStr(hay, needle)` as a yes/no (an empty needle is never found).
pub fn contains_ic(hay: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    if hay.is_ascii() && needle.is_ascii() {
        return hay.len() >= needle.len()
            && hay.as_bytes().windows(needle.len()).any(|win| win.eq_ignore_ascii_case(needle.as_bytes()));
    }
    find_u16(&utf16(hay), 0, &utf16(needle)).is_some()
}

pub fn starts_with_ic(hay: &str, prefix: &str) -> bool {
    if hay.is_ascii() && prefix.is_ascii() {
        return hay.len() >= prefix.len() && hay.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes());
    }
    let (h, p) = (utf16(hay), utf16(prefix));
    h.len() >= p.len()
        && unsafe { CompareStringOrdinal(h.as_ptr(), p.len() as i32, p.as_ptr(), p.len() as i32, 1) == CSTR_EQUAL }
}

/// `StringReplace(s, find, rep, 0, 0)`: replaces all, ignoring case.
pub fn replace_all_ic(s: &str, find: &str, rep: &str) -> String {
    if find.is_empty() {
        return s.to_string();
    }
    let (h, f, r) = (utf16(s), utf16(find), utf16(rep));
    let mut out: Vec<u16> = Vec::with_capacity(h.len());
    let (mut pos, mut any) = (0, false);
    while let Some(i) = find_u16(&h, pos, &f) {
        out.extend_from_slice(&h[pos..i]);
        out.extend_from_slice(&r);
        pos = i + f.len();
        any = true;
    }
    if !any {
        return s.to_string();
    }
    out.extend_from_slice(&h[pos..]);
    String::from_utf16_lossy(&out)
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
    // "C:\Dir\" keeps the trailing slash only if the original had it; a bare root always has it.
    if stack.is_empty() || trailing {
        out.push('\\');
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

fn lc(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
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

/// Entries of `dir` that match `filter` (one or more patterns separated by `;`) and `kind`, as full paths and in
/// the order the file system gives them. `stop_at_first` ends the scan at the first match.
fn scan_dir(dir: &str, filter: &str, kind: Kind, stop_at_first: bool) -> Vec<String> {
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
                if stop_at_first {
                    break;
                }
            }
        }
    }
    out
}

/// `_FileListToArray($dir, $filter, $flag, True)`: full paths, sorted by name ignoring the case.
/// `filter` accepts several patterns separated by `;`.
pub fn list_dir(dir: &str, filter: &str, kind: Kind) -> Vec<String> {
    let mut out = scan_dir(dir, filter, kind, false);
    out.sort_by_cached_key(|s| s.to_lowercase());
    out
}

/// AutoIt `FileExists`: accepts wildcards in the last component.
pub fn exists(p: &str) -> bool {
    if p.is_empty() {
        return false;
    }
    if has_wildcard(p) {
        let (par, name) = split_parent(p);
        return !scan_dir(&par, &name, Kind::Both, true).is_empty();
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
/// A failure on one entry does not stop the others: every failure is added to `errors`. Symbolic links and
/// junctions are not followed (that could leave the tree being copied) and are reported as not copied.
pub fn copy_dir(src: &Path, dst: &Path, overwrite: bool, errors: &mut Vec<String>) {
    if let Err(e) = fs::create_dir_all(dst) {
        errors.push(format!("{}: {}", dst.display(), e));
        return;
    }
    let rd = match fs::read_dir(src) {
        Ok(rd) => rd,
        Err(e) => {
            errors.push(format!("{}: {}", src.display(), e));
            return;
        }
    };
    for entry in rd {
        let e = match entry {
            Ok(e) => e,
            Err(err) => {
                errors.push(format!("{}: {}", src.display(), err));
                continue;
            }
        };
        let from = e.path();
        let to = dst.join(e.file_name());
        let ft = match e.file_type() {
            Ok(t) => t,
            Err(err) => {
                errors.push(format!("{}: {}", from.display(), err));
                continue;
            }
        };
        if ft.is_symlink() {
            errors.push(format!("{}: symbolic link or junction not copied", from.display()));
        } else if ft.is_dir() {
            copy_dir(&from, &to, overwrite, errors);
        } else if overwrite || !to.exists() {
            if to.exists() {
                clear_readonly(&to.to_string_lossy());
            }
            if let Err(err) = fs::copy(&from, &to) {
                errors.push(format!("{}: {}", from.display(), err));
            }
        }
    }
}

/// Moves a directory (rename, or copy+delete across volumes). The source is deleted only when everything was
/// copied.
pub fn move_dir(src: &str, dst: &str) -> io::Result<()> {
    if let Some(par) = Path::new(dst).parent() {
        let _ = fs::create_dir_all(par);
    }
    if fs::rename(src, dst).is_ok() {
        return Ok(());
    }
    let mut errors = Vec::new();
    copy_dir(Path::new(src), Path::new(dst), true, &mut errors);
    if !errors.is_empty() {
        return Err(io::Error::other(errors.join("; ")));
    }
    if remove_dir_recursive(src) {
        Ok(())
    } else {
        Err(io::Error::other("cannot remove source dir"))
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

/// `FileMove(…, 8|1)`. With `overwrite` the destination is replaced in one step by the system (it is never
/// deleted first, so a failure leaves the old file in place); across volumes the system copies and deletes.
pub fn move_file(src: &str, dst: &str, overwrite: bool) -> io::Result<()> {
    if !overwrite && Path::new(dst).exists() {
        return Ok(());
    }
    if let Some(par) = Path::new(dst).parent() {
        let _ = fs::create_dir_all(par);
    }
    if Path::new(dst).exists() {
        clear_readonly(dst);
    }
    let flags = MOVEFILE_COPY_ALLOWED | if overwrite { MOVEFILE_REPLACE_EXISTING } else { 0 };
    if unsafe { MoveFileExW(w(src).as_ptr(), w(dst).as_ptr(), flags) } != 0 {
        return Ok(());
    }
    let first = io::Error::last_os_error();
    // A read-only source cannot be deleted by the system after the copy: copy it and clear the attribute.
    match fs::copy(src, dst) {
        Ok(_) => {
            clear_readonly(src);
            fs::remove_file(src)
        }
        Err(_) => Err(first),
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

// ------------------------------------------------- System32 / WOW64 rewriting

/// Subfolders of `System32` that WOW64 never redirects (Microsoft Learn, "File System
/// Redirector"). A path inside them means the same to every process, so it is kept.
const WOW64_NOT_REDIRECTED: &[&str] = &["catroot", "catroot2", "driverstore", "drivers\\etc", "logfiles", "spool"];

/// Makes a path under `<windir>\System32` mean what it means to the main application, even
/// when gaP has the other architecture (64-bit Windows only):
/// gaP x86 + application x64 -> `Sysnative`; gaP x64 + application x86 -> `SysWOW64`.
/// Any other combination, and any other path, is returned unchanged.
pub fn rewrite_system32(p: &str, windir: &str, os: &str, gap: &str, app: &str) -> String {
    if os != "x64" || gap == app {
        return p.to_string();
    }
    let windir = windir.trim_end_matches('\\');
    let prefix = format!("{}\\System32", windir);
    let Some(head) = p.get(..prefix.len()) else { return p.to_string() };
    if !head.eq_ignore_ascii_case(&prefix) {
        return p.to_string();
    }
    let rest = &p[prefix.len()..];
    if !(rest.is_empty() || rest.starts_with('\\')) {
        return p.to_string();
    }
    let sub = rest.trim_start_matches('\\').to_lowercase();
    if WOW64_NOT_REDIRECTED.iter().any(|d| sub == *d || sub.starts_with(&format!("{}\\", d))) {
        return p.to_string();
    }
    let alias = if app == "x64" { "Sysnative" } else { "SysWOW64" };
    format!("{}\\{}{}", windir, alias, rest)
}

#[cfg(test)]
mod tests {
    use super::rewrite_system32 as rw;
    use super::*;

    #[test]
    fn path_full_resolves_against_the_base() {
        assert_eq!(path_full("App\\a.exe", "C:\\Base"), "C:\\Base\\App\\a.exe");
        assert_eq!(path_full("..\\y", "C:\\Base\\Sub"), "C:\\Base\\y");
        assert_eq!(path_full(".\\x\\.\\y", "C:\\Base"), "C:\\Base\\x\\y");
        // rooted: only the drive of the base is kept
        assert_eq!(path_full("\\Dir", "D:\\Base"), "D:\\Dir");
        // absolute, forward slashes and the trailing slash
        assert_eq!(path_full("C:/a/./b/", "X:\\"), "C:\\a\\b\\");
        assert_eq!(path_full("a\\", "C:\\B"), "C:\\B\\a\\");
        // a drive root keeps its slash, and ".." cannot climb above it
        assert_eq!(path_full("C:\\", "D:\\"), "C:\\");
        assert_eq!(path_full("C:\\..\\..\\x", "D:\\"), "C:\\x");
        // an empty path is the base
        assert_eq!(path_full("", "C:\\Base"), "C:\\Base");
        // UNC
        assert_eq!(path_full("\\\\srv\\share\\a\\..\\b", "C:\\"), "\\\\srv\\share\\b");
        assert_eq!(path_full("\\\\srv\\share", "C:\\"), "\\\\srv\\share\\");
    }

    #[test]
    fn path_pieces() {
        assert_eq!(split_parent("C:\\a\\b"), ("C:\\a".to_string(), "b".to_string()));
        assert_eq!(split_parent("C:\\a\\b\\"), ("C:\\a".to_string(), "b".to_string()));
        assert_eq!(parent_dir("C:\\a"), "C:\\");
        assert_eq!(file_name("C:\\a\\b.txt"), "b.txt");
        assert_eq!(trim_trailing_sep("C:\\"), "C:\\");
        assert_eq!(trim_trailing_sep("C:\\a\\"), "C:\\a");
        assert!(has_extension("a.txt") && !has_extension("a") && !has_extension(".gitignore") && !has_extension("a."));
    }

    #[test]
    fn wildcards() {
        assert!(wildcard_match("*", "anything"));
        assert!(wildcard_match("*.TXT", "notes.txt"));
        assert!(wildcard_match("a?c", "ABC"));
        assert!(wildcard_match("config.ini_Backup_*", "config.ini_Backup_2024"));
        assert!(wildcard_match("a*b*c", "aXXbYYc"));
        assert!(!wildcard_match("a?c", "ac"));
        assert!(!wildcard_match("*.txt", "notes.txt.bak"));
        assert!(!wildcard_match("abc", "abcd"));
        assert!(has_wildcard("a*") && has_wildcard("a?") && !has_wildcard("a"));
    }

    #[test]
    fn numbers_and_segments() {
        assert_eq!(auto_num("  -2 "), -2);
        assert_eq!(auto_num("+5x"), 5);
        assert_eq!(auto_num("abc"), 0);
        assert_eq!(auto_num(""), 0);
        assert_eq!(split_args("a :: b :: c"), vec!["a", "b", "c"]);
        assert_eq!(split_args("a::b"), vec!["a::b"]);
        let a = vec!["x".to_string()];
        assert_eq!(arg(&a, 0), "x");
        assert_eq!(arg(&a, 1), "NULL");
    }

    #[test]
    fn ignoring_the_case() {
        // the AutoIt "=" between strings ignores the case: "null" is "NULL" too
        assert!(is_null("NULL") && is_null("null") && is_null("Null"));
        assert!(!is_null("*NULL*") && !is_null(""));
        assert!(eq_ic("Hello", "hELLO") && !eq_ic("Hello", "Hell"));
        assert!(contains_ic("C:\\Program Files\\App", "program files") && !contains_ic("abc", ""));
        assert!(starts_with_ic("Notepad++.exe", "notepad") && !starts_with_ic("No", "Notepad"));
        assert_eq!(replace_all_ic("%Desktop%\\a\\%DESKTOP%", "%desktop%", "D:"), "D:\\a\\D:");
        assert_eq!(replace_all_ic("abc", "x", "y"), "abc");
        assert_eq!(replace_all_ic("abc", "", "y"), "abc");
        // not only ASCII
        assert!(eq_ic("Ação", "AÇÃO") && contains_ic("Pasta Ação", "ação"));
    }

    #[test]
    fn encodings_roundtrip() {
        let text = "Olá, ação \u{1F600}";
        for enc in [Enc::Utf16Le, Enc::Utf16Be, Enc::Utf8Bom, Enc::Utf8] {
            let (back, found) = decode_bytes(&encode_string(text, enc));
            assert_eq!((back.as_str(), found), (text, enc));
        }
        // plain ASCII is read as ANSI
        assert_eq!(decode_bytes(b"abc"), ("abc".to_string(), Enc::Ansi));
    }

    #[test]
    fn lines_keep_their_terminators() {
        let l = split_lines_keep("a\r\nb\nc\rd");
        assert_eq!(
            l,
            vec![
                ("a".to_string(), "\r\n".to_string()),
                ("b".to_string(), "\n".to_string()),
                ("c".to_string(), "\r".to_string()),
                ("d".to_string(), String::new()),
            ]
        );
    }

    /// A folder of its own in the temporary folder, removed at the end.
    struct Scratch(std::path::PathBuf);
    impl Scratch {
        fn new(name: &str) -> Scratch {
            let p = std::env::temp_dir().join(format!("gap_test_{}_{}", name, std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Scratch(p)
        }
        fn path(&self, rel: &str) -> String {
            self.0.join(rel).to_string_lossy().to_string()
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn copy_dir_merges_and_keeps_going() {
        let t = Scratch::new("copy");
        std::fs::create_dir_all(t.path("src\\sub")).unwrap();
        std::fs::write(t.path("src\\a.txt"), "new").unwrap();
        std::fs::write(t.path("src\\sub\\b.txt"), "b").unwrap();
        std::fs::create_dir_all(t.path("dst")).unwrap();
        std::fs::write(t.path("dst\\a.txt"), "old").unwrap();
        std::fs::write(t.path("dst\\keep.txt"), "keep").unwrap();
        let mut errors = Vec::new();
        // without overwrite the existing file stays
        copy_dir(Path::new(&t.path("src")), Path::new(&t.path("dst")), false, &mut errors);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(std::fs::read_to_string(t.path("dst\\a.txt")).unwrap(), "old");
        assert_eq!(std::fs::read_to_string(t.path("dst\\sub\\b.txt")).unwrap(), "b");
        // with overwrite it is replaced, and what was only in the target is kept
        copy_dir(Path::new(&t.path("src")), Path::new(&t.path("dst")), true, &mut errors);
        assert_eq!(std::fs::read_to_string(t.path("dst\\a.txt")).unwrap(), "new");
        assert_eq!(std::fs::read_to_string(t.path("dst\\keep.txt")).unwrap(), "keep");
        // a source that does not exist is reported, not a panic
        copy_dir(Path::new(&t.path("nope")), Path::new(&t.path("dst2")), true, &mut errors);
        assert_eq!(errors.len(), 1);
    }

    #[test]
    fn move_file_replaces_in_one_step() {
        let t = Scratch::new("move");
        std::fs::write(t.path("a.txt"), "a").unwrap();
        std::fs::write(t.path("b.txt"), "b").unwrap();
        // no overwrite: nothing happens
        move_file(&t.path("a.txt"), &t.path("b.txt"), false).unwrap();
        assert_eq!(std::fs::read_to_string(t.path("b.txt")).unwrap(), "b");
        assert!(is_file(&t.path("a.txt")));
        // overwrite: the target is replaced and the source is gone
        move_file(&t.path("a.txt"), &t.path("b.txt"), true).unwrap();
        assert_eq!(std::fs::read_to_string(t.path("b.txt")).unwrap(), "a");
        assert!(!is_file(&t.path("a.txt")));
        // the folders of the target are created
        std::fs::write(t.path("c.txt"), "c").unwrap();
        move_file(&t.path("c.txt"), &t.path("x\\y\\c.txt"), true).unwrap();
        assert!(is_file(&t.path("x\\y\\c.txt")));
    }

    #[test]
    fn listing_and_existence_with_wildcards() {
        let t = Scratch::new("list");
        for f in ["B.log", "a.log", "c.txt"] {
            std::fs::write(t.path(f), "").unwrap();
        }
        std::fs::create_dir_all(t.path("dir.log")).unwrap();
        let names = |v: Vec<String>| v.iter().map(|p| file_name(p)).collect::<Vec<_>>();
        // sorted ignoring the case; kinds are told apart; several patterns separated by ";"
        assert_eq!(names(list_dir(&t.path(""), "*.log", Kind::Files)), ["a.log", "B.log"]);
        assert_eq!(names(list_dir(&t.path(""), "*.log", Kind::Dirs)), ["dir.log"]);
        assert_eq!(names(list_dir(&t.path(""), "*.txt;a*", Kind::Files)), ["a.log", "c.txt"]);
        assert!(exists(&t.path("*.TXT")) && !exists(&t.path("*.zip")));
        assert!(exists(&t.path("c.txt")) && !exists(&t.path("nope.txt")) && !exists(""));
        delete_files(&t.path("*.log"));
        assert!(!is_file(&t.path("a.log")) && !is_file(&t.path("B.log")) && is_file(&t.path("c.txt")));
    }

    const W: &str = r"C:\Windows";

    #[test]
    fn system32_follows_the_application() {
        // gaP x86 + application x64: the real System32 is reached through Sysnative
        assert_eq!(rw(r"C:\Windows\System32\notepad.exe", W, "x64", "x86", "x64"), r"C:\Windows\Sysnative\notepad.exe");
        assert_eq!(rw(r"c:\windows\system32", W, "x64", "x86", "x64"), r"C:\Windows\Sysnative");
        // gaP x64 + application x86: SysWOW64
        assert_eq!(rw(r"C:\Windows\System32\cmd.exe", W, "x64", "x64", "x86"), r"C:\Windows\SysWOW64\cmd.exe");
        // same architecture, or 32-bit Windows: nothing to do
        assert_eq!(rw(r"C:\Windows\System32\cmd.exe", W, "x64", "x64", "x64"), r"C:\Windows\System32\cmd.exe");
        assert_eq!(rw(r"C:\Windows\System32\cmd.exe", W, "x64", "x86", "x86"), r"C:\Windows\System32\cmd.exe");
        assert_eq!(rw(r"C:\Windows\System32\cmd.exe", W, "x86", "x86", "x86"), r"C:\Windows\System32\cmd.exe");
    }

    #[test]
    fn system32_rewrite_keeps_other_paths() {
        assert_eq!(rw(r"C:\Windows\System32x\a.dll", W, "x64", "x64", "x86"), r"C:\Windows\System32x\a.dll");
        assert_eq!(rw(r"C:\Program Files\App\System32\a.dll", W, "x64", "x64", "x86"), r"C:\Program Files\App\System32\a.dll");
        assert_eq!(rw(r"D:\Windows\System32\a.dll", W, "x64", "x64", "x86"), r"D:\Windows\System32\a.dll");
        assert_eq!(rw("C:", W, "x64", "x64", "x86"), "C:");
        // folders that WOW64 does not redirect
        assert_eq!(rw(r"C:\Windows\System32\drivers\etc\hosts", W, "x64", "x64", "x86"), r"C:\Windows\System32\drivers\etc\hosts");
        assert_eq!(rw(r"C:\Windows\System32\spool", W, "x64", "x64", "x86"), r"C:\Windows\System32\spool");
        // ...but their neighbours are
        assert_eq!(rw(r"C:\Windows\System32\drivers\x.sys", W, "x64", "x64", "x86"), r"C:\Windows\SysWOW64\drivers\x.sys");
    }
}
