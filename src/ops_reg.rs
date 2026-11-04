//! Registry keywords:
//! regkey, regvalue, (regkey), (regvalue), ->regkey, <-regkey, ->regvalue,
//! <-regvalue, +regkey, +regvalue, -regkey, -regvalue, regexport, regimport.

use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND};

use crate::ctx::{Ctx, Prog};
use crate::reg::{self, View};
use crate::{sys, util};
use util::{arg, eq_ic, is_null};

/// Registry view flag: NULL/empty/x86 = 32 bits; x64 = 64 bits.
fn view_of(ctx: &mut Ctx, s: &str) -> View {
    let f = ctx.expand(s);
    View::from_flag(if f.is_empty() || is_null(&f) { "x86" } else { &f })
}

fn key_of(ctx: &mut Ctx, s: &str) -> String {
    ctx.expand_ex(s, true).trim_end_matches('\\').to_string()
}

/// Message for a registry change that failed. `code` is the system error: it is shown with its text, and the
/// hint about `!admin` only when the error is "access denied", gaP is not running as administrator and at least one
/// of `keys` is outside HKEY_CURRENT_USER (a key of the current user does not need the right, so the hint would
/// only mislead). Without a code (reg.exe gave none) or without keys (a .reg file can hold any key) the hint is
/// shown whenever gaP is not an administrator.
fn change_failed(ctx: &Ctx, text: &str, code: Option<u32>, keys: &[&str]) {
    let mut m = text.to_string();
    if let Some(c) = code {
        m.push_str(&format!(" {}.", std::io::Error::from_raw_os_error(c as i32)));
    }
    let outside_hkcu = keys.is_empty() || keys.iter().any(|k| !reg::is_hkcu(k));
    if code.is_none_or(|c| c == ERROR_ACCESS_DENIED) && outside_hkcu && !sys::is_admin() {
        m.push_str(" Administrator rights may be required. Please, set \"!admin = 1\" in the [GENERAL] section and try again.");
    }
    ctx.msg(&m);
}

/// "HKCU\Software\K\Value" -> ("HKCU\Software\K", "Value").
fn split_value(path: &str) -> (String, String) {
    match path.rfind('\\') {
        Some(i) => (path[..i].to_string(), path[i + 1..].to_string()),
        None => (path.to_string(), String::new()),
    }
}

fn settings_reg(ctx: &mut Ctx, arg_val: &str) -> String {
    let mut f = if is_null(arg_val) {
        format!("{}\\settings.reg", ctx.script_dir.trim_end_matches('\\'))
    } else {
        let e = ctx.expand(arg_val);
        util::path_full(&e, &ctx.script_dir)
    };
    if !f.to_lowercase().ends_with(".reg") {
        if f.ends_with('\\') {
            f.push_str("settings.reg");
        } else {
            f.push_str("\\settings.reg");
        }
    }
    f
}

// ---------------------------------------------------------------- copy / move

// NEW (gaP-rs): a copy that fails (or a move whose source cannot be removed) is reported; before it
// was silent. A missing source is still silent: there is nothing to copy or to restore.
/// `false` = the copy failed (it was reported); a source that is not there is not a failure, there is nothing to copy.
pub fn copy_key(ctx: &Ctx, src: &str, tgt: &str, mv: bool, view: View) -> bool {
    if eq_ic(src, tgt) {
        return true;
    }
    match reg::copy_tree(src, tgt, view) {
        Ok(()) => {
            ctx.ok(&format!("reg {} key \"{}\" -> \"{}\"", if mv { "move" } else { "copy" }, src, tgt));
            if mv {
                if let Err(code) = reg::delete_tree(src, view) {
                    let t = format!("The registry key \"{}\" was copied but could not be removed.", src);
                    change_failed(ctx, &t, Some(code), &[src, tgt]);
                }
            }
            true
        }
        Err(e) if e.code == ERROR_FILE_NOT_FOUND && e.step == reg::STEP_OPEN_SOURCE => {
            ctx.log(&format!("reg copy key \"{}\": it does not exist, nothing to copy", src));
            true
        }
        Err(e) => {
            // The step tells which of the calls got the error; a message to the user does not need it.
            ctx.log_err(
                &format!("copy registry key \"{}\" to \"{}\" (step: {})", src, tgt, e.step),
                &format!("error {}", e.code),
            );
            let mut t = format!("The registry key \"{}\" could not be copied to \"{}\".", src, tgt);
            if mv {
                t.push_str(" The key that was to be moved was not removed.");
            }
            change_failed(ctx, &t, Some(e.code), &[src, tgt]);
            false
        }
    }
}

