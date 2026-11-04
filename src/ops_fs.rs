//! Keywords for files and directories:
//! dir, file, (dir), (file), ->dir, <-dir, ->file, <-file, +dir, +file,
//! -dir, -file, attributes, batch, date, keeponly, replace, replaceline,
//! regmerge.

use std::ptr::{null, null_mut};
use std::time::SystemTime;

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Storage::FileSystem::*;
use windows_sys::Win32::System::Time::*;
use windows_sys::Win32::System::Ioctl::FSCTL_SET_COMPRESSION;
use windows_sys::Win32::System::IO::DeviceIoControl;

use crate::ctx::{Ctx, Prog};
use crate::{sys, util};
use util::{arg, eq_ic, is_null, Enc, Kind};

// ---------------------------------------------------------- protections

fn norm_cmp(p: &str) -> String {
    util::trim_trailing_sep(p).to_lowercase()
}

fn is_inside(child: &str, parent: &str) -> bool {
    let (c, p) = (norm_cmp(child), norm_cmp(parent));
    c == p || c.starts_with(&format!("{}\\", p))
}

/// The launcher itself, its configuration file, a drive root or an ancestor of the launcher directory: removal
/// blocked.
fn dangerous_delete(ctx: &Ctx, path: &str) -> bool {
    let p = util::trim_trailing_sep(path);
    let is_root = p.len() <= 3 && p.ends_with([':', '\\']);
    let np = norm_cmp(&p);
    is_root || is_inside(&ctx.script_dir, &p) || np == norm_cmp(&ctx.ini_path) || np == norm_cmp(&ctx.script_full)
}

// --------------------------------------------------------------- copy / move

/// A move that replaces a folder puts the old one aside first: `.gap_old` while the move is not confirmed (it may
/// hold the only copy of the data), `.gap_del` once it is (it only waits to be deleted).
const ASIDE: &str = ".gap_old";
const TRASH: &str = ".gap_del";

/// Something is at `p` (a link that points nowhere counts).
fn occupied(p: &str) -> bool {
    std::fs::symlink_metadata(p).is_ok()
}

/// `base`, or `base2`, `base3`... the first name that is free.
fn free_name(base: &str) -> String {
    let mut name = base.to_string();
    let mut n = 1;
    while occupied(&name) {
        n += 1;
        name = format!("{}{}", base, n);
    }
    name
}

/// `entry` is a name that `free_name` makes for the trash of `name`: `<name>.gap_del`, or that with a number after it.
/// The wildcard that lists the candidates also matches `<name>.gap_del_old` and the like, and those are not ours.
fn is_trash_name(name: &str, entry: &str) -> bool {
    let base = format!("{}{}", name, TRASH).to_lowercase();
    entry.to_lowercase().strip_prefix(&base).is_some_and(|rest| rest.bytes().all(|b| b.is_ascii_digit()))
}

/// Finishes what an earlier replacing move of `tgt` left when it was interrupted: the garbage of a replacement
/// that was confirmed is deleted, and the old folder is put back if the target is gone (the interruption came
/// before the new content arrived). An old folder next to a target that exists is left alone: nothing says
/// whether the target is complete.
fn recover_replaced(ctx: &Ctx, tgt: &str) {
    let (par, name) = util::split_parent(tgt);
    let par = if par.len() == 2 { format!("{}\\", par) } else { par };
    for old in util::list_dir(&par, &format!("{}{}*", name, TRASH), Kind::Dirs) {
        if is_trash_name(&name, &util::file_name(&old)) && !util::remove_dir_recursive(&old) {
            ctx.err(&format!("delete leftover folder \"{}\"", old));
        }
    }
    let aside = format!("{}{}", tgt, ASIDE);
    if occupied(&aside) && !occupied(tgt) {
        match std::fs::rename(&aside, tgt) {
            Ok(()) => ctx.log(&format!("\"{}\" put back from the folder an interrupted replacement left", tgt)),
            Err(e) => ctx.log_err(&format!("put \"{}\" back from \"{}\"", tgt, aside), &e),
        }
    }
}

