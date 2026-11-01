//! Keywords for files and directories:
//! dir, file, (dir), (file), ->dir, <-dir, ->file, <-file, +dir, +file,
//! -dir, -file, attributes, batch, date, keeponly, replace, replaceline,
//! regmerge.

use std::ptr::{null, null_mut};
use std::time::SystemTime;

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Storage::FileSystem::*;
use windows_sys::Win32::System::Time::*;
use windows_sys::Win32::System::IO::DeviceIoControl;

use crate::ctx::{Ctx, Prog, Section};
use crate::{sys, util};
use util::{eq_ic, Enc, Kind};

const NULL: &str = "NULL";

fn is_null(s: &str) -> bool {
    s == NULL
}

// ---------------------------------------------------------- protections

fn norm_cmp(p: &str) -> String {
    util::trim_trailing_sep(p).to_lowercase()
}

fn is_inside(child: &str, parent: &str) -> bool {
    let (c, p) = (norm_cmp(child), norm_cmp(parent));
    c == p || c.starts_with(&format!("{}\\", p))
}

/// Drive root or ancestor of the launcher directory: removal blocked.
fn dangerous_delete(ctx: &Ctx, path: &str) -> bool {
    let p = util::trim_trailing_sep(path);
    let is_root = p.len() <= 3 && p.ends_with([':', '\\']);
    is_root || is_inside(&ctx.script_dir, &p)
}

// --------------------------------------------------------------- copy / move

/// Core of `->dir`/`<-dir` with already resolved paths.
pub fn transfer_dir(ctx: &Ctx, src: &str, tgt: &str, mv: bool, ow: bool) {
    if eq_ic(&norm_cmp(src), &norm_cmp(tgt)) || !util::is_dir(src) {
        return;
    }
    if is_inside(tgt, src) || (mv && is_inside(src, tgt)) {
        ctx.msg(&format!("Directory operation skipped: \"{}\" and \"{}\" are nested.", src, tgt));
        return;
    }
    let (s, t) = (std::path::Path::new(src), std::path::Path::new(tgt));
    match (mv, ow) {
        (false, ow) => {
            let _ = util::copy_dir(s, t, ow);
        }
        (true, true) => {
            if util::exists(tgt) {
                util::remove_dir_recursive(tgt);
            }
            let _ = util::move_dir(src, tgt);
        }
        (true, false) => {
            if !util::exists(tgt) {
                let _ = util::move_dir(src, tgt);
            }
        }
    }
}

/// Core of `->file`/`<-file` with resolved FILE paths.
pub fn transfer_file(src: &str, tgt: &str, mv: bool, ow: bool) {
    if eq_ic(src, tgt) || !util::is_file(src) {
        return;
    }
    let _ = if mv { util::move_file(src, tgt, ow) } else { util::copy_file(src, tgt, ow) };
}

pub fn run_prog_fs(ctx: &Ctx, p: &Prog) {
    match p {
        Prog::LtrDir { src, tgt, mv, ow } => transfer_dir(ctx, src, tgt, *mv, *ow),
        Prog::LtrFile { src, tgt, mv, ow } => transfer_file(src, tgt, *mv, *ow),
        _ => {}
    }
}

fn parse_transfer_flags(flag: &str) -> (bool, bool) {
    let f = if is_null(flag) || flag.is_empty() { "copy, overwrite" } else { flag };
    let mv = util::contains_ic(f, "move");
    let ow = !util::contains_ic(f, "no overwrite");
    (mv, ow)
}

fn arg(a: &[String], i: usize) -> &str {
    a.get(i).map(|s| s.as_str()).unwrap_or(NULL)
}

fn full(ctx: &mut Ctx, p: &str) -> String {
    let e = ctx.expand(p);
    util::path_full(&e, &ctx.script_dir)
}

fn default_settings_dir(ctx: &Ctx) -> String {
    format!("{}\\Settings", ctx.script_dir.trim_end_matches('\\'))
}