/// `false` = the copy failed (it was reported); a source that is not there is not a failure.
pub fn copy_value(ctx: &Ctx, src: &str, tgt: &str, mv: bool, view: View) -> bool {
    if eq_ic(src, tgt) {
        return true;
    }
    let (sk, sv) = split_value(src);
    let (tk, tv) = split_value(tgt);
    let Some(v) = reg::read_value(&sk, &sv, view) else {
        ctx.log(&format!("reg copy value \"{}\": it does not exist, nothing to copy", src));
        return true;
    };
    if let Err(code) = reg::write_value(&tk, &tv, v.kind, &v.data, view) {
        let t = format!("The registry value \"{}\" could not be copied to \"{}\".", src, tgt);
        change_failed(ctx, &t, Some(code), &[&sk, &tk]);
        return false;
    }
    ctx.ok(&format!("reg {} value \"{}\" -> \"{}\"", if mv { "move" } else { "copy" }, src, tgt));
    if mv {
        if let Err(code) = reg::delete_value(&sk, &sv, view) {
            let t = format!("The registry value \"{}\" was copied but could not be removed.", src);
            change_failed(ctx, &t, Some(code), &[&sk]);
        }
    }
    true
}

/// Message for an item that was skipped because its original data could not be saved: nothing was changed.
fn not_saved(ctx: &Ctx, what: &str, src: &str) {
    ctx.msg(&format!("The original {} \"{}\" could not be saved, so this item was skipped and nothing was changed.", what, src));
}

pub fn ltr_regkey(ctx: &mut Ctx, args: &[String], rtl: bool) {
    let (mut a, mut b) = (arg(args, 0).to_string(), arg(args, 1).to_string());
    if rtl {
        std::mem::swap(&mut a, &mut b);
    }
    let src = key_of(ctx, &a);
    let tgt = key_of(ctx, &b);
    let cflag = ctx.expand(arg(args, 2));
    let view = view_of(ctx, arg(args, 3));
    copy_key(ctx, &src, &tgt, eq_ic(&cflag, "move"), view);
}

pub fn ltr_regvalue(ctx: &mut Ctx, args: &[String], rtl: bool) {
    let (mut a, mut b) = (arg(args, 0).to_string(), arg(args, 1).to_string());
    if rtl {
        std::mem::swap(&mut a, &mut b);
    }
    let src = key_of(ctx, &a);
    let tgt = key_of(ctx, &b);
    let cflag = ctx.expand(arg(args, 2));
    let view = view_of(ctx, arg(args, 3));
    copy_value(ctx, &src, &tgt, eq_ic(&cflag, "move"), view);
}

// ------------------------------------------------------------ create / delete

pub fn plus_regkey(ctx: &mut Ctx, args: &[String]) {
    let k = key_of(ctx, arg(args, 0));
    let view = view_of(ctx, arg(args, 1));
    if let Err(code) = reg::create_key(&k, view) {
        change_failed(ctx, &format!("The registry key \"{}\" could not be created.", k), Some(code), &[&k]);
    } else {
        ctx.ok(&format!("reg create key \"{}\"", k));
    }
}

