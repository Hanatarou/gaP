//! Miscellaneous keywords: clip, clean, delay, envvar, iniread, iniwrite,
//! killprocess, message, nowait, quit, regdll, run, service, stringreplace,
//! symlink, systemrefresh, uservar.

use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::Storage::FileSystem::{CreateSymbolicLinkW, DeleteFileW, RemoveDirectoryW, DELETE};
use windows_sys::Win32::System::Services::*;

use crate::ctx::{exit_code, exit_now, Ctx, Prog, Section, EXIT_CANCELLED, FUNC_MACROS, RESERVED_VARS};
use crate::ini::IniDoc;
use crate::{clean, sys, ui, util};
use util::{arg, eq_ic, is_null};

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
    let t = ctx.expand(arg(args, 0));
    let Ok(ms) = t.trim().parse::<i64>() else {
        ctx.warn(&format!("delay: \"{}\" is not a number of milliseconds; gaP did not wait", t.trim()));
        return;
    };
    std::thread::sleep(std::time::Duration::from_millis(ms.clamp(0, i32::MAX as i64) as u64));
}

/// The value of a switch that is 1 (on) or 0 (off), as in `quit = 1`. Anything else is reported and read as off.
fn switch_on(ctx: &mut Ctx, kw: &str, args: &[String]) -> bool {
    if args.is_empty() {
        return false;
    }
    let t = ctx.expand(&args[0]);
    match t.trim().parse::<i64>() {
        Ok(1) => true,
        Ok(0) => false,
        _ => {
            if !t.trim().is_empty() && !is_null(t.trim()) {
                ctx.warn(&format!("{}: \"{}\" is not valid, use 1 (or 0); it was ignored", kw, t.trim()));
            }
            false
        }
    }
}

pub fn envvar(ctx: &mut Ctx, args: &[String]) {
    let name = ctx.expand(arg(args, 0)).trim().to_string();
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
    let name = arg(args, 0).trim().to_string();
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
    // Optional 5th segment "expand": the value read is text of the gaP world and its macros are expanded once.
    // Without it the value is data from another file and is kept as it is.
    let flag = ctx.expand(arg(args, 4)).trim().to_string();
    let expand = eq_ic(&flag, "expand");
    if !flag.is_empty() && !is_null(&flag) && !expand {
        ctx.warn(&format!("giniread: \"{}\" is not valid, use expand; the value is kept as it is", flag));
    }
    if !util::is_file(&file) || ctx.refuse_big(&file) {
        return;
    }
    let doc = IniDoc::load(&file);
    let section = if is_null(&sec) { None } else { Some(sec.as_str()) };
    let val = match doc.get(section, &key) {
        Some(v) if expand => ctx.expand(&v),
        Some(v) => v,
        None => "NOT_FOUND!".to_string(),
    };
    set_user_var(ctx, &var, &val);
}

pub fn giniwrite(ctx: &mut Ctx, args: &[String]) {
    let file = full(ctx, arg(args, 0));
    let mut sec = ctx.expand(arg(args, 1)).trim().to_string();
    let mut key = ctx.expand(arg(args, 2));
    if is_null(&key) {
        key.clear();
    }
    let key = key.replace("*NULL*", "NULL");
    let mut val = ctx.expand(arg(args, 3));
    if is_null(&val) {
        val.clear();
    }
    let val = val.replace("*NULL*", "NULL");
    if ctx.refuse_big(&file) {
        return;
    }
    let mut doc = IniDoc::load(&file);
    let delete_section = sec.starts_with("--");
    if delete_section {
        sec = sec[2..].trim().to_string();
    }
    if delete_section {
        if is_null(&sec) {
            doc.delete_global_keys();
        } else {
            doc.delete_section(&sec.replace("*NULL*", "NULL"));
        }
    } else {
        let sec = sec.replace("*NULL*", "NULL");
        let s = if is_null(&sec) && arg(args, 1) != "*NULL*" { None } else { Some(sec.as_str()) };
        if key.is_empty() {
            // nothing to write (the original also ignored this case)
        } else if val.is_empty() {
            doc.delete_key(s, &key);
        } else {
            doc.new_file_encoding(&format!("{}{}{}", sec, key, val));
            doc.set(s, &key, &val);
        }
    }
    match doc.save(&file) {
        Ok(()) => ctx.ok(&format!("ini write \"{}\"", file)),
        Err(e) => ctx.log_err(&format!("write \"{}\"", file), &e),
    }
}

