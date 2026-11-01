//! [GENERAL] section: main application, command line, working directory
//! and initial checks (read-only, single instance, administrator,
//! .NET, Java, Internet, OS, debugging).

use std::path::Path;

use crate::clean;
use crate::ctx::{exit_now, Ctx};
use crate::reg::{self, View};
use crate::{crypto, ini, sys, ui, util};
use util::{auto_num, eq_ic};

/// IniRead(GENERAL, key, default) with the macros already expanded.
fn general(ctx: &mut Ctx, key: &str, default: &str) -> String {
    let raw = ini::read(&ctx.ini_path.clone(), "GENERAL", key, default);
    ctx.expand(&raw)
}

fn quit_clean(ctx: &Ctx) -> ! {
    clean::clean_all(&ctx.script_full);
    exit_now()
}

// ---------------------------------------------------------------------- read-only

pub fn read_only_init(ctx: &mut Ctx) {
    let probe = format!("{}\\@#$readonly@#$", ctx.script_dir.trim_end_matches('\\'));
    let opened = std::fs::OpenOptions::new().create(true).append(true).open(&probe);
    match opened {
        Ok(f) => {
            drop(f);
            let _ = std::fs::remove_file(&probe);
            ctx.is_readonly = false;
        }
        Err(_) => {
            ctx.is_readonly = true;
            let v = auto_num(&general(ctx, "!readonly", "1"));
            let exe = format!("{}.exe", ctx.exe_name);
            let text = format!(
                "\"{0}\" is running from a read only drive. Some applications may not work as expected in this mode. \"{0}\" will not be able to update configuration changes in files stored in a read only drive.",
                exe
            );
            match v {
                -2 => quit_clean(ctx),
                -1 => {
                    ctx.msg(&text);
                    quit_clean(ctx);
                }
                1 => ctx.msg(&text),
                _ => {}
            }
        }
    }
}

// ----------------------------------------------------------------------- Java

pub struct JavaInfo {
    /// Installed major versions ("6", "7", "8", "17"…).
    pub majors: Vec<u32>,
    pub exe: Option<String>,
}

/// "1.8.0_201" -> 8, "17.0.2" -> 17, "1.6" -> 6.
pub fn java_major(v: &str) -> Option<u32> {
    let t: String = v.trim().chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    let mut p = t.split('.').filter(|s| !s.is_empty()).map(|s| s.parse::<u32>().ok());
    let a = p.next()??;
    if a == 1 {
        p.next().flatten().or(Some(1))
    } else {
        Some(a)
    }
}

pub fn java_info() -> JavaInfo {
    let views: Vec<View> = if sys::os_arch() == "x64" { vec![View::X64, View::X86] } else { vec![View::Native] };
    let roots = [
        "HKLM\\SOFTWARE\\JavaSoft\\Java Runtime Environment",
        "HKLM\\SOFTWARE\\JavaSoft\\JRE",
        "HKLM\\SOFTWARE\\JavaSoft\\JDK",
        "HKLM\\SOFTWARE\\JavaSoft\\Java Development Kit",
    ];
    let mut majors: Vec<u32> = Vec::new();
    let mut exe: Option<String> = None;
    let add = |m: Option<u32>, majors: &mut Vec<u32>| {
        if let Some(m) = m {
            if !majors.contains(&m) {
                majors.push(m);
            }
        }
    };
    let try_exe = |home: &str| -> Option<String> {
        for rel in ["bin\\javaw.exe", "javaw.exe"] {
            let p = format!("{}\\{}", home.trim_end_matches('\\'), rel);
            if Path::new(&p).is_file() {
                return Some(p);
            }
        }
        None
    };
    for v in &views {
        for root in roots {
            let cur = reg::read_string(root, "CurrentVersion", *v);
            if let Some(c) = &cur {
                add(java_major(c), &mut majors);
            }
            for name in ["Java6FamilyVersion", "Java7FamilyVersion"] {
                if let Some(s) = reg::read_string(root, name, *v) {
                    add(java_major(&s), &mut majors);
                }
            }
            for sub in reg::enum_subkeys(root, *v) {
                add(java_major(&sub), &mut majors);
            }
            if exe.is_none() {
                let mut cands: Vec<String> = Vec::new();
                if let Some(c) = &cur {
                    cands.push(c.clone());
                }
                cands.extend(reg::enum_subkeys(root, *v));
                for c in cands {
                    if let Some(h) = reg::read_string(&format!("{}\\{}", root, c), "JavaHome", *v) {
                        if let Some(e) = try_exe(&h) {
                            exe = Some(e);
                            break;
                        }
                    }
                }
            }
        }
    }
    if exe.is_none() {
        if let Ok(h) = std::env::var("JAVA_HOME") {
            exe = try_exe(&h);
        }
    }
    JavaInfo { majors, exe }
}