pub fn plus_regvalue(ctx: &mut Ctx, args: &[String]) {
    let key = key_of(ctx, arg(args, 0));
    let name = ctx.expand(arg(args, 1));
    let data = ctx.expand(arg(args, 2));
    let mut kind = ctx.expand(arg(args, 3));
    if is_null(&kind) || kind.is_empty() {
        kind = "REG_SZ".into();
    }
    let sep = ctx.expand(arg(args, 4));
    let view = view_of(ctx, arg(args, 5));
    match reg::build_value(&kind, &data, &sep) {
        Some((t, bytes)) => {
            if let Err(code) = reg::write_value(&key, &name, t, &bytes, view) {
                let text = format!("The registry value \"{}\\{}\" could not be written.", key, name);
                change_failed(ctx, &text, Some(code), &[&key]);
            } else {
                ctx.ok(&format!("reg write value \"{}\\{}\"", key, name));
            }
        }
        None => ctx.msg(&format!("The value \"{}\" can't be converted to the type \"{}\".", data, kind)),
    }
}

pub fn minus_regkey(ctx: &mut Ctx, args: &[String]) {
    let k = key_of(ctx, arg(args, 0));
    let eflag = ctx.expand(arg(args, 1));
    let view = view_of(ctx, arg(args, 2));
    delete_key(ctx, &k, eq_ic(&eflag, "ifempty"), view);
}

/// Deletes the key (`if_empty`: only if it has no values and no subkeys). A key that is not there is not an error;
/// a key that is still there after the attempt is reported.
pub fn delete_key(ctx: &Ctx, key: &str, if_empty: bool, view: View) {
    let st = if if_empty { reg::key_state(key, view) } else { 1 };
    if st > 0 && (!if_empty || st == 2) {
        match reg::delete_tree(key, view) {
            Ok(()) => ctx.ok(&format!("reg delete key \"{}\"", key)),
            Err(code) => {
                if reg::key_exists(key, view) {
                    ctx.err(&format!("delete registry key \"{}\" (error {})", key, code));
                    change_failed(ctx, &format!("The registry key \"{}\" could not be deleted.", key), Some(code), &[key]);
                }
            }
        }
    }
}

/// Empties the key (values and subkeys) and keeps the key itself, so its name is never left in the "being deleted"
/// state (see `reg::clear_tree`). A failure is reported and `false` is returned.
fn clear_key(ctx: &Ctx, key: &str, view: View) -> bool {
    match reg::clear_tree(key, view) {
        Ok(()) => {
            ctx.ok(&format!("reg clear key \"{}\"", key));
            true
        }
        Err(code) => {
            ctx.err(&format!("clear registry key \"{}\" (error {})", key, code));
            change_failed(ctx, &format!("The registry key \"{}\" could not be emptied.", key), Some(code), &[key]);
            false
        }
    }
}

/// Puts the key saved in `backup` back in `key` (the AFTER step of `regkey` and `(regkey)`), in an order that never
/// removes something that cannot be put back: the saved key must be readable before anything is touched, the key
/// is emptied only then, and the saved key is removed only after it was copied back. When something fails the
/// message says so and what was not touched stays as it is.
pub fn restore_key(ctx: &Ctx, key: &str, backup: &str, view: View) {
    if !reg::key_present(backup, view) {
        // Nothing was saved (the key did not exist before the run): only what the application made is removed.
        ctx.log(&format!("reg restore \"{}\": nothing was saved, only what the application made is removed", key));
        clear_key(ctx, key, view);
        return;
    }
    if let Err(code) = reg::readable(backup, view) {
        ctx.err(&format!("open registry key \"{}\" (error {})", backup, code));
        let t = format!(
            "The saved registry key \"{}\" can't be read, so \"{}\" was left as it is and nothing was removed.",
            backup, key
        );
        change_failed(ctx, &t, Some(code), &[backup, key]);
        return;
    }
    if !clear_key(ctx, key, view) {
        ctx.msg(&format!("The saved registry key \"{}\" was not restored and was not removed.", backup));
        return;
    }
    copy_key(ctx, backup, key, true, view);
}

/// The `_backup` item that an interrupted run left for the registry keyword `kw` (`regkey`, `(regkey)`, `regvalue` or
/// `(regvalue)`), if it is still there.
pub fn leftover_backup(ctx: &mut Ctx, kw: &str, args: &[String]) -> Option<String> {
    let src = key_of(ctx, arg(args, 0));
    let flag = if matches!(kw, "regkey" | "regvalue") { arg(args, 2) } else { arg(args, 1) };
    let view = view_of(ctx, flag);
    let backup = format!("{}_backup", src);
    let present = if kw.contains("value") {
        let (k, v) = split_value(&backup);
        reg::value_exists(&k, &v, view)
    } else {
        reg::key_present(&backup, view)
    };
    present.then_some(backup)
}