/// `->dir` / `<-dir`. `rtl` swaps source and destination.
pub fn ltr_dir(ctx: &mut Ctx, args: &[String], rtl: bool) {
    let (mut a, mut b) = (arg(args, 0).to_string(), arg(args, 1).to_string());
    if rtl {
        std::mem::swap(&mut a, &mut b);
    }
    let src = if is_null(&a) { default_settings_dir(ctx) } else { full(ctx, &a) };
    let tgt = if is_null(&b) { default_settings_dir(ctx) } else { full(ctx, &b) };
    let flag = ctx.expand(arg(args, 2));
    let (mv, ow) = parse_transfer_flags(&flag);
    transfer_dir(ctx, &util::trim_trailing_sep(&src), &util::trim_trailing_sep(&tgt), mv, ow);
}

/// Finds the destination file when the user gave a directory or nothing.
fn resolve_file_target(ctx: &Ctx, src: &str, tgt: &str) -> String {
    let name = util::file_name(src);
    if tgt.ends_with('\\') || util::is_dir(tgt) || (!util::exists(tgt) && !util::has_extension(&util::file_name(tgt))) {
        format!("{}\\{}", tgt.trim_end_matches('\\'), name)
    } else {
        let _ = ctx;
        tgt.to_string()
    }
}

/// `->file` / `<-file`.
pub fn ltr_file(ctx: &mut Ctx, args: &[String], rtl: bool) {
    let (mut a, mut b) = (arg(args, 0).to_string(), arg(args, 1).to_string());
    if rtl {
        std::mem::swap(&mut a, &mut b);
    }
    // source NULL + destination with extension => source = {GAPROOT}\<destination name>
    if is_null(&a) && !is_null(&b) {
        let tmp = ctx.expand(&b);
        let n = util::file_name(&tmp);
        if util::has_extension(&n) {
            a = format!("{{GAPROOT}}\\{}", n);
        }
    }
    if is_null(&a) {
        return;
    }
    let src = full(ctx, &a);
    let tgt = if is_null(&b) { ctx.script_dir.clone() } else { full(ctx, &b) };
    let flag = ctx.expand(arg(args, 2));
    let (mv, ow) = parse_transfer_flags(&flag);
    let tgt = resolve_file_target(ctx, &src, &tgt);
    transfer_file(&src, &tgt, mv, ow);
}

/// `dir = Source [:: Destination]` (BEFORE only).
pub fn dir_kw(ctx: &mut Ctx, args: &[String]) {
    let src = util::trim_trailing_sep(&full(ctx, arg(args, 0)));
    let tgt = if is_null(arg(args, 1)) { default_settings_dir(ctx) } else { full(ctx, arg(args, 1)) };
    let tgt = util::trim_trailing_sep(&tgt);
    let backup = format!("{}_backup", src);
    transfer_dir(ctx, &src, &backup, true, true);
    transfer_dir(ctx, &tgt, &src, false, true);
    ctx.after_prog.push(Prog::LtrDir { src: src.clone(), tgt, mv: true, ow: true });
    ctx.after_prog.push(Prog::LtrDir { src: backup, tgt: src, mv: true, ow: true });
}

/// `file = Source [:: Destination]` (BEFORE only).
pub fn file_kw(ctx: &mut Ctx, args: &[String]) {
    let src = full(ctx, arg(args, 0));
    let settings = if is_null(arg(args, 1)) {
        format!("{}\\{}", ctx.script_dir.trim_end_matches('\\'), util::file_name(&src))
    } else {
        let t = full(ctx, arg(args, 1));
        resolve_file_target(ctx, &src, &t)
    };
    let backup = format!("{}_backup", src);
    transfer_file(&src, &backup, true, true);
    transfer_file(&settings, &src, false, true);
    ctx.after_prog.push(Prog::LtrFile { src: src.clone(), tgt: settings, mv: true, ow: true });
    ctx.after_prog.push(Prog::LtrFile { src: backup, tgt: src, mv: true, ow: true });
}

