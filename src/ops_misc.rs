//! Miscellaneous keywords: clip, clean, delay, envvar, iniread, iniwrite,
//! killprocess, message, nowait, quit, regdll, run, service, stringreplace,
//! symlink, systemrefresh, uservar.

use std::ptr::null_mut;

use windows_sys::Win32::Storage::FileSystem::{CreateSymbolicLinkW, DeleteFileW};
use windows_sys::Win32::Storage::FileSystem::RemoveDirectoryW;

use crate::ctx::{exit_now, Ctx, Prog, Section, FUNC_MACROS, RESERVED_VARS};
use crate::ini::IniDoc;
use crate::{clean, sys, ui, util};
use util::eq_ic;

const NULL: &str = "NULL";

fn arg(a: &[String], i: usize) -> &str {
    a.get(i).map(|s| s.as_str()).unwrap_or(NULL)
}

/// Full path, as written (what other programs and links must receive).
fn full_logical(ctx: &mut Ctx, p: &str) -> String {
    let e = ctx.expand(p);
    util::path_full(&e, &ctx.script_dir)
}

/// Full path as gaP has to open it so that it means what it means to the main application.
fn full(ctx: &mut Ctx, p: &str) -> String {
    let f = full_logical(ctx, p);
    ctx.io_path(&f)
}

pub fn clip(ctx: &mut Ctx, args: &[String]) {
    let t = ctx.expand(arg(args, 0));
    if t == "*EMPTY*" {
        ui::clip_put("");
    } else {
        ui::clip_put(&t);
    }
}

pub fn clean_kw(ctx: &mut Ctx, args: &[String]) {
    let f = full(ctx, arg(args, 0));
    clean::clean_file(&f);
}

pub fn delay(ctx: &mut Ctx, args: &[String]) {
    let ms = ctx.expand_num(arg(args, 0)).clamp(0, i32::MAX as i64) as u64;
    std::thread::sleep(std::time::Duration::from_millis(ms));
}

pub fn envvar(ctx: &mut Ctx, args: &[String]) {
    let name = ctx.expand(arg(args, 0));
    let val = ctx.expand(arg(args, 1));
    if !name.is_empty() && !name.contains('=') && !name.contains('\0') && !val.contains('\0') {
        std::env::set_var(name, val);
    }
}

/// Defines a user variable, validating the name.
pub fn set_user_var(ctx: &mut Ctx, name: &str, value: &str) {
    let reserved = RESERVED_VARS.iter().any(|r| eq_ic(r, name)) || FUNC_MACROS.iter().any(|r| eq_ic(r, name));
    if reserved || name.is_empty() {
        ctx.msg(&format!("The variable name \"{}\" can't be used. Please, choose another name for it.", name));
    } else {
        ctx.add_var(name, value);
    }
}

pub fn uservar(ctx: &mut Ctx, args: &[String]) {
    let name = arg(args, 0).to_string();
    let reserved = RESERVED_VARS.iter().any(|r| eq_ic(r, &name)) || FUNC_MACROS.iter().any(|r| eq_ic(r, &name));
    if reserved {
        set_user_var(ctx, &name, "");
        return;
    }
    let v = ctx.expand(arg(args, 1));
    set_user_var(ctx, &name, &v);
}

pub fn giniread(ctx: &mut Ctx, args: &[String]) {
    let file = full(ctx, arg(args, 0));
    let sec = ctx.expand(arg(args, 1));
    let key = ctx.expand(arg(args, 2));
    let var = ctx.expand(arg(args, 3));
    if !util::is_file(&file) {
        return;
    }
    let doc = IniDoc::load(&file);
    let section = if sec == NULL { None } else { Some(sec.as_str()) };
    let val = doc.get(section, &key).unwrap_or_else(|| "NOT_FOUND!".to_string());
    set_user_var(ctx, &var, &val);
}