/// Deletes the value. A value that is not there is not an error; one that is still there after the attempt is
/// reported.
fn delete_value_reported(ctx: &Ctx, key: &str, name: &str, view: View) {
    match reg::delete_value(key, name, view) {
        Ok(()) => ctx.ok(&format!("reg delete value \"{}\\{}\"", key, name)),
        Err(code) => {
            if reg::value_exists(key, name, view) {
                ctx.err(&format!("delete registry value \"{}\\{}\" (error {})", key, name, code));
                let t = format!("The registry value \"{}\\{}\" could not be deleted.", key, name);
                change_failed(ctx, &t, Some(code), &[key]);
            }
        }
    }
}

pub fn minus_regvalue(ctx: &mut Ctx, args: &[String]) {
    let k = key_of(ctx, arg(args, 0));
    let name = ctx.expand(arg(args, 1));
    let view = view_of(ctx, arg(args, 2));
    delete_value_reported(ctx, &k, &name, view);
}

// ----------------------------------------------------- backup / restore

/// `regkey = Key [:: file.reg] [:: x86|x64]` (BEFORE only).
pub fn regkey_kw(ctx: &mut Ctx, args: &[String]) {
    let src = key_of(ctx, arg(args, 0));
    let file = settings_reg(ctx, arg(args, 1));
    let view = view_of(ctx, arg(args, 2));
    let backup = format!("{}_backup", src);
    if !copy_key(ctx, &src, &backup, false, view) {
        not_saved(ctx, "registry key", &src);
        return;
    }
    delete_key(ctx, &src, false, view);
    import_or_warn(ctx, &file, view);
    ctx.after_prog.push(Prog::RegExport { key: format!("{}\\", src), file, view });
    ctx.after_prog.push(Prog::RestoreRegKey { key: src.clone(), backup, view });
    ctx.after_prog.push(Prog::MinusRegKey { key: src.clone(), if_empty: true, view });
    ctx.after_prog.push(Prog::CleanParents { key: src, view });
}

/// `regvalue = Key\Value [:: file.reg] [:: x86|x64]` (BEFORE only).
pub fn regvalue_kw(ctx: &mut Ctx, args: &[String]) {
    let src = key_of(ctx, arg(args, 0));
    let file = settings_reg(ctx, arg(args, 1));
    let view = view_of(ctx, arg(args, 2));
    let (k, v) = split_value(&src);
    let backup = format!("{}_backup", src);
    if !copy_value(ctx, &src, &backup, false, view) {
        not_saved(ctx, "registry value", &src);
        return;
    }
    import_or_warn(ctx, &file, view);
    ctx.after_prog.push(Prog::RegExport { key: src.clone(), file, view });
    ctx.after_prog.push(Prog::MinusRegValue { key: k.clone(), name: v, view });
    ctx.after_prog.push(Prog::LtrRegValue { src: backup, tgt: src, mv: true, view });
    ctx.after_prog.push(Prog::MinusRegKey { key: k.clone(), if_empty: true, view });
    ctx.after_prog.push(Prog::CleanParents { key: k, view });
}

/// `(regkey) = Key [:: x86|x64]` (BEFORE only).
pub fn pregkeyp_kw(ctx: &mut Ctx, args: &[String]) {
    let src = key_of(ctx, arg(args, 0));
    let view = view_of(ctx, arg(args, 1));
    let backup = format!("{}_backup", src);
    if !copy_key(ctx, &src, &backup, false, view) {
        not_saved(ctx, "registry key", &src);
        return;
    }
    ctx.after_prog.push(Prog::RestoreRegKey { key: src.clone(), backup, view });
    ctx.after_prog.push(Prog::MinusRegKey { key: src.clone(), if_empty: true, view });
    ctx.after_prog.push(Prog::CleanParents { key: src, view });
}