pub fn java_early(ctx: &mut Ctx) {
    let info = java_info();
    let exe = info.exe.unwrap_or_else(|| "There is no Java Runtime Environment installed.".to_string());
    ctx.add_var("JAVAEXE", &exe);
}

pub fn java_full(ctx: &mut Ctx) {
    let req = general(ctx, "!java", "");
    if req.is_empty() {
        return;
    }
    let exit = req.contains('!');
    let req = req.replace('!', "");
    let info = java_info();
    let ok = java_major(&req).map(|m| info.majors.contains(&m)).unwrap_or(false);
    if !ok {
        ctx.msg(&format!("The main application requires Java Runtime Environment version {} to run.", req));
        if exit {
            exit_now();
        }
    }
}

// --------------------------------------------------------------- main application

pub fn main_app_init(ctx: &mut Ctx) {
    let app = general(ctx, "application", "");
    if app.is_empty() || app == "NULL" {
        ctx.msg("You MUST define a main application path in the \"GENERAL\" section.");
        exit_now();
    }
    let segs = util::split_args(&app);
    if segs.len() > 3 {
        ctx.msg("Check the main application path input data.");
        exit_now();
    }
    ctx.show_state = 5; // SW_SHOW
    if segs.len() == 3 {
        let s = segs[2].as_str();
        if eq_ic(s, "hide") {
            ctx.show_state = 0;
        } else if eq_ic(s, "min") {
            ctx.show_state = 6;
        } else if eq_ic(s, "max") {
            ctx.show_state = 3;
        }
    }
    let pick = |ctx: &Ctx, s: &str| util::path_full(s, &ctx.script_dir);
    let chosen;
    if ctx.os_arch == "x64" && segs.len() >= 2 {
        let p64 = pick(ctx, &segs[1]);
        if util::exists(&p64) {
            chosen = p64;
        } else {
            let p86 = pick(ctx, &segs[0]);
            if util::exists(&p86) {
                chosen = p86;
            } else {
                ctx.msg("The main application (x86) path and (x64) path are invalid.");
                exit_now();
            }
        }
    } else {
        let p86 = pick(ctx, &segs[0]);
        if util::exists(&p86) {
            chosen = p86;
        } else {
            ctx.msg("The main application (x86) path is invalid.");
            exit_now();
        }
    }
    let exepath = chosen.rfind('\\').map(|i| chosen[..i].to_string()).unwrap_or_default();
    ctx.add_var("APPEXE", &chosen);
    ctx.add_var("EXEPATH", &exepath);
    clean::set_main_app(&chosen);
    ctx.main_app = Some(chosen);
}

pub fn cmdline_init(ctx: &mut Ctx) {
    ctx.main_cmdline = general(ctx, "commandline", "");
    let c = ctx.main_cmdline.clone();
    ctx.add_var("COMMANDLINE", &c);
}

pub fn workdir_init(ctx: &mut Ctx) {
    let wd = general(ctx, "workdir", "");
    let dir = if wd.is_empty() {
        ctx.get_var("EXEPATH").unwrap_or("").to_string()
    } else {
        let full = util::path_full(&wd, &ctx.script_dir);
        if !util::exists(&full) {
            ctx.msg("The main application working directory path is invalid.");
            exit_now();
        }
        full
    };
    ctx.main_workdir = dir.clone();
    ctx.add_var("WORKDIR", &dir);
}

// ---------------------------------------------------------- single instance

pub fn gapnew_paths(ctx: &Ctx, pid: u32) -> (String, String) {
    let local = format!("{}\\@#$gapnew@#$", ctx.script_dir.trim_end_matches('\\'));
    let dir = format!("{}{}_{}_{}", ctx.common_appdata, ctx.exe_name, pid, ctx.file_hash);
    (local, format!("{}\\@#$gapnew@#$", dir))
}

fn send_new_cmdline(ctx: &Ctx, owner_pid: u32) {
    let (local, shared) = gapnew_paths(ctx, owner_pid);
    let target = if ctx.is_readonly { shared } else { local };
    if let Some(par) = Path::new(&target).parent() {
        let _ = std::fs::create_dir_all(par);
    }
    let tmp = format!("{}.tmp", target);
    if std::fs::write(&tmp, ctx.cmd_raw.as_bytes()).is_ok() {
        let _ = std::fs::remove_file(&target);
        let _ = std::fs::rename(&tmp, &target);
    }
}