pub fn stringreplace(ctx: &mut Ctx, args: &[String]) {
    let text = ctx.expand(arg(args, 0));
    let find = ctx.expand(arg(args, 1));
    let mut rep = ctx.expand(arg(args, 2));
    let var = ctx.expand(arg(args, 3)).trim().to_string();
    if is_null(&rep) {
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
        ctx.log(&format!("kill processes whose name starts with \"{}\"", p));
    }
}

pub fn message(ctx: &mut Ctx, args: &[String]) {
    let text = ctx.expand(arg(args, 0));
    if text.is_empty() {
        return;
    }
    let flag = arg(args, 1).trim();
    if !flag.is_empty() && !is_null(flag) && !eq_ic(flag, "cancellable") {
        ctx.warn(&format!("message: \"{}\" is not valid, use cancellable; a plain message was shown", flag));
    }
    if eq_ic(flag, "cancellable") {
        if !ui::ok_cancel_topmost(&ctx.title, &text) {
            exit_code(EXIT_CANCELLED);
        }
    } else {
        ui::info_topmost(&ctx.title, &text);
    }
}

pub fn nowait(ctx: &mut Ctx, sec: Section, args: &[String]) {
    if switch_on(ctx, "nowait", args) {
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
    if switch_on(ctx, "quit", args) {
        exit_now();
    }
}

pub fn regdll(ctx: &mut Ctx, args: &[String]) {
    // regsvr32 runs with the architecture of the application, so it registers the DLL
    // in the registry view of the application and resolves the path itself.
    let file = full_logical(ctx, arg(args, 0));
    let mut flag = ctx.expand(arg(args, 1)).trim().to_string();
    if is_null(&flag) || flag.is_empty() {
        flag = "register".into();
    }
    if !util::is_file(&ctx.io_path(&file)) {
        ctx.warn(&format!("regdll \"{}\": the file does not exist, nothing to do", file));
        return;
    }
    let exe = sys::system32_for(ctx.app_arch, "regsvr32.exe");
    let ok = if eq_ic(&flag, "register") {
        sys::run_hidden_wait(&exe, &["/S", &file])
    } else if eq_ic(&flag, "unregister") {
        sys::run_hidden_wait(&exe, &["/U", "/S", &file])
    } else {
        ctx.warn(&format!("regdll \"{}\": \"{}\" is not valid, use register or unregister; nothing was done", file, flag));
        return;
    };
    if ok {
        ctx.ok(&format!("regsvr32 {} \"{}\"", flag, file));
    } else {
        ctx.err(&format!("regsvr32 {} \"{}\" failed", flag, file));
    }
}

/// `run = App [:: wait] [:: parameters] [:: folder]`.
pub fn grun(ctx: &mut Ctx, args: &[String]) {
    let app = full(ctx, arg(args, 0));
    let wait = ctx.expand(arg(args, 1)).trim().to_string();
    if !wait.is_empty() && !is_null(&wait) && !eq_ic(&wait, "wait") {
        ctx.warn(&format!("run: \"{}\" is not valid, use wait; gaP does not wait for the application", wait));
    }
    let mut cl = ctx.expand(arg(args, 2));
    if is_null(&cl) {
        cl.clear();
    }
    let wd = ctx.expand(arg(args, 3));
    let wd = if is_null(&wd) || wd.is_empty() {
        ctx.script_dir.trim_end_matches('\\').to_string()
    } else {
        util::path_full(&wd, &ctx.script_dir)
    };
    if !util::exists(&app) {
        ctx.warn(&format!("run \"{}\": the file does not exist, nothing to run", app));
        return;
    }
    let cmd = if cl.is_empty() { format!("\"{}\"", app) } else { format!("\"{}\" {}", app, cl) };
    match sys::launch(&cmd, &wd, 5, eq_ic(&wait, "wait")) {
        Ok(()) => ctx.ok(&format!("run {}{}", cmd, if eq_ic(&wait, "wait") { " (waited)" } else { "" })),
        Err(e) => {
            ctx.err(&format!("run {} failed (error {})", cmd, e));
            ctx.msg(&format!("\"{}\" could not be started (error {}).", app, e));
        }
    }
}

/// Separates the executable (which must exist) from the parameters in `binPath`.
fn service_exe_exists(spec: &str) -> bool {
    let s = spec.trim();
    if let Some(rest) = s.strip_prefix('"') {
        return rest.find('"').map(|i| util::is_file(&rest[..i])).unwrap_or(false);
    }
    if util::is_file(s) {
        return true;
    }
    let mut pos = 0;
    while let Some(i) = s[pos..].find(' ') {
        pos += i;
        if util::is_file(&s[..pos]) {
            return true;
        }
        pos += 1;
    }
    false
}

/// A service of the type that can show windows on the desktop (`type= interact`).
const SERVICE_INTERACTIVE_PROCESS: u32 = 0x100;

/// `type=` of `sc create` as the service type of the Service Control Manager.
fn service_type(name: &str) -> Option<u32> {
    Some(match name.to_lowercase().as_str() {
        "own" => SERVICE_WIN32_OWN_PROCESS,
        "share" => SERVICE_WIN32_SHARE_PROCESS,
        "interact own" => SERVICE_WIN32_OWN_PROCESS | SERVICE_INTERACTIVE_PROCESS,
        "interact share" => SERVICE_WIN32_SHARE_PROCESS | SERVICE_INTERACTIVE_PROCESS,
        "userown" => SERVICE_USER_OWN_PROCESS,
        "usershare" => SERVICE_USER_SHARE_PROCESS,
        "kernel" => SERVICE_KERNEL_DRIVER,
        "filesys" => SERVICE_FILE_SYSTEM_DRIVER,
        "rec" => SERVICE_RECOGNIZER_DRIVER,
        "adapt" => SERVICE_ADAPTER,
        _ => return None,
    })
}

/// A handle of the Service Control Manager or of a service, closed when it goes out of scope.
struct Sc(SC_HANDLE);
impl Drop for Sc {
    fn drop(&mut self) {
        unsafe {
            CloseServiceHandle(self.0);
        }
    }
}

/// Service Control Manager call; the error is the system code.
fn scm_do(name: &str, flag: &str, file: &str, typ: &str) -> Result<(), u32> {
    unsafe {
        let scm_access = if flag == "create" { SC_MANAGER_CREATE_SERVICE } else { SC_MANAGER_CONNECT };
        let scm = OpenSCManagerW(null(), null(), scm_access);
        if scm.is_null() {
            return Err(GetLastError());
        }
        let scm = Sc(scm);
        let wname = util::w(name);
        let open = |access: u32| -> Result<Sc, u32> {
            let h = OpenServiceW(scm.0, wname.as_ptr(), access);
            if h.is_null() {
                Err(GetLastError())
            } else {
                Ok(Sc(h))
            }
        };
        match flag {
            "create" => {
                let kind = service_type(typ).ok_or(87u32)?; // ERROR_INVALID_PARAMETER
                let wfile = util::w(file);
                // like `sc create`: started by hand, normal error control, display name = name
                let h = CreateServiceW(
                    scm.0, wname.as_ptr(), wname.as_ptr(), SERVICE_ALL_ACCESS, kind, SERVICE_DEMAND_START,
                    SERVICE_ERROR_NORMAL, wfile.as_ptr(), null(), null_mut(), null(), null(), null(),
                );
                if h.is_null() {
                    return Err(GetLastError());
                }
                drop(Sc(h));
                Ok(())
            }
            "delete" => {
                let svc = open(DELETE)?;
                if DeleteService(svc.0) == 0 {
                    return Err(GetLastError());
                }
                Ok(())
            }
            "start" => {
                let svc = open(SERVICE_START)?;
                if StartServiceW(svc.0, 0, null()) == 0 {
                    return Err(GetLastError());
                }
                Ok(())
            }
            "stop" => {
                let svc = open(SERVICE_STOP)?;
                let mut status: SERVICE_STATUS = std::mem::zeroed();
                if ControlService(svc.0, SERVICE_CONTROL_STOP, &mut status) == 0 {
                    return Err(GetLastError());
                }
                Ok(())
            }
            // ERROR_INVALID_PARAMETER: an action that is not handled above is an error, not a success
            _ => Err(87),
        }
    }
}

pub fn service(ctx: &mut Ctx, args: &[String]) {
    let name = ctx.expand(arg(args, 0));
    let flag = ctx.expand(arg(args, 1)).trim().to_lowercase();
    let file = ctx.expand(arg(args, 2));
    let mut typ = ctx.expand(arg(args, 3));
    if is_null(&typ) || typ.is_empty() {
        typ = "own".into();
    }
    if !matches!(flag.as_str(), "create" | "delete" | "start" | "stop") {
        ctx.warn(&format!("service \"{}\": \"{}\" is not valid, use create, delete, start or stop; nothing was done", name, flag));
        return;
    }
    if flag == "create" && !service_exe_exists(&file) {
        ctx.warn(&format!("service \"{}\": the file \"{}\" does not exist, the service was not created", name, file));
        return;
    }
    if flag == "create" && service_type(&typ).is_none() {
        ctx.warn(&format!("service \"{}\": the type \"{}\" is not valid, the service was not created", name, typ));
        return;
    }
    if let Err(code) = scm_do(&name, &flag, &file, &typ) {
        ctx.log_err(&format!("service \"{}\" {}", name, flag), &format!("error {}", code));
    }
}

pub fn symlink(ctx: &mut Ctx, args: &[String]) {
    let flag = if args.len() > 2 { ctx.expand(&args[2]).trim().to_string() } else { "Create".to_string() };
    let link = full_logical(ctx, arg(args, 0));
    let target = full_logical(ctx, arg(args, 1));
    if eq_ic(&flag, "remove") {
        let kind = if args.len() > 3 { ctx.expand(&args[3]) } else { String::new() };
        let is_dir = kind.trim() != "0";
        remove_symlink(&link, is_dir);
        return;
    }
    if !eq_ic(&flag, "create") {
        ctx.warn(&format!("symlink \"{}\": \"{}\" is not valid, use create or remove; nothing was done", link, flag));
        return;
    }
    if !util::exists(&target) {
        ctx.warn(&format!("symlink \"{}\": the target \"{}\" does not exist, no link was created", link, target));
        return;
    }
    let ok_fs = |p: &str| {
        let f = sys::fs_name(p);
        util::contains_ic(&f, "NTFS") || util::contains_ic(&f, "ReFS")
    };
    if !ok_fs(&link) || !ok_fs(&target) {
        ctx.warn(&format!(
            "symlink \"{}\": links need a local NTFS or ReFS drive for the link and the target (a network path is not supported); no link was created",
            link
        ));
        return;
    }
    let is_dir = util::is_dir(&target);
    // A link left by a run that did not finish is replaced; anything else that is there is left alone.
    match std::fs::symlink_metadata(&link) {
        Ok(md) if md.file_type().is_symlink() => {
            use std::os::windows::fs::MetadataExt;
            // a link to a folder carries the "directory" attribute
            remove_symlink(&link, md.file_attributes() & 0x10 != 0)
        }
        Ok(_) => {
            ctx.msg(&format!("The symbolic link \"{}\" could not be created: there is a file or folder with that name.", link));
            return;
        }
        Err(_) => {}
    }
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
    } else {
        let code = unsafe { GetLastError() };
        ctx.log_err(&format!("create symbolic link \"{}\"", link), &format!("error {}", code));
        if !sys::is_admin() {
            ctx.msg("The symbolic link could not be created. Administrator rights (or Windows Developer Mode) are required.");
        } else {
            ctx.msg(&format!("The symbolic link \"{}\" could not be created (error {}).", link, code));
        }
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
    if switch_on(ctx, "systemrefresh", args) {
        unsafe {
            windows_sys::Win32::UI::Shell::SHChangeNotify(0x0800_0000, 0x1000, null_mut(), null_mut());
        }
    }
}
