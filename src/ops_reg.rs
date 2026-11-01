//! Registry keywords:
//! regkey, regvalue, (regkey), (regvalue), ->regkey, <-regkey, ->regvalue,
//! <-regvalue, +regkey, +regvalue, -regkey, -regvalue, regexport, regimport.

use crate::ctx::{Ctx, Prog};
use crate::reg::{self, View};
use crate::{sys, util};
use util::eq_ic;

const NULL: &str = "NULL";

fn arg(a: &[String], i: usize) -> &str {
    a.get(i).map(|s| s.as_str()).unwrap_or(NULL)
}

/// Registry view flag: NULL/empty/x86 = 32 bits; x64 = 64 bits.
fn view_of(ctx: &mut Ctx, s: &str) -> View {
    let f = ctx.expand(s);
    View::from_flag(if f.is_empty() || f == NULL { "x86" } else { &f })
}

fn key_of(ctx: &mut Ctx, s: &str) -> String {
    ctx.expand_ex(s, true).trim_end_matches('\\').to_string()
}

/// "HKCU\Software\K\Value" -> ("HKCU\Software\K", "Value").
fn split_value(path: &str) -> (String, String) {
    match path.rfind('\\') {
        Some(i) => (path[..i].to_string(), path[i + 1..].to_string()),
        None => (path.to_string(), String::new()),
    }
}