/// Core of `->dir`/`<-dir` with already resolved paths. What fails is written to the debug log. `false` = the
/// operation failed; nothing to do (no source) is not a failure.
pub fn transfer_dir(ctx: &Ctx, src: &str, tgt: &str, mv: bool, ow: bool) -> bool {
    let what = format!("dir {} \"{}\" -> \"{}\"{}", if mv { "move" } else { "copy" }, src, tgt, if ow { " (overwrite)" } else { "" });
    if !util::is_dir(src) {
        ctx.log(&format!("{}: the source does not exist, nothing to do", what));
        return true;
    }
    if eq_ic(&norm_cmp(src), &norm_cmp(tgt)) {
        ctx.log(&format!("{}: the source and the destination are the same folder, nothing to do", what));
        return true;
    }
    let ok = transfer_dir_run(ctx, src, tgt, mv, ow);
    if ok {
        ctx.ok(&what);
    } else {
        ctx.err(&format!("{} failed", what));
    }
    ok
}

fn transfer_dir_run(ctx: &Ctx, src: &str, tgt: &str, mv: bool, ow: bool) -> bool {
    if eq_ic(&norm_cmp(src), &norm_cmp(tgt)) || !util::is_dir(src) {
        return true;
    }
    if is_inside(tgt, src) || (mv && is_inside(src, tgt)) {
        ctx.msg(&format!("Directory operation skipped: \"{}\" and \"{}\" are nested.", src, tgt));
        return false;
    }
    let (s, t) = (std::path::Path::new(src), std::path::Path::new(tgt));
    match (mv, ow) {
        (false, ow) => {
            let mut errors = Vec::new();
            util::copy_dir(s, t, ow, &mut errors);
            let ok = errors.is_empty();
            for e in errors {
                ctx.log_err(&format!("copy \"{}\" to \"{}\"", src, tgt), &e);
            }
            ok
        }
        (true, true) => {
            let tgt = util::trim_trailing_sep(tgt);
            recover_replaced(ctx, &tgt);
            if !occupied(&tgt) {
                return match util::move_dir(src, &tgt) {
                    Ok(()) => true,
                    Err(e) => {
                        ctx.log_err(&format!("move \"{}\" to \"{}\"", src, tgt), &e);
                        false
                    }
                };
            }
            // The old target is moved aside, not deleted: if the move fails it is put back, so a failure never
            // costs the data that was there.
            let aside = free_name(&format!("{}{}", tgt, ASIDE));
            if let Err(e) = std::fs::rename(&tgt, &aside) {
                ctx.log_err(&format!("replace \"{}\" (it cannot be moved aside)", tgt), &e);
                return false;
            }
            match util::move_dir(src, &tgt) {
                Ok(()) => {
                    // One rename says "the replacement is done". If the deletion that follows fails or is
                    // interrupted, what stays is marked as garbage and the next replacement of this target
                    // removes it; it is never taken for the old data.
                    let trash = free_name(&format!("{}{}", tgt, TRASH));
                    let old = if std::fs::rename(&aside, &trash).is_ok() { trash } else { aside };
                    if !util::remove_dir_recursive(&old) {
                        ctx.err(&format!("delete the previous content \"{}\"", old));
                    }
                    true
                }
                Err(e) => {
                    ctx.log_err(&format!("move \"{}\" to \"{}\"", src, tgt), &e);
                    util::remove_dir_recursive(&tgt); // what was copied before the failure
                    if let Err(e) = std::fs::rename(&aside, &tgt) {
                        ctx.log_err(&format!("put \"{}\" back", tgt), &e);
                        ctx.msg(&format!(
                            "\"{}\" could not be replaced and could not be put back. Its previous content is in \"{}\".",
                            tgt, aside
                        ));
                    }
                    false
                }
            }
        }
        (true, false) => {
            if util::exists(tgt) {
                return true;
            }
            match util::move_dir(src, tgt) {
                Ok(()) => true,
                Err(e) => {
                    ctx.log_err(&format!("move \"{}\" to \"{}\"", src, tgt), &e);
                    false
                }
            }
        }
    }
}