pub fn giniwrite(ctx: &mut Ctx, args: &[String]) {
    let file = full(ctx, arg(args, 0));
    let mut sec = ctx.expand(arg(args, 1));
    let mut key = ctx.expand(arg(args, 2));
    if key == NULL {
        key.clear();
    }
    let key = key.replace("*NULL*", "NULL");
    let mut val = ctx.expand(arg(args, 3));
    if val == NULL {
        val.clear();
    }
    let val = val.replace("*NULL*", "NULL");
    let mut doc = IniDoc::load(&file);
    let delete_section = sec.starts_with("--");
    if delete_section {
        sec = sec[2..].to_string();
    }
    if delete_section {
        if sec == NULL {
            doc.delete_global_keys();
        } else {
            doc.delete_section(&sec.replace("*NULL*", "NULL"));
        }
    } else {
        let sec = sec.replace("*NULL*", "NULL");
        let s = if sec == "NULL" && arg(args, 1) != "*NULL*" { None } else { Some(sec.as_str()) };
        if key.is_empty() {
            // nothing to write (the original also ignored this case)
        } else if val.is_empty() {
            doc.delete_key(s, &key);
        } else {
            doc.new_file_encoding(&format!("{}{}{}", sec, key, val));
            doc.set(s, &key, &val);
        }
    }
    let _ = doc.save(&file);
}

pub fn stringreplace(ctx: &mut Ctx, args: &[String]) {
    let text = ctx.expand(arg(args, 0));
    let find = ctx.expand(arg(args, 1));
    let mut rep = ctx.expand(arg(args, 2));
    let var = ctx.expand(arg(args, 3));
    if rep == NULL {
        rep.clear();
    }
    let rep = rep.replace("*NULL*", "NULL");
    let out = util::replace_all_ic(&text, &find, &rep);
    set_user_var(ctx, &var, &out);
}

pub fn killprocess(ctx: &mut Ctx, args: &[String]) {
    let p = ctx.expand(arg(args, 0));
    if !p.is_empty() {
        sys::kill_by_prefix(&p);
    }
}

pub fn message(ctx: &mut Ctx, args: &[String]) {
    let text = ctx.expand(arg(args, 0));
    if text.is_empty() {
        return;
    }
    if eq_ic(arg(args, 1), "cancellable") {
        if !ui::ok_cancel_topmost(&ctx.title, &text) {
            exit_now();
        }
    } else {
        ui::info_topmost(&ctx.title, &text);
    }
}

pub fn nowait(ctx: &mut Ctx, sec: Section, args: &[String]) {
    let v = if args.is_empty() { 0 } else { ctx.expand_num(&args[0]) };
    if v == 1 {
        match sec {
            Section::Before => {
                ctx.run_after = false;
                ctx.wait_main = false;
            }
            Section::After => exit_now(),
        }
    }
}

pub fn quit(ctx: &mut Ctx, args: &[String]) {
    let v = if args.is_empty() { 0 } else { ctx.expand_num(&args[0]) };
    if v == 1 {
        exit_now();
    }
}

pub fn regdll(ctx: &mut Ctx, args: &[String]) {
    // regsvr32 runs with the architecture of the application, so it registers the DLL
    // in the registry view of the application and resolves the path itself.
    let file = full_logical(ctx, arg(args, 0));
    let mut flag = ctx.expand(arg(args, 1));
    if flag == NULL || flag.is_empty() {
        flag = "register".into();
    }
    if !util::is_file(&ctx.io_path(&file)) {
        return;
    }
    let exe = sys::system32_for(ctx.app_arch, "regsvr32.exe");
    if eq_ic(&flag, "register") {
        sys::run_hidden_wait(&exe, &["/S", &file]);
    } else if eq_ic(&flag, "unregister") {
        sys::run_hidden_wait(&exe, &["/U", "/S", &file]);
    }
}

/// `run = App [:: wait] [:: parameters] [:: folder]`.
pub fn grun(ctx: &mut Ctx, args: &[String]) {
    let app = full(ctx, arg(args, 0));
    let wait = ctx.expand(arg(args, 1));
    let mut cl = ctx.expand(arg(args, 2));
    if cl == NULL {
        cl.clear();
    }
    let wd = ctx.expand(arg(args, 3));
    let wd = if wd == NULL || wd.is_empty() {
        ctx.script_dir.trim_end_matches('\\').to_string()
    } else {
        util::path_full(&wd, &ctx.script_dir)
    };
    if !util::exists(&app) {
        return;
    }
    let cmd = if cl.is_empty() { format!("\"{}\"", app) } else { format!("\"{}\" {}", app, cl) };
    if let Err(e) = sys::launch(&cmd, &wd, 5, eq_ic(&wait, "wait")) {
        ctx.msg(&format!("\"{}\" could not be started (error {}).", app, e));
    }
}