fn settings_reg(ctx: &mut Ctx, arg_val: &str) -> String {
    let mut f = if arg_val == NULL {
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

pub fn copy_key(src: &str, tgt: &str, mv: bool, view: View) {
    if eq_ic(src, tgt) || !reg::key_exists(src, view) {
        return;
    }
    if reg::copy_tree(src, tgt, view) && mv {
        reg::delete_tree(src, view);
    }
}

pub fn copy_value(src: &str, tgt: &str, mv: bool, view: View) {
    if eq_ic(src, tgt) {
        return;
    }
    let (sk, sv) = split_value(src);
    let (tk, tv) = split_value(tgt);
    let Some(v) = reg::read_value(&sk, &sv, view) else { return };
    if reg::write_value(&tk, &tv, v.kind, &v.data, view) && mv {
        reg::delete_value(&sk, &sv, view);
    }
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
    copy_key(&src, &tgt, eq_ic(&cflag, "move"), view);
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
    copy_value(&src, &tgt, eq_ic(&cflag, "move"), view);
}

// ------------------------------------------------------------ create / delete

pub fn plus_regkey(ctx: &mut Ctx, args: &[String]) {
    let k = key_of(ctx, arg(args, 0));
    let view = view_of(ctx, arg(args, 1));
    reg::create_key(&k, view);
}

pub fn plus_regvalue(ctx: &mut Ctx, args: &[String]) {
    let key = key_of(ctx, arg(args, 0));
    let name = ctx.expand(arg(args, 1));
    let data = ctx.expand(arg(args, 2));
    let mut kind = ctx.expand(arg(args, 3));
    if kind == NULL || kind.is_empty() {
        kind = "REG_SZ".into();
    }
    let sep = ctx.expand(arg(args, 4));
    let view = view_of(ctx, arg(args, 5));
    match reg::build_value(&kind, &data, &sep) {
        Some((t, bytes)) => {
            reg::write_value(&key, &name, t, &bytes, view);
        }
        None => ctx.msg(&format!("The value \"{}\" can't be converted to the type \"{}\".", data, kind)),
    }
}

pub fn minus_regkey(ctx: &mut Ctx, args: &[String]) {
    let k = key_of(ctx, arg(args, 0));
    let eflag = ctx.expand(arg(args, 1));
    let view = view_of(ctx, arg(args, 2));
    delete_key(&k, eq_ic(&eflag, "ifempty"), view);
}

pub fn delete_key(key: &str, if_empty: bool, view: View) {
    let st = reg::key_state(key, view);
    if st > 0 && (!if_empty || st == 2) {
        reg::delete_tree(key, view);
    }
}

pub fn minus_regvalue(ctx: &mut Ctx, args: &[String]) {
    let k = key_of(ctx, arg(args, 0));
    let name = ctx.expand(arg(args, 1));
    let view = view_of(ctx, arg(args, 2));
    reg::delete_value(&k, &name, view);
}

// ----------------------------------------------------- backup / restore

/// `regkey = Key [:: file.reg] [:: x86|x64]` (BEFORE only).
pub fn regkey_kw(ctx: &mut Ctx, args: &[String]) {
    let src = key_of(ctx, arg(args, 0));
    let file = settings_reg(ctx, arg(args, 1));
    let view = view_of(ctx, arg(args, 2));
    let backup = format!("{}_backup", src);
    copy_key(&src, &backup, false, view);
    delete_key(&src, false, view);
    import_file(&file, view);
    ctx.after_prog.push(Prog::RegExport { key: format!("{}\\", src), file, view });
    ctx.after_prog.push(Prog::MinusRegKey { key: src.clone(), if_empty: false, view });
    ctx.after_prog.push(Prog::LtrRegKey { src: backup, tgt: src.clone(), mv: true, view });
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
    copy_value(&src, &backup, false, view);
    import_file(&file, view);
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
    copy_key(&src, &backup, false, view);
    ctx.after_prog.push(Prog::MinusRegKey { key: src.clone(), if_empty: false, view });
    ctx.after_prog.push(Prog::LtrRegKey { src: backup, tgt: src.clone(), mv: true, view });
    ctx.after_prog.push(Prog::MinusRegKey { key: src.clone(), if_empty: true, view });
    ctx.after_prog.push(Prog::CleanParents { key: src, view });
}

/// `(regvalue) = Key\Value [:: x86|x64]` (BEFORE only).
pub fn pregvaluep_kw(ctx: &mut Ctx, args: &[String]) {
    let src = key_of(ctx, arg(args, 0));
    let view = view_of(ctx, arg(args, 1));
    let (k, v) = split_value(&src);
    let backup = format!("{}_backup", src);
    copy_value(&src, &backup, false, view);
    ctx.after_prog.push(Prog::MinusRegValue { key: k.clone(), name: v, view });
    ctx.after_prog.push(Prog::LtrRegValue { src: backup, tgt: src, mv: true, view });
    ctx.after_prog.push(Prog::MinusRegKey { key: k.clone(), if_empty: true, view });
    ctx.after_prog.push(Prog::CleanParents { key: k, view });
}

/// Runs an automatic registry task (AFTER queue).
pub fn run_prog_reg(ctx: &Ctx, p: &Prog) {
    match p {
        Prog::RegExport { key, file, view } => export_to(ctx, key, file, *view),
        Prog::MinusRegKey { key, if_empty, view } => delete_key(key, *if_empty, *view),
        Prog::MinusRegValue { key, name, view } => {
            reg::delete_value(key, name, *view);
        }
        Prog::LtrRegKey { src, tgt, mv, view } => copy_key(src, tgt, *mv, *view),
        Prog::LtrRegValue { src, tgt, mv, view } => copy_value(src, tgt, *mv, *view),
        Prog::CleanParents { key, view } => reg::delete_empty_parents(key, *view),
        _ => {}
    }
}

// ----------------------------------------------------------- export / import

fn reg_exe() -> String {
    sys::system32("reg.exe")
}

pub fn import_file(file: &str, view: View) {
    if util::is_file(file) {
        sys::run_hidden_wait(&reg_exe(), &["import", file, view.reg_exe_flag()]);
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

/// Core of `regexport` with resolved paths. `key_or_value` ends in
/// '\' when it is a key; otherwise it is "Key\Value".
pub fn export_to(_ctx: &Ctx, key_or_value: &str, file: &str, view: View) {
    let is_key = key_or_value.ends_with('\\');
    let spec = key_or_value.trim_end_matches('\\');
    let (key, value) = if is_key { (spec.to_string(), String::new()) } else { split_value(spec) };
    let exists = if is_key { reg::key_exists(&key, view) } else { reg::value_exists(&key, &value, view) };
    if !exists {
        return;
    }
    if util::exists(file) {
        util::delete_files(file);
    }
    let par = util::parent_dir(file);
    if !par.is_empty() {
        let _ = std::fs::create_dir_all(&par);
    }
    sys::run_hidden_wait(&reg_exe(), &["export", &key, file, "/y", view.reg_exe_flag()]);
    if !is_key {
        filter_single_value(file, &key, &value);
    }
}

pub fn regexport_kw(ctx: &mut Ctx, args: &[String]) {
    let file = settings_reg_dirlike(ctx, arg(args, 1));
    let spec = ctx.expand_ex(arg(args, 0), true);
    let view = view_of(ctx, arg(args, 2));
    export_to(ctx, &spec, &file, view);
}

/// Like `settings_reg`, but without forcing ".reg" on paths that already end in .reg.
fn settings_reg_dirlike(ctx: &mut Ctx, a: &str) -> String {
    settings_reg(ctx, a)
}

pub fn regimport_kw(ctx: &mut Ctx, args: &[String]) {
    let file = settings_reg(ctx, arg(args, 0));
    let view = view_of(ctx, arg(args, 1));
    import_file(&file, view);
}