/// Core of `->file`/`<-file` with resolved FILE paths.
pub fn transfer_file(ctx: &Ctx, src: &str, tgt: &str, mv: bool, ow: bool) -> bool {
    let what = format!("file {} \"{}\" -> \"{}\"{}", if mv { "move" } else { "copy" }, src, tgt, if ow { " (overwrite)" } else { "" });
    if eq_ic(src, tgt) || !util::is_file(src) {
        ctx.log(&format!("{}: nothing to do (no source file, or source and target are the same)", what));
        return true;
    }
    let r = if mv { util::move_file(src, tgt, ow) } else { util::copy_file(src, tgt, ow) };
    if let Err(e) = r {
        ctx.log_err(&format!("{} \"{}\" to \"{}\"", if mv { "move" } else { "copy" }, src, tgt), &e);
        return false;
    }
    ctx.ok(&what);
    true
}

pub fn run_prog_fs(ctx: &Ctx, p: &Prog) {
    match p {
        Prog::LtrDir { src, tgt, mv, ow } => {
            transfer_dir(ctx, src, tgt, *mv, *ow);
        }
        Prog::LtrFile { src, tgt, mv, ow } => {
            transfer_file(ctx, src, tgt, *mv, *ow);
        }
        _ => {}
    }
}

/// The flag of a copy or a move: the words `copy`, `move`, `overwrite` and `no overwrite`, separated by commas,
/// spaces or hyphens (`no-overwrite` is read as `no overwrite`). The default is `copy, overwrite`. A word that is
/// none of them is reported and left out, so a typo does not change what the item does without a word about it.
fn parse_transfer_flags(ctx: &Ctx, flag: &str) -> (bool, bool) {
    let (mut mv, mut ow) = (false, true);
    if is_null(flag) {
        return (mv, ow);
    }
    let lower = flag.to_lowercase();
    let mut words = lower.split(|c: char| c.is_whitespace() || c == ',' || c == '-').filter(|w| !w.is_empty());
    while let Some(w) = words.next() {
        match w {
            "copy" => mv = false,
            "move" => mv = true,
            "overwrite" => ow = true,
            "nooverwrite" => ow = false,
            "no" => match words.next() {
                Some("overwrite") => ow = false,
                _ => ctx.warn(&format!("flag \"{}\": \"no\" has to be followed by \"overwrite\"; it was ignored", flag.trim())),
            },
            other => ctx.warn(&format!(
                "flag \"{}\": \"{}\" is not valid, use copy, move, overwrite or no overwrite; it was ignored",
                flag.trim(),
                other
            )),
        }
    }
    (mv, ow)
}

/// Full path of an argument as gaP has to open it so that it means what it means to the
/// main application (`System32` of the application, see `Ctx::io_path`).
fn full(ctx: &mut Ctx, p: &str) -> String {
    let e = ctx.expand(p);
    let f = util::path_full(&e, &ctx.script_dir);
    ctx.io_path(&f)
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
    let (mv, ow) = parse_transfer_flags(ctx, &flag);
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
    let (mv, ow) = parse_transfer_flags(ctx, &flag);
    let tgt = resolve_file_target(ctx, &src, &tgt);
    transfer_file(ctx, &src, &tgt, mv, ow);
}

/// The move of a file to its `_backup` failed. When the file is still there it was not touched, and what the move
/// left at `backup` (a copy it could not finish, or a complete one whose original it could not remove) is of no use:
/// it is deleted, so that the next run does not stop for a `_backup` that holds nothing.
fn drop_failed_backup(ctx: &Ctx, src: &str, backup: &str) {
    if util::is_file(src) && util::exists(backup) {
        let failed = util::delete_files(backup);
        if failed > 0 {
            ctx.log(&format!("the unfinished backup \"{}\" could not be deleted ({} file(s) kept)", backup, failed));
        } else {
            ctx.log(&format!("the unfinished backup \"{}\" was deleted", backup));
        }
    }
}