/// `(regvalue) = Key\Value [:: x86|x64]` (BEFORE only).
pub fn pregvaluep_kw(ctx: &mut Ctx, args: &[String]) {
    let src = key_of(ctx, arg(args, 0));
    let view = view_of(ctx, arg(args, 1));
    let (k, v) = split_value(&src);
    let backup = format!("{}_backup", src);
    if !copy_value(ctx, &src, &backup, false, view) {
        not_saved(ctx, "registry value", &src);
        return;
    }
    ctx.after_prog.push(Prog::MinusRegValue { key: k.clone(), name: v, view });
    ctx.after_prog.push(Prog::LtrRegValue { src: backup, tgt: src, mv: true, view });
    ctx.after_prog.push(Prog::MinusRegKey { key: k.clone(), if_empty: true, view });
    ctx.after_prog.push(Prog::CleanParents { key: k, view });
}

/// Runs an automatic registry task (AFTER queue).
pub fn run_prog_reg(ctx: &Ctx, p: &Prog) {
    match p {
        Prog::RegExport { key, file, view } => {
            let note = " The registry was left as it is and the original data stays in the \"_backup\" item.";
            export_to(ctx, key, file, *view, note);
        }
        Prog::MinusRegKey { key, if_empty, view } => delete_key(ctx, key, *if_empty, *view),
        Prog::RestoreRegKey { key, backup, view } => {
            if is_unsaved(ctx, key) {
                ctx.warn(&format!("reg restore \"{}\": skipped, its export failed", key));
            } else {
                restore_key(ctx, key, backup, *view);
            }
        }
        Prog::MinusRegValue { key, name, view } => {
            if is_unsaved(ctx, &format!("{}\\{}", key, name)) {
                ctx.warn(&format!("reg delete value \"{}\\{}\": skipped, its export failed", key, name));
            } else {
                delete_value_reported(ctx, key, name, *view);
            }
        }
        Prog::LtrRegValue { src, tgt, mv, view } => {
            if is_unsaved(ctx, tgt) {
                ctx.warn(&format!("reg restore \"{}\": skipped, its export failed", tgt));
            } else {
                copy_value(ctx, src, tgt, *mv, *view);
            }
        }
        Prog::CleanParents { key, view } => reg::delete_empty_parents(key, *view),
        _ => {}
    }
}

// ----------------------------------------------------------- export / import

fn reg_exe() -> String {
    sys::system32("reg.exe")
}

/// Imports a .reg file. `false` = reg.exe failed (a missing file is not a failure).
pub fn import_file(file: &str, view: View) -> bool {
    if util::is_file(file) {
        sys::run_hidden_wait(&reg_exe(), &["import", file, view.reg_exe_flag()])
    } else {
        true
    }
}

fn import_or_warn(ctx: &Ctx, file: &str, view: View) {
    if !util::is_file(file) {
        ctx.log(&format!("reg import \"{}\": the file does not exist, nothing to import", file));
    } else if import_file(file, view) {
        ctx.ok(&format!("reg import \"{}\"", file));
    } else {
        ctx.err(&format!("reg import \"{}\" failed", file));
        change_failed(ctx, &format!("The registry file \"{}\" could not be imported.", file), None, &[]);
    }
}