/// `(dir) = Path`.
pub fn pdirp_kw(ctx: &mut Ctx, args: &[String]) {
    let src = util::trim_trailing_sep(&full(ctx, arg(args, 0)));
    let backup = format!("{}_backup", src);
    transfer_dir(ctx, &src, &backup, true, true);
    ctx.after_prog.push(Prog::LtrDir { src: backup, tgt: src, mv: true, ow: true });
}

/// `(file) = Path`.
pub fn pfilep_kw(ctx: &mut Ctx, args: &[String]) {
    let src = full(ctx, arg(args, 0));
    let backup = format!("{}_backup", src);
    transfer_file(&src, &backup, true, true);
    ctx.after_prog.push(Prog::LtrFile { src: backup, tgt: src, mv: true, ow: true });
}

// ----------------------------------------------------------- create / delete

pub fn plus_dir(ctx: &mut Ctx, args: &[String]) {
    let d = util::trim_trailing_sep(&full(ctx, arg(args, 0)));
    if !util::exists(&d) {
        let _ = std::fs::create_dir_all(&d);
    }
}

pub fn plus_file(ctx: &mut Ctx, args: &[String]) {
    let f = full(ctx, arg(args, 0));
    let mut flag = ctx.expand(arg(args, 1));
    if is_null(&flag) || flag.is_empty() {
        flag = "overwrite".into();
    }
    let mut text = ctx.expand(arg(args, 2));
    if is_null(&text) {
        text.clear();
    }
    let text = text.replace("*NULL*", "NULL");
    let exists = util::exists(&f);
    let go = eq_ic(&flag, "overwrite") || (eq_ic(&flag, "no overwrite") && !exists);
    if go {
        if exists {
            let _ = std::fs::remove_file(&f); // recreates (mode "erase")
        }
        let enc = if util::ansi_to_string(&util::string_to_ansi(&text)) == text { Enc::Ansi } else { Enc::Utf8 };
        let _ = util::write_text(&f, &text, enc);
    }
}

pub fn minus_dir(ctx: &mut Ctx, args: &[String]) {
    let d = util::trim_trailing_sep(&full(ctx, arg(args, 0)));
    let flag = ctx.expand(arg(args, 1));
    if !util::exists(&d) {
        return;
    }
    let (par, name) = util::split_parent(&d);
    let par = if par.len() == 2 { format!("{}\\", par) } else { par };
    for m in util::list_dir(&par, &name, Kind::Dirs) {
        if dangerous_delete(ctx, &m) {
            ctx.msg(&format!("Refusing to delete \"{}\" (drive root or a parent of the launcher directory).", m));
            continue;
        }
        if eq_ic(&flag, "ifempty") {
            util::remove_dir_if_empty(&m);
        } else {
            util::remove_dir_recursive(&m);
        }
    }
}

pub fn minus_file(ctx: &mut Ctx, args: &[String]) {
    let f = full(ctx, arg(args, 0));
    if util::exists(&f) {
        util::delete_files(&f);
    }
}

// --------------------------------------------------------------- attributes