/// An earlier run that did not finish left its original data in `backup`: copying over it would lose it. Used when the
/// item runs, for the items that the check at the start could not look at (they ask the user for a folder, a file, a
/// text or a password). The item is skipped and nothing of it is changed.
fn backup_busy(ctx: &Ctx, what: &str, backup: &str) -> bool {
    if !occupied(backup) {
        return false;
    }
    ctx.err(&format!("leftover backup \"{}\"", backup));
    ctx.msg(&format!(
        "An earlier run did not finish and left the original {} in \"{}\". This item was skipped and nothing was changed. Restore or delete it and run again.",
        what, backup
    ));
    true
}

/// An item whose original data could not be saved is skipped: nothing was changed.
fn not_saved(ctx: &Ctx, what: &str, src: &str) {
    ctx.msg(&format!("The original {} \"{}\" could not be saved, so this item was skipped and nothing was changed.", what, src));
}

/// `dir = Source [:: Destination]` (BEFORE only).
pub fn dir_kw(ctx: &mut Ctx, args: &[String]) {
    let src = util::trim_trailing_sep(&full(ctx, arg(args, 0)));
    let tgt = if is_null(arg(args, 1)) { default_settings_dir(ctx) } else { full(ctx, arg(args, 1)) };
    let tgt = util::trim_trailing_sep(&tgt);
    let backup = format!("{}_backup", src);
    if backup_busy(ctx, "folder", &backup) {
        return;
    }
    if !transfer_dir(ctx, &src, &backup, true, true) {
        not_saved(ctx, "folder", &src);
        return;
    }
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
    if backup_busy(ctx, "file", &backup) {
        return;
    }
    if !transfer_file(ctx, &src, &backup, true, true) {
        drop_failed_backup(ctx, &src, &backup);
        not_saved(ctx, "file", &src);
        return;
    }
    transfer_file(ctx, &settings, &src, false, true);
    ctx.after_prog.push(Prog::LtrFile { src: src.clone(), tgt: settings, mv: true, ow: true });
    ctx.after_prog.push(Prog::LtrFile { src: backup, tgt: src, mv: true, ow: true });
}

/// `(dir) = Path`.
pub fn pdirp_kw(ctx: &mut Ctx, args: &[String]) {
    let src = util::trim_trailing_sep(&full(ctx, arg(args, 0)));
    let backup = format!("{}_backup", src);
    if backup_busy(ctx, "folder", &backup) {
        return;
    }
    if !transfer_dir(ctx, &src, &backup, true, true) {
        not_saved(ctx, "folder", &src);
        return;
    }
    ctx.after_prog.push(Prog::LtrDir { src: backup, tgt: src, mv: true, ow: true });
}

/// `(file) = Path`.
pub fn pfilep_kw(ctx: &mut Ctx, args: &[String]) {
    let src = full(ctx, arg(args, 0));
    let backup = format!("{}_backup", src);
    if backup_busy(ctx, "file", &backup) {
        return;
    }
    if !transfer_file(ctx, &src, &backup, true, true) {
        drop_failed_backup(ctx, &src, &backup);
        not_saved(ctx, "file", &src);
        return;
    }
    ctx.after_prog.push(Prog::LtrFile { src: backup, tgt: src, mv: true, ow: true });
}

/// The `_backup` item that an interrupted run left for the keyword `kw` (`dir`, `(dir)`, `file` or `(file)`), if it
/// is still there.
pub fn leftover_backup(ctx: &mut Ctx, kw: &str, args: &[String]) -> Option<String> {
    let src = full(ctx, arg(args, 0));
    let src = if kw.contains("dir") { util::trim_trailing_sep(&src) } else { src };
    let backup = format!("{}_backup", src);
    occupied(&backup).then_some(backup)
}

// ----------------------------------------------------------- create / delete

pub fn plus_dir(ctx: &mut Ctx, args: &[String]) {
    let d = util::trim_trailing_sep(&full(ctx, arg(args, 0)));
    // creating a folder that exists is not an error
    match std::fs::create_dir_all(&d) {
        Ok(()) => ctx.ok(&format!("dir create \"{}\"", d)),
        Err(e) => ctx.log_err(&format!("create folder \"{}\"", d), &e),
    }
}