pub fn multiple_init(ctx: &mut Ctx) {
    let multiple = auto_num(&general(ctx, "!multiple", "0"));
    let code = format!("@{}@{}@", crypto::sha1_hex(&util::string_to_ansi(&ctx.script_full)), ctx.file_hash);
    let owner = sys::singleton_pid(&code);
    if owner == 0 {
        return;
    }
    ctx.first_instance = false;
    match multiple {
        -1 => exit_now(),
        1 => {
            send_new_cmdline(ctx, owner);
            exit_now();
        }
        0 => {
            let q = format!(
                "\"{}.exe\" is already running. Are you sure to launch another instance of the main application?",
                ctx.exe_name
            );
            if ui::ok_cancel(&ctx.title, &q) {
                send_new_cmdline(ctx, owner);
            }
            exit_now();
        }
        _ => {}
    }
}

// ------------------------------------------------------------------ debugging

pub fn debug_log_init(ctx: &mut Ctx) {
    let mut on = auto_num(&general(ctx, "!debug", "0")) == 1;
    if !ctx.first_instance {
        on = false;
    }
    if !on {
        return;
    }
    let t = util::local_now();
    let stamp = format!("{:04}.{:02}.{:02}.{:02}.{:02}.{:02}", t.year, t.month, t.day, t.hour, t.min, t.sec);
    let path = if !ctx.is_readonly {
        let d = format!("{}\\Logs", ctx.script_dir.trim_end_matches('\\'));
        let _ = std::fs::create_dir_all(&d);
        format!("{}\\{}_{}.log", d, ctx.exe_name, stamp)
    } else {
        let d = format!("{}gaPLogs", ctx.common_appdata);
        let _ = std::fs::create_dir_all(&d);
        format!("{}\\{}_{}_{}.log", d, ctx.exe_name, std::process::id(), stamp)
    };
    let lg = crate::ctx::Logger { path };
    let pad = |s: String| format!("{:<57}#", s);
    lg.write("######################gaP Log Header######################");
    lg.write(&pad(format!(
        "#  Date(DD/MM/YYYY - HH:MM:SS): {:02}/{:02}/{:04} - {:02}:{:02}:{:02}",
        t.day, t.month, t.year, t.hour, t.min, t.sec
    )));
    lg.write(&pad(format!("#  gaP Version: {}", crate::ctx::PROGRAM_VERSION)));
    lg.write(&pad(format!("#  Host OS: {}", sys::os_name())));
    lg.write(&pad(format!("#  Host Architecture: {}", ctx.os_arch)));
    lg.write("######################gaP Log Header######################");
    lg.write("");
    lg.write("[Shell Folders]");
    for (n, v) in &ctx.shell {
        lg.write(&format!("{}|{}", n, v));
    }
    lg.write("");
    ctx.debug = Some(lg);
}

// -------------------------------------------------------------- administrator

pub fn admin_init(ctx: &mut Ctx) {
    let v = general(ctx, "!admin", "");
    if v.is_empty() || sys::is_admin() {
        return;
    }
    let exe = format!("\"{}.exe\"", ctx.exe_name);
    match auto_num(&v) {
        -1 => exit_now(),
        1 => {
            // Windows default: missing EnableLUA = UAC on.
            let lua = reg::read_dword(
                "HKLM\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Policies\\System",
                "EnableLUA",
                View::Native,
            );
            if sys::os_version().0 >= 6 && lua == Some(0) {
                ctx.msg(&format!(
                    "{exe} could not be launched with administrator privileges. The main cause is explained below. Please, check it and try again.\r\n\r\nSystem administrator disabled UAC (User Account Control) for all users. When this is done UAC dialogs will not be triggered and standard users will not be able to launch {exe} with administrator privileges."
                ));
                exit_now();
            }
            if !sys::relaunch_elevated(&ctx.script_full, &ctx.cmd_raw, &ctx.script_dir) {
                ctx.msg(&format!(
                    "{exe} could not be launched with administrator privileges. Some possible causes are listed below. Please, check them and try again.\r\n\r\n- User cancelled the \"Run As\" dialog;\r\n- User cancelled the UAC (User Account Control) dialog;\r\n- User filled the \"User name\" or \"Password\" fields with wrong data; and\r\n- System administrator disabled UAC for all users. When this is done UAC dialogs will not be triggered and standard users will not be able to launch {exe} with administrator privileges."
                ));
            }
            exit_now();
        }
        2 => {
            ctx.msg("The main application requires administrator privileges to run.");
            exit_now();
        }
        _ => {}
    }
}

// ----------------------------------------------------------------------- .NET

fn ver_parts(v: &str) -> Vec<u32> {
    v.split('.').map(|p| p.trim().parse::<u32>().unwrap_or(0)).collect()
}

fn ver_ge(a: &str, b: &str) -> bool {
    let (x, y) = (ver_parts(a), ver_parts(b));
    for i in 0..x.len().max(y.len()) {
        let (p, q) = (x.get(i).copied().unwrap_or(0), y.get(i).copied().unwrap_or(0));
        if p != q {
            return p > q;
        }
    }
    true
}