pub fn attributes(ctx: &mut Ctx, args: &[String]) {
    let path = util::trim_trailing_sep(&full(ctx, arg(args, 0)));
    let vals = ctx.expand(arg(args, 1)).replace(' ', "");
    let mut list: Vec<String> = vals.split(',').map(|s| s.to_lowercase()).collect();
    if list.len() == 1 && (list[0] == "null" || list[0].is_empty()) {
        list[0] = "normal".into();
    }
    if !util::exists(&path) {
        return;
    }
    unsafe {
        let wp = util::w(&path);
        let cur = GetFileAttributesW(wp.as_ptr());
        if cur == INVALID_FILE_ATTRIBUTES {
            return;
        }
        let managed = FILE_ATTRIBUTE_ARCHIVE
            | FILE_ATTRIBUTE_READONLY
            | FILE_ATTRIBUTE_SYSTEM
            | FILE_ATTRIBUTE_HIDDEN
            | FILE_ATTRIBUTE_OFFLINE
            | FILE_ATTRIBUTE_TEMPORARY;
        // "-RASHNOT": starts by clearing everything (also makes the file writable)
        SetFileAttributesW(wp.as_ptr(), cur & !managed);
        let mut bits = 0u32;
        let mut compress = false;
        for v in &list {
            match v.chars().next() {
                Some('c') => compress = true,
                Some('a') => bits |= FILE_ATTRIBUTE_ARCHIVE,
                Some('r') => bits |= FILE_ATTRIBUTE_READONLY,
                Some('s') => bits |= FILE_ATTRIBUTE_SYSTEM,
                Some('h') => bits |= FILE_ATTRIBUTE_HIDDEN,
                Some('o') => bits |= FILE_ATTRIBUTE_OFFLINE,
                Some('t') => bits |= FILE_ATTRIBUTE_TEMPORARY,
                _ => {}
            }
        }
        // NTFS compression (FSCTL_SET_COMPRESSION): always turns off, turns on if requested
        let h = CreateFileW(
            wp.as_ptr(), GENERIC_READ | GENERIC_WRITE, FILE_SHARE_READ | FILE_SHARE_WRITE, null(),
            OPEN_EXISTING, FILE_FLAG_BACKUP_SEMANTICS, null_mut(),
        );
        if h != INVALID_HANDLE_VALUE {
            // COMPRESSION_FORMAT_DEFAULT (1) turns on; COMPRESSION_FORMAT_NONE (0) turns off.
            let state: u16 = if compress { 1 } else { 0 };
            let mut ret = 0u32;
            DeviceIoControl(
                h, 0x0009_C040, &state as *const u16 as *const _, 2, null_mut(), 0, &mut ret, null_mut(),
            );
            CloseHandle(h);
        }
        let mut fin = (cur & !managed) | bits;
        if fin & !FILE_ATTRIBUTE_DIRECTORY == 0 {
            fin |= FILE_ATTRIBUTE_NORMAL;
        } else {
            fin &= !FILE_ATTRIBUTE_NORMAL;
        }
        SetFileAttributesW(wp.as_ptr(), fin);
    }
}

// ------------------------------------------------------------------- batch

pub fn batch(ctx: &mut Ctx, args: &[String]) {
    use std::os::windows::process::CommandExt;
    let f = full(ctx, arg(args, 0));
    let mut show = ctx.expand(arg(args, 1));
    if is_null(&show) || show.is_empty() {
        show = "hide".into();
    }
    if !util::exists(&f) {
        return;
    }
    let wd = util::parent_dir(&f);
    let cmd = sys::system32("cmd.exe");
    let mut c = std::process::Command::new(cmd);
    c.current_dir(&wd);
    if eq_ic(&show, "hide") {
        c.raw_arg(format!("/C \"{}\"", f)).creation_flags(0x0800_0000);
    } else if eq_ic(&show, "show") {
        c.raw_arg(format!("/K \"{}\"", f));
    } else {
        return;
    }
    let _ = c.status();
}

// -------------------------------------------------------------------- dates

fn filetime_from_systemtime(t: SystemTime) -> u64 {
    let d = t.duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0);
    d / 100 + 116_444_736_000_000_000
}

/// "YYYYMMDDHHMMSS" in local time (format of `FileGetTime(..., 1)`).
pub fn local_stamp(t: SystemTime) -> String {
    unsafe {
        let ft64 = filetime_from_systemtime(t);
        let ft = FILETIME { dwLowDateTime: ft64 as u32, dwHighDateTime: (ft64 >> 32) as u32 };
        let (mut utc, mut loc): (SYSTEMTIME, SYSTEMTIME) = std::mem::zeroed();
        FileTimeToSystemTime(&ft, &mut utc);
        SystemTimeToTzSpecificLocalTime(null(), &utc, &mut loc);
        format!(
            "{:04}{:02}{:02}{:02}{:02}{:02}",
            loc.wYear, loc.wMonth, loc.wDay, loc.wHour, loc.wMinute, loc.wSecond
        )
    }
}