pub fn plus_file(ctx: &mut Ctx, args: &[String]) {
    let f = full(ctx, arg(args, 0));
    let mut flag = ctx.expand(arg(args, 1)).replace('-', " ").split_whitespace().collect::<Vec<_>>().join(" ");
    if is_null(&flag) || flag.is_empty() {
        flag = "overwrite".into();
    }
    if !eq_ic(&flag, "overwrite") && !eq_ic(&flag, "no overwrite") {
        ctx.warn(&format!("file create \"{}\": \"{}\" is not valid, use overwrite or no overwrite; nothing was done", f, flag));
        return;
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
        match util::write_text(&f, &text, enc) {
            Ok(()) => ctx.ok(&format!("file create \"{}\"", f)),
            Err(e) => ctx.log_err(&format!("create file \"{}\"", f), &e),
        }
    }
}

pub fn minus_dir(ctx: &mut Ctx, args: &[String]) {
    let d = util::trim_trailing_sep(&full(ctx, arg(args, 0)));
    let flag = ctx.expand(arg(args, 1)).trim().to_string();
    // No flag deletes the whole folder; a flag that is not "ifempty" is a mistake, and a mistake must not delete a tree.
    let if_empty = eq_ic(&flag, "ifempty");
    if !if_empty && !flag.is_empty() && !is_null(&flag) {
        ctx.warn(&format!("dir delete \"{}\": \"{}\" is not valid, use ifempty or leave it empty; nothing was deleted", d, flag));
        return;
    }
    let (par, name) = util::split_parent(&d);
    let par = if par.len() == 2 { format!("{}\\", par) } else { par };
    for m in util::list_dir(&par, &name, Kind::Dirs) {
        if dangerous_delete(ctx, &m) {
            ctx.msg(&format!("Refusing to delete \"{}\" (the launcher, its configuration file, a drive root or a parent of the launcher directory).", m));
            continue;
        }
        if if_empty {
            if util::remove_dir_if_empty(&m) {
                ctx.ok(&format!("dir delete \"{}\" (only if empty)", m));
            } else {
                ctx.log(&format!("dir delete \"{}\": not empty, kept", m));
            }
        } else if util::remove_dir_recursive(&m) {
            ctx.ok(&format!("dir delete \"{}\"", m));
        } else {
            ctx.err(&format!("delete folder \"{}\"", m));
        }
    }
}

pub fn minus_file(ctx: &mut Ctx, args: &[String]) {
    let f = full(ctx, arg(args, 0));
    let (par, name) = util::split_parent(&f);
    let par = if par.len() == 2 { format!("{}\\", par) } else { par };
    let mut failed = 0;
    for m in util::list_dir(&par, &name, Kind::Files) {
        if dangerous_delete(ctx, &m) {
            ctx.msg(&format!("Refusing to delete \"{}\" (the launcher, its configuration file, a drive root or a parent of the launcher directory).", m));
            continue;
        }
        failed += util::delete_files(&m);
    }
    if failed > 0 {
        ctx.err(&format!("delete {} file(s) of \"{}\"", failed, f));
    } else {
        ctx.ok(&format!("file delete \"{}\"", f));
    }
}

// --------------------------------------------------------------- attributes