const NET_RELEASES: &[(u32, &str)] = &[
    (533320, "4.8.1"), (528040, "4.8"), (461808, "4.7.2"), (461308, "4.7.1"), (460798, "4.7"),
    (394802, "4.6.2"), (394254, "4.6.1"), (393295, "4.6"), (379893, "4.5.2"), (378675, "4.5.1"), (378389, "4.5"),
];

pub fn dotnet_versions() -> Vec<String> {
    let ndp = "HKLM\\SOFTWARE\\Microsoft\\NET Framework Setup\\NDP";
    let view = if sys::os_arch() == "x64" { View::X64 } else { View::Native };
    let mut out: Vec<String> = Vec::new();
    for k in reg::enum_subkeys(ndp, view) {
        // "v3.5" / "v2.0.50727" -> "3.5" / "2.0"
        let digits: String = k.trim_start_matches(['v', 'V']).chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
        let mut it = digits.split('.');
        if let (Some(a), Some(b)) = (it.next(), it.next()) {
            if !a.is_empty() && !b.is_empty() {
                out.push(format!("{}.{}", a, b));
            }
        }
    }
    if let Some(rel) = reg::read_dword(&format!("{}\\v4\\Full", ndp), "Release", view) {
        for (min, name) in NET_RELEASES {
            if rel >= *min {
                out.push(name.to_string());
            }
        }
    }
    out
}

pub fn dotnet_init(ctx: &mut Ctx) {
    let req = general(ctx, "!dotnet", "");
    if req.is_empty() {
        return;
    }
    let exit = req.contains('!');
    let req = req.replace('!', "");
    let inst = dotnet_versions();
    let ok = inst.iter().any(|v| v == &req) || (req.starts_with("4.") && inst.iter().any(|v| v.starts_with("4.") && ver_ge(v, &req)));
    if !ok {
        ctx.msg(&format!("The main application requires .NET Framework version {} to run.", req));
        if exit {
            exit_now();
        }
    }
}

// ------------------------------------------------------------------- Internet

pub fn is_online() -> bool {
    let local = sys::ping("127.0.0.1", 4000);
    if !local {
        return false;
    }
    if sys::ping("www.google.com", 4000) || sys::ping("www.microsoft.com", 4000) {
        return true;
    }
    // Networks that block ICMP: try a plain TCP connection.
    sys::tcp_reachable("www.microsoft.com", 443, 4000) || sys::tcp_reachable("www.google.com", 443, 4000)
}

pub fn online_init(ctx: &mut Ctx) {
    let v = general(ctx, "!online", "");
    if v.is_empty() {
        return;
    }
    let mode = auto_num(&v);
    let online = is_online();
    let t = ctx.title.clone();
    match (online, mode) {
        (false, 1) => {
            if !ui::ok_cancel(&t, "The main application requires an active Internet connection to run. Continue anyway?") {
                exit_now();
            }
        }
        (false, 2) => {
            ctx.msg("The main application requires an active Internet connection to run.");
            exit_now();
        }
        (false, -1) => exit_now(),
        (true, -2) => {
            if !ui::ok_cancel(&t, "The main application requires an inactive Internet connection to run. Continue anyway?") {
                exit_now();
            }
        }
        (true, -3) => {
            ctx.msg("The main application requires an inactive Internet connection to run.");
            exit_now();
        }
        (true, -4) => exit_now(),
        _ => {}
    }
}

// ------------------------------------------------------------------------ OS

pub fn os_init(ctx: &mut Ctx) {
    let os = general(ctx, "!os", "");
    if os.is_empty() {
        return;
    }
    let (ma, mi, _) = sys::os_version();
    let cur = (ma, mi);
    let (older, name) = match os.strip_prefix('<') {
        Some(n) => (true, n.to_string()),
        None => (false, os.clone()),
    };
    let thr = match name.to_lowercase().as_str() {
        "xp" => (5, 1),
        "vista" => (6, 0),
        "7" => (6, 1),
        "8" => (6, 2),
        "8.1" => (6, 3),
        _ => return,
    };
    let pretty = match name.to_lowercase().as_str() {
        "xp" => "XP",
        "vista" => "Vista",
        n => n,
    }
    .to_string();
    if !older && cur < thr {
        ctx.msg(&format!("The main application requires a host Operating System equal Windows {} or newer.", pretty));
        exit_now();
    }
    if older && cur > thr {
        ctx.msg(&format!("The main application requires a host Operating System equal Windows {} or older.", pretty));
        exit_now();
    }
}

pub fn wait_init(ctx: &mut Ctx) {
    ctx.waitprocess = general(ctx, "!waitprocess", "");
}