/// Separates the executable (which must exist) from the parameters in `binPath`.
fn service_exe_exists(spec: &str) -> bool {
    let s = spec.trim();
    if let Some(rest) = s.strip_prefix('"') {
        return rest.find('"').map(|i| util::exists(&rest[..i])).unwrap_or(false);
    }
    if util::exists(s) {
        return true;
    }
    let mut pos = 0;
    while let Some(i) = s[pos..].find(' ') {
        pos += i;
        if util::exists(&s[..pos]) {
            return true;
        }
        pos += 1;
    }
    false
}

pub fn service(ctx: &mut Ctx, args: &[String]) {
    let name = ctx.expand(arg(args, 0));
    let flag = ctx.expand(arg(args, 1));
    let file = ctx.expand(arg(args, 2));
    let mut typ = ctx.expand(arg(args, 3));
    if typ == NULL || typ.is_empty() {
        typ = "own".into();
    }
    let sc = sys::system32("sc.exe");
    let l = flag.to_lowercase();
    match l.as_str() {
        "create" if service_exe_exists(&file) => {
            let mut a: Vec<String> = vec!["create".into(), name, "binPath=".into(), file];
            match typ.to_lowercase().as_str() {
                "interact own" => a.extend(["type=".into(), "interact".into(), "type=".into(), "own".into()]),
                "interact share" => a.extend(["type=".into(), "interact".into(), "type=".into(), "share".into()]),
                t => a.extend(["type=".into(), t.to_string()]),
            }
            let refs: Vec<&str> = a.iter().map(|s| s.as_str()).collect();
            sys::run_hidden_wait(&sc, &refs);
        }
        "delete" | "start" | "stop" => {
            sys::run_hidden_wait(&sc, &[l.as_str(), &name]);
        }
        _ => {}
    }
}

pub fn symlink(ctx: &mut Ctx, args: &[String]) {
    let flag = if args.len() > 2 { args[2].clone() } else { "Create".to_string() };
    let link = full_logical(ctx, arg(args, 0));
    let target = full_logical(ctx, arg(args, 1));
    if eq_ic(&flag, "remove") {
        let is_dir = args.get(3).map(|s| s.trim() != "0").unwrap_or(true);
        remove_symlink(&link, is_dir);
        return;
    }
    if !eq_ic(&flag, "create") {
        return;
    }
    if !util::exists(&target) {
        return;
    }
    let ok_fs = |p: &str| {
        let f = sys::fs_name(p);
        util::contains_ic(&f, "NTFS") || util::contains_ic(&f, "ReFS")
    };
    if !ok_fs(&link) || !ok_fs(&target) {
        return;
    }
    let is_dir = util::is_dir(&target);
    let parent = util::parent_dir(&link);
    if !parent.is_empty() {
        let _ = std::fs::create_dir_all(&parent);
    }
    let created = unsafe {
        let flags = (if is_dir { 1 } else { 0 }) | 2; // SYMBOLIC_LINK_FLAG_ALLOW_UNPRIVILEGED_CREATE
        let mut ok = CreateSymbolicLinkW(util::w(&link).as_ptr(), util::w(&target).as_ptr(), flags);
        if !ok {
            ok = CreateSymbolicLinkW(util::w(&link).as_ptr(), util::w(&target).as_ptr(), flags & 1);
        }
        ok
    };
    if created {
        ctx.after_prog.push(Prog::SymlinkRemove { link, is_dir });
    } else if !sys::is_admin() {
        ctx.msg("The symbolic link could not be created. Administrator rights (or Windows Developer Mode) are required.");
    }
}

pub fn remove_symlink(link: &str, is_dir: bool) {
    unsafe {
        let w = util::w(link);
        if is_dir {
            RemoveDirectoryW(w.as_ptr());
        } else {
            DeleteFileW(w.as_ptr());
        }
    }
}

pub fn systemrefresh(ctx: &mut Ctx, args: &[String]) {
    let v = if args.is_empty() { 0 } else { ctx.expand_num(&args[0]) };
    if v == 1 {
        unsafe {
            windows_sys::Win32::UI::Shell::SHChangeNotify(0x0800_0000, 0x1000, null_mut(), null_mut());
        }
    }
}