pub fn attributes(ctx: &mut Ctx, args: &[String]) {
    let path = util::trim_trailing_sep(&full(ctx, arg(args, 0)));
    let vals = ctx.expand(arg(args, 1)).replace(' ', "");
    // "NULL" or nothing: no letter below matches, so no attribute is set (the file becomes "normal")
    // Each element is one letter (C, A, R, S, H, O, T) or the whole word; anything else is reported and left out.
    const WORDS: [&str; 9] = ["archive", "compressed", "hidden", "normal", "offline", "readonly", "system", "temporary", "null"];
    let mut list: Vec<String> = Vec::new();
    let all: Vec<String> = vals.split(',').map(|s| s.to_lowercase()).collect();
    for v in all.iter() {
        let one_letter = v.len() == 1 && "carshot".contains(v.as_str());
        if one_letter || WORDS.contains(&v.as_str()) {
            list.push(v.clone());
        } else if !(v.is_empty() && all.len() == 1) {
            ctx.warn(&format!(
                "attributes \"{}\": \"{}\" is not valid, use C, A, R, S, H, O, T (or the whole word) or NULL; it was ignored",
                path, v
            ));
        }
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
        if SetFileAttributesW(wp.as_ptr(), cur & !managed) == 0 {
            ctx.log_err(&format!("attributes \"{}\": clear the attributes", path), &std::io::Error::last_os_error());
        }
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
                h, FSCTL_SET_COMPRESSION, &state as *const u16 as *const _, 2, null_mut(), 0, &mut ret, null_mut(),
            );
            CloseHandle(h);
        }
        let mut fin = (cur & !managed) | bits;
        if fin & !FILE_ATTRIBUTE_DIRECTORY == 0 {
            fin |= FILE_ATTRIBUTE_NORMAL;
        } else {
            fin &= !FILE_ATTRIBUTE_NORMAL;
        }
        if SetFileAttributesW(wp.as_ptr(), fin) == 0 {
            ctx.log_err(&format!("attributes \"{}\": set the attributes", path), &std::io::Error::last_os_error());
        }
    }
}

// ------------------------------------------------------------------- batch