fn parse_stamp(digits: &str) -> Option<SYSTEMTIME> {
    if digits.len() < 4 || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let part = |a: usize, b: usize, d: u16| -> u16 {
        digits.get(a..b).and_then(|s| s.parse().ok()).unwrap_or(d)
    };
    Some(SYSTEMTIME {
        wYear: part(0, 4, 1970),
        wMonth: part(4, 6, 1),
        wDay: part(6, 8, 1),
        wHour: part(8, 10, 0),
        wMinute: part(10, 12, 0),
        wSecond: part(12, 14, 0),
        wMilliseconds: 0,
        wDayOfWeek: 0,
    })
}

pub fn date(ctx: &mut Ctx, args: &[String]) {
    let f = full(ctx, arg(args, 0));
    let raw = ctx.expand(arg(args, 1));
    let mut d: String = raw.chars().filter(|c| !matches!(c, '.' | '/' | ' ' | ':')).collect();
    if is_null(&d) || eq_ic(&d, "now") || d.is_empty() {
        d = local_stamp(SystemTime::now());
    }
    let mut kind = ctx.expand(arg(args, 2));
    if is_null(&kind) || kind.is_empty() {
        kind = "modified".into();
    }
    if !util::exists(&f) {
        return;
    }
    let Some(local) = parse_stamp(&d) else { return };
    unsafe {
        let (mut utc, mut ft): (SYSTEMTIME, FILETIME) = std::mem::zeroed();
        if TzSpecificLocalTimeToSystemTime(null(), &local, &mut utc) == 0 {
            return;
        }
        if SystemTimeToFileTime(&utc, &mut ft) == 0 {
            return;
        }
        let wp = util::w(&f);
        let h = CreateFileW(
            wp.as_ptr(), FILE_WRITE_ATTRIBUTES, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, null(),
            OPEN_EXISTING, FILE_FLAG_BACKUP_SEMANTICS, null_mut(),
        );
        if h == INVALID_HANDLE_VALUE {
            return;
        }
        let (c, a, m): (*const FILETIME, *const FILETIME, *const FILETIME) = match kind.to_lowercase().as_str() {
            "created" => (&ft, null(), null()),
            "accessed" => (null(), &ft, null()),
            _ => (null(), null(), &ft),
        };
        SetFileTime(h, c, a, m);
        CloseHandle(h);
    }
}

// ----------------------------------------------------------------- keeponly