fn reg_escape(name: &str) -> String {
    name.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Keeps only the value `name` of the .reg file exported from the key `key`.
fn filter_single_value(file: &str, key: &str, name: &str) {
    let Some((text, enc)) = util::read_text(file) else { return };
    let want_key = key.to_lowercase();
    let want_val = format!("\"{}\"=", reg_escape(name)).to_lowercase();
    let mut out = String::new();
    let (mut in_key, mut keep_cont) = (false, false);
    let mut header_done = false;
    for (line, _) in util::split_lines_keep(&text) {
        let t = line.trim_end();
        if !header_done {
            if t.is_empty() {
                continue;
            }
            if t.eq_ignore_ascii_case("REGEDIT4") || t.eq_ignore_ascii_case("Windows Registry Editor Version 5.00") {
                out.push_str(t);
                out.push_str("\r\n\r\n");
                header_done = true;
                continue;
            }
            header_done = true;
        }
        if t.starts_with('[') && t.ends_with(']') {
            // [HKEY_X\path] -> compare without the root
            let inner = &t[1..t.len() - 1];
            let rest = inner.split_once('\\').map(|x| x.1).unwrap_or("").to_lowercase();
            let want_rest = want_key.split_once('\\').map(|x| x.1).unwrap_or("").to_string();
            in_key = rest == want_rest;
            keep_cont = false;
            if in_key {
                out.push_str(t);
                out.push_str("\r\n");
            }
            continue;
        }
        if in_key && (keep_cont || t.to_lowercase().starts_with(&want_val)) {
            out.push_str(t);
            out.push_str("\r\n");
            keep_cont = t.ends_with('\\');
        }
    }
    out.push_str("\r\n");
    let enc = if matches!(enc, util::Enc::Ansi) { util::Enc::Ansi } else { enc };
    let _ = util::write_text(file, &out, enc);
}

/// The key or value as `unsaved` names it: the text without the backslash that marks a key, in lower case.
fn unsaved_id(spec: &str) -> String {
    spec.trim_end_matches('\\').to_lowercase()
}

/// `spec` is a key or value whose export failed in this run, so its AFTER steps must leave it as it is.
fn is_unsaved(ctx: &Ctx, spec: &str) -> bool {
    let id = unsaved_id(spec);
    ctx.unsaved.borrow().contains(&id)
}

/// Core of `regexport` with resolved paths. `key_or_value` ends in '\' when it is a key; otherwise it is
/// "Key\Value". The registry is exported to a file beside the target and that file takes the place of the target
/// only when `reg.exe` succeeded, so a failure never costs the file that was there. `false` = the export failed
/// (it was reported, with `note` after the message); a key or value that does not exist is not a failure.
pub fn export_to(ctx: &Ctx, key_or_value: &str, file: &str, view: View, note: &str) -> bool {
    let is_key = key_or_value.ends_with('\\');
    let spec = key_or_value.trim_end_matches('\\');
    let (key, value) = if is_key { (spec.to_string(), String::new()) } else { split_value(spec) };
    let exists = if is_key { reg::key_exists(&key, view) } else { reg::value_exists(&key, &value, view) };
    if !exists {
        ctx.log(&format!("reg export \"{}\": it does not exist, nothing to export", spec));
        return true;
    }
    let par = util::parent_dir(file);
    if !par.is_empty() {
        let _ = std::fs::create_dir_all(&par);
    }
    let tmp = format!("{}.gap_tmp.reg", file);
    let _ = std::fs::remove_file(&tmp);
    let mut done = sys::run_hidden_wait(&reg_exe(), &["export", &key, &tmp, "/y", view.reg_exe_flag()]) && util::is_file(&tmp);
    if done && !is_key {
        filter_single_value(&tmp, &key, &value);
    }
    if done {
        if let Err(e) = util::move_over(&tmp, file) {
            ctx.log_err(&format!("replace \"{}\" with the exported file", file), &e);
            done = false;
        }
    }
    if !done {
        let _ = std::fs::remove_file(&tmp);
        ctx.err(&format!("reg export \"{}\" -> \"{}\" failed", spec, file));
        ctx.unsaved.borrow_mut().push(unsaved_id(spec));
        ctx.msg(&format!(
            "The registry {} \"{}\" could not be exported to \"{}\". The previous file was kept.{}",
            if is_key { "key" } else { "value" },
            spec,
            file,
            note
        ));
        return false;
    }
    ctx.ok(&format!("reg export \"{}\" -> \"{}\"", spec, file));
    true
}

pub fn regexport_kw(ctx: &mut Ctx, args: &[String]) {
    let file = settings_reg(ctx, arg(args, 1));
    let spec = ctx.expand_ex(arg(args, 0), true);
    let view = view_of(ctx, arg(args, 2));
    export_to(ctx, &spec, &file, view, "");
}

pub fn regimport_kw(ctx: &mut Ctx, args: &[String]) {
    let file = settings_reg(ctx, arg(args, 0));
    let view = view_of(ctx, arg(args, 1));
    import_or_warn(ctx, &file, view);
}