pub fn batch(ctx: &mut Ctx, args: &[String]) {
    use std::os::windows::process::CommandExt;
    // `logical` goes to cmd.exe (it runs with the architecture of the application and
    // resolves the path itself); gaP opens the file through `f`.
    let logical = {
        let e = ctx.expand(arg(args, 0));
        util::path_full(&e, &ctx.script_dir)
    };
    let f = ctx.io_path(&logical);
    let mut show = ctx.expand(arg(args, 1)).trim().to_string();
    if is_null(&show) || show.is_empty() {
        show = "hide".into();
    }
    if !util::exists(&f) {
        ctx.warn(&format!("batch \"{}\": the file does not exist, nothing to run", logical));
        return;
    }
    // The working directory is the folder as written: cmd.exe resolves it itself and
    // `Sysnative` only exists for a 32-bit process.
    let wd = util::parent_dir(&logical);
    let cmd = sys::system32_for(ctx.app_arch, "cmd.exe");
    let mut c = std::process::Command::new(cmd);
    c.current_dir(&wd);
    if eq_ic(&show, "hide") {
        // The path goes between doubled quotes, as in the original: cmd.exe drops the outer pair and keeps the
        // inner one, which is what a path with "&" or "(x86)" needs.
        c.raw_arg(format!("/C \"\"{}\"\"", logical)).creation_flags(0x0800_0000);
    } else if eq_ic(&show, "show") {
        c.raw_arg(format!("/K \"\"{}\"\"", logical));
    } else {
        ctx.warn(&format!("batch \"{}\": \"{}\" is not valid, use hide or show; nothing was run", logical, show));
        return;
    }
    if let Err(e) = c.status() {
        ctx.log_err(&format!("run batch \"{}\"", logical), &e);
    }
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
    let mut d: String = raw.chars().filter(|c| !matches!(c, '.' | '/' | ' ' | ':' | '-')).collect();
    if is_null(&d) || eq_ic(&d, "now") || d.is_empty() {
        d = local_stamp(SystemTime::now());
    }
    let mut kind = ctx.expand(arg(args, 2)).trim().to_string();
    if is_null(&kind) || kind.is_empty() {
        kind = "modified".into();
    }
    if !["modified", "created", "accessed"].iter().any(|k| eq_ic(&kind, k)) {
        ctx.warn(&format!("date \"{}\": \"{}\" is not valid, use modified, created or accessed; nothing was changed", f, kind));
        return;
    }
    if !util::exists(&f) {
        ctx.warn(&format!("date \"{}\": the file does not exist, nothing to change", f));
        return;
    }
    let Some(local) = parse_stamp(&d) else {
        ctx.warn(&format!("date \"{}\": \"{}\" is not a valid date and time, nothing was changed", f, raw));
        return;
    };
    unsafe {
        let (mut utc, mut ft): (SYSTEMTIME, FILETIME) = std::mem::zeroed();
        if TzSpecificLocalTimeToSystemTime(null(), &local, &mut utc) == 0 {
            ctx.log_err(&format!("date \"{}\": convert the local time to UTC", f), &std::io::Error::last_os_error());
            return;
        }
        if SystemTimeToFileTime(&utc, &mut ft) == 0 {
            ctx.log_err(&format!("date \"{}\": convert the time to a file time", f), &std::io::Error::last_os_error());
            return;
        }
        let wp = util::w(&f);
        let h = CreateFileW(
            wp.as_ptr(), FILE_WRITE_ATTRIBUTES, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, null(),
            OPEN_EXISTING, FILE_FLAG_BACKUP_SEMANTICS, null_mut(),
        );
        if h == INVALID_HANDLE_VALUE {
            ctx.log_err(&format!("date \"{}\": open the file", f), &std::io::Error::last_os_error());
            return;
        }
        let (c, a, m): (*const FILETIME, *const FILETIME, *const FILETIME) = match kind.trim().to_lowercase().as_str() {
            "created" => (&ft, null(), null()),
            "accessed" => (null(), &ft, null()),
            _ => (null(), null(), &ft),
        };
        if SetFileTime(h, c, a, m) == 0 {
            ctx.log_err(&format!("date \"{}\": set the time", f), &std::io::Error::last_os_error());
        }
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
    // A number of items that is not a whole number (or is negative) must not read as 0: that would delete everything.
    let number_text = ctx.expand(arg(args, 5));
    let number = match number_text.trim().parse::<i64>() {
        Ok(n) if n >= 0 => n as usize,
        _ => {
            ctx.msg(&format!(
                "keeponly: \"{}\" is not valid as the number of items to keep. Use a whole number, 0 or more. Nothing was deleted.",
                number_text.trim()
            ));
            return;
        }
    };
    // The folder as gaP resolved it: a relative path is resolved against the folder of gaP.exe, and the log is where
    // to see which folder that was.
    ctx.log(&format!("keeponly: \"{}\" resolved to \"{}\"", arg(args, 0), path));
    if !util::is_dir(&path) {
        ctx.log(&format!("keeponly: the folder \"{}\" does not exist, nothing to do", path));
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
    ctx.log(&format!(
        "keeponly: {} item(s) match \"{}\" in \"{}\", {} to keep",
        items.len(),
        if filter.is_empty() || is_null(&filter) { "*" } else { &filter },
        path,
        number
    ));
    if items.len() <= number {
        return;
    }
    // Sorted by the real time (UTC), not by the local time written as text: when the clock goes back (end of the
    // daylight saving time) the local text of a newer file is smaller than the one of an older file.
    // Each item is (time, name in lower case, path); the time is not used when the sort is by name.
    let mut keyed: Vec<(u64, String, String)> = items
        .into_iter()
        .map(|p| {
            let md = std::fs::metadata(&p).ok();
            let st = |f: fn(&std::fs::Metadata) -> std::io::Result<SystemTime>| -> u64 {
                md.as_ref().and_then(|m| f(m).ok()).map(filetime_from_systemtime).unwrap_or(0)
            };
            let time = match param.as_str() {
                "modified" => st(|m| m.modified()),
                "created" => st(|m| m.created()),
                "accessed" => st(|m| m.accessed()),
                "highest" => st(|m| m.modified()).max(st(|m| m.created())).max(st(|m| m.accessed())),
                "lowest" => st(|m| m.modified()).min(st(|m| m.created())).min(st(|m| m.accessed())),
                _ => 0,
            };
            (time, util::file_name(&p).to_lowercase(), p)
        })
        .collect();
    keyed.sort_by(|a, b| {
        let ord = if param == "name" { a.1.cmp(&b.1) } else { a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)) };
        if ascending {
            ord
        } else {
            ord.reverse()
        }
    });
    let mut failed = Vec::new();
    for (_, _, p) in keyed.into_iter().skip(number) {
        if dangerous_delete(ctx, &p) {
            ctx.msg(&format!("Refusing to delete \"{}\" (the launcher, its configuration file, a drive root or a parent of the launcher directory).", p));
            continue;
        }
        let ok = match kind {
            Kind::Files => util::delete_files(&p) == 0,
            _ => util::remove_dir_recursive(&p),
        };
        ctx.log(&format!("keeponly: delete \"{}\": {}", p, if ok { "done" } else { "FAILED" }));
        if !ok {
            failed.push(p);
        }
    }
    if !failed.is_empty() {
        ctx.msg(&format!(
            "keeponly: {} item(s) of \"{}\" could not be deleted (in use or protected), for example \"{}\".",
            failed.len(),
            path,
            failed[0]
        ));
    }
}