pub fn keeponly(ctx: &mut Ctx, args: &[String]) {
    let path = util::trim_trailing_sep(&full(ctx, arg(args, 0)));
    let filter = ctx.expand(arg(args, 1));
    let typ = ctx.expand(arg(args, 2));
    let order = ctx.expand(arg(args, 3));
    let param = ctx.expand(arg(args, 4)).to_lowercase();
    let number = ctx.expand_num(arg(args, 5)).max(0) as usize;
    if !util::is_dir(&path) {
        return;
    }
    let kind = if eq_ic(&typ, "file") {
        Kind::Files
    } else if eq_ic(&typ, "folder") {
        Kind::Dirs
    } else {
        ctx.msg("keeponly: the third segment must be \"file\" or \"folder\".");
        return;
    };
    let ascending = eq_ic(&order, "ascending");
    if !ascending && !eq_ic(&order, "descending") {
        ctx.msg("keeponly: the fourth segment must be \"ascending\" or \"descending\".");
        return;
    }
    if !["modified", "created", "accessed", "highest", "lowest", "name"].contains(&param.as_str()) {
        ctx.msg("keeponly: invalid sort criteria.");
        return;
    }
    let items = util::list_dir(&path, if filter.is_empty() || is_null(&filter) { "*" } else { &filter }, kind);
    if items.len() <= number {
        return;
    }
    let mut keyed: Vec<(String, String)> = items
        .into_iter()
        .map(|p| {
            let md = std::fs::metadata(&p).ok();
            let st = |f: fn(&std::fs::Metadata) -> std::io::Result<SystemTime>| -> String {
                md.as_ref().and_then(|m| f(m).ok()).map(local_stamp).unwrap_or_else(|| "0".repeat(14))
            };
            let key = match param.as_str() {
                "modified" => st(|m| m.modified()),
                "created" => st(|m| m.created()),
                "accessed" => st(|m| m.accessed()),
                "highest" => [st(|m| m.modified()), st(|m| m.created()), st(|m| m.accessed())].into_iter().max().unwrap(),
                "lowest" => [st(|m| m.modified()), st(|m| m.created()), st(|m| m.accessed())].into_iter().min().unwrap(),
                _ => util::file_name(&p).to_lowercase(),
            };
            (key, p)
        })
        .collect();
    keyed.sort_by(|a, b| a.0.cmp(&b.0));
    if !ascending {
        keyed.reverse();
    }
    for (_, p) in keyed.into_iter().skip(number) {
        match kind {
            Kind::Files => util::delete_files(&p),
            _ => {
                util::remove_dir_recursive(&p);
            }
        }
    }
}

// ---------------------------------------------------------- text in files

pub fn replace(ctx: &mut Ctx, args: &[String]) {
    let f = full(ctx, arg(args, 0));
    let find = ctx.expand(arg(args, 1));
    let rep = ctx.expand(arg(args, 2));
    if !util::is_file(&f) {
        return;
    }
    if let Some((text, enc)) = util::read_text(&f) {
        let new = util::replace_all_ic(&text, &find, &rep);
        if new != text {
            let _ = util::write_text(&f, &new, enc);
        }
    }
}

pub fn replaceline(ctx: &mut Ctx, args: &[String]) {
    let f = full(ctx, arg(args, 0));
    let find = ctx.expand(arg(args, 1));
    let rep = ctx.expand(arg(args, 2));
    if !util::is_file(&f) || find.is_empty() {
        return;
    }
    if let Some((text, enc)) = util::read_text(&f) {
        let mut lines = util::split_lines_keep(&text);
        if let Some(i) = lines.iter().position(|(l, _)| util::contains_ic(l, &find)) {
            lines[i].0 = rep;
            let out: String = lines.iter().map(|(l, e)| format!("{}{}", l, e)).collect();
            let _ = util::write_text(&f, &out, enc);
        }
    }
}

/// `regmerge`: appends the 2nd .reg to the 1st, removing the duplicate header.
pub fn regmerge(ctx: &mut Ctx, args: &[String]) {
    let f1 = full(ctx, arg(args, 0));
    let f2 = full(ctx, arg(args, 1));
    if !util::is_file(&f1) || !util::is_file(&f2) {
        return;
    }
    let (Some((t1, e1)), Some((t2, _))) = (util::read_text(&f1), util::read_text(&f2)) else { return };
    let is_header = |l: &str| {
        let t = l.trim();
        t.eq_ignore_ascii_case("REGEDIT4") || t.eq_ignore_ascii_case("Windows Registry Editor Version 5.00")
    };
    let mut body = util::split_lines_keep(&t2);
    if let Some(i) = body.iter().position(|(l, _)| is_header(l)) {
        body.remove(i);
    }
    let mut out = t1;
    if !out.ends_with('\n') && !out.is_empty() {
        out.push_str("\r\n");
    }
    for (l, e) in body {
        out.push_str(&l);
        out.push_str(if e.is_empty() { "\r\n" } else { &e });
    }
    let _ = util::write_text(&f1, &out, e1);
}

#[allow(dead_code)]
pub fn section_unused(_s: Section) {}