// ---------------------------------------------------------- text in files

pub fn replace(ctx: &mut Ctx, args: &[String]) {
    let f = full(ctx, arg(args, 0));
    let find = ctx.expand(arg(args, 1));
    let rep = ctx.expand(arg(args, 2));
    if !util::is_file(&f) || ctx.refuse_big(&f) {
        return;
    }
    if let Some((text, enc)) = util::read_text(&f) {
        let new = util::replace_all_ic(&text, &find, &rep);
        if new != text {
            match util::write_text(&f, &new, enc) {
                Ok(()) => ctx.ok(&format!("replace text in \"{}\"", f)),
                Err(e) => ctx.log_err(&format!("write \"{}\"", f), &e),
            }
        }
    }
}

pub fn replaceline(ctx: &mut Ctx, args: &[String]) {
    let f = full(ctx, arg(args, 0));
    let find = ctx.expand(arg(args, 1));
    let rep = ctx.expand(arg(args, 2));
    if !util::is_file(&f) || find.is_empty() || ctx.refuse_big(&f) {
        return;
    }
    if let Some((text, enc)) = util::read_text(&f) {
        let mut lines = util::split_lines_keep(&text);
        if let Some(i) = lines.iter().position(|(l, _)| util::contains_ic(l, &find)) {
            lines[i].0 = rep;
            let out: String = lines.iter().map(|(l, e)| format!("{}{}", l, e)).collect();
            match util::write_text(&f, &out, enc) {
                Ok(()) => ctx.ok(&format!("replace line in \"{}\"", f)),
                Err(e) => ctx.log_err(&format!("write \"{}\"", f), &e),
            }
        }
    }
}

/// `regmerge`: appends the 2nd .reg to the 1st, removing the duplicate header.
pub fn regmerge(ctx: &mut Ctx, args: &[String]) {
    let f1 = full(ctx, arg(args, 0));
    let f2 = full(ctx, arg(args, 1));
    if !util::is_file(&f1) || !util::is_file(&f2) || ctx.refuse_big(&f1) || ctx.refuse_big(&f2) {
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
    match util::write_text(&f1, &out, e1) {
        Ok(()) => ctx.ok(&format!("regmerge \"{}\" + \"{}\"", f1, f2)),
        Err(e) => ctx.log_err(&format!("write \"{}\"", f1), &e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_trash_that_gap_makes_is_deleted() {
        // what free_name makes
        assert!(is_trash_name("Data", "Data.gap_del"));
        assert!(is_trash_name("Data", "Data.gap_del2"));
        assert!(is_trash_name("Data", "DATA.GAP_DEL13"));
        // folders of the user that only start like it
        assert!(!is_trash_name("Data", "Data.gap_del_old"));
        assert!(!is_trash_name("Data", "Data.gap_del_teste"));
        assert!(!is_trash_name("Data", "Data.gap_delete"));
        assert!(!is_trash_name("Data", "Data.gap_del.bak"));
        assert!(!is_trash_name("Data", "Data"));
        assert!(!is_trash_name("Data", "Data.gap_old"));
    }
}
