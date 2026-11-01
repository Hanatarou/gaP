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

/// Registry views where the Java Runtime Environments are looked for.
fn all_views() -> Vec<View> {
    if sys::os_arch() == "x64" {
        vec![View::X64, View::X86]
    } else {
        vec![View::Native]
    }
}

/// The registry view of the main application: a 32-bit application uses the 32-bit
/// Java/.NET, a 64-bit one the 64-bit Java/.NET.
fn app_view(ctx: &Ctx) -> View {
    if ctx.os_arch != "x64" {
        View::Native
    } else if ctx.app_arch == "x64" {
        View::X64
    } else {
        View::X86
    }
}

pub fn java_info(views: &[View]) -> JavaInfo {
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
    for v in views {
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
    // Runs before the main application is chosen (`{JAVAEXE}` can be used in `application`).
    let info = java_info(&all_views());
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
    let info = java_info(&[app_view(ctx)]);
    let ok = java_major(&req).map(|m| info.majors.contains(&m)).unwrap_or(false);
    if !ok {
        ctx.msg(&format!("The main application requires Java Runtime Environment version {} to run.", req));
        if exit {
            exit_now();
        }
    }
}

// --------------------------------------------------------------- main application

/// What `application` leads to: the index of the chosen segment (0 = x86, 1 = x64) or why
/// none could be chosen.
#[derive(Debug, PartialEq)]
enum AppChoice {
    Chosen(usize),
    /// Only the x86 segment was tried (32-bit OS, or a single segment) and it does not exist.
    NoX86,
    /// Both segments were tried (64-bit OS) and none exists.
    NoneValid,
}

/// Architecture of the application of a segment: the 2nd one is x64, the other is x86.
fn segment_arch(index: usize) -> &'static str {
    if index == 1 {
        "x64"
    } else {
        "x86"
    }
}

/// Chooses the segment of `application`: on a 64-bit OS the 2nd (x64) one if it exists,
/// otherwise the 1st (x86) one; on a 32-bit OS, or with a single segment, always the 1st.
/// `exists(i)` tells whether the application of segment `i` exists.
fn choose_segment(os_arch: &str, segments: usize, exists: impl Fn(usize) -> bool) -> AppChoice {
    if os_arch == "x64" && segments >= 2 {
        if exists(1) {
            AppChoice::Chosen(1)
        } else if exists(0) {
            AppChoice::Chosen(0)
        } else {
            AppChoice::NoneValid
        }
    } else if exists(0) {
        AppChoice::Chosen(0)
    } else {
        AppChoice::NoX86
    }
}

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
    // The 1st segment is the x86 application and the 2nd one is the x64 application, so the
    // architecture of the application is the architecture of the segment that was chosen.
    // Existence is checked as that application sees the path (System32 vs SysWOW64).
    let exists = |i: usize| util::exists(&sys::io_path(segment_arch(i), &pick(ctx, &segs[i])));
    let index = match choose_segment(ctx.os_arch, segs.len(), exists) {
        AppChoice::Chosen(i) => i,
        AppChoice::NoX86 => {
            ctx.msg("The main application (x86) path is invalid.");
            exit_now();
        }
        AppChoice::NoneValid => {
            ctx.msg("The main application (x86) path and (x64) path are invalid.");
            exit_now();
        }
    };
    let chosen = pick(ctx, &segs[index]);
    let app_arch = segment_arch(index);
    let exepath = chosen.rfind('\\').map(|i| chosen[..i].to_string()).unwrap_or_default();
    ctx.app_arch = app_arch;
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

pub fn dotnet_versions(view: View) -> Vec<String> {
    let ndp = "HKLM\\SOFTWARE\\Microsoft\\NET Framework Setup\\NDP";
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
    let inst = dotnet_versions(app_view(ctx));
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

/// Windows version as compared by `!os`. Windows 11 is still "10.0": it is told from
/// Windows 10 by its build number (22000 or newer).
fn os_level(major: u32, minor: u32, build: u32) -> (u32, u32, u32) {
    (major, minor, u32::from(major == 10 && minor == 0 && build >= 22000))
}

/// Value of `!os` (XP, Vista, 7, 8, 8.1, 10, 11) as an `os_level`.
fn os_threshold(name: &str) -> Option<(u32, u32, u32)> {
    match name.to_lowercase().as_str() {
        "xp" => Some((5, 1, 0)),
        "vista" => Some((6, 0, 0)),
        "7" => Some((6, 1, 0)),
        "8" => Some((6, 2, 0)),
        "8.1" => Some((6, 3, 0)),
        "10" => Some((10, 0, 0)),
        "11" => Some((10, 0, 1)),
        _ => None,
    }
}

pub fn os_init(ctx: &mut Ctx) {
    let os = general(ctx, "!os", "");
    if os.is_empty() {
        return;
    }
    let (ma, mi, build) = sys::os_version();
    let cur = os_level(ma, mi, build);
    let (older, name) = match os.strip_prefix('<') {
        Some(n) => (true, n.to_string()),
        None => (false, os.clone()),
    };
    let Some(thr) = os_threshold(&name) else {
        // NEW (gaP-rs): the original ignored a value it did not know without saying anything.
        ctx.msg(&format!(
            "The \"{}\" value of \"!os\" isn't valid. Use XP, Vista, 7, 8, 8.1, 10 or 11 (with \"<\" before it for the maximum version).",
            os
        ));
        return;
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

#[cfg(test)]
mod tests {
    use super::{choose_segment, os_level, os_threshold, segment_arch, AppChoice};

    #[test]
    fn application_segment_choice() {
        // 32-bit OS: always the 1st segment, even when a 2nd one is written
        assert_eq!(choose_segment("x86", 2, |i| i == 1), AppChoice::NoX86);
        assert_eq!(choose_segment("x86", 2, |_| true), AppChoice::Chosen(0));
        // a single segment on a 64-bit OS
        assert_eq!(choose_segment("x64", 1, |_| true), AppChoice::Chosen(0));
        assert_eq!(choose_segment("x64", 1, |_| false), AppChoice::NoX86);
        // 64-bit OS, two segments: the x64 one wins, then the x86 one, then nothing
        assert_eq!(choose_segment("x64", 2, |_| true), AppChoice::Chosen(1));
        assert_eq!(choose_segment("x64", 2, |i| i == 1), AppChoice::Chosen(1));
        assert_eq!(choose_segment("x64", 2, |i| i == 0), AppChoice::Chosen(0));
        assert_eq!(choose_segment("x64", 2, |_| false), AppChoice::NoneValid);
        assert_eq!((segment_arch(0), segment_arch(1)), ("x86", "x64"));
    }

    #[test]
    fn os_values_and_levels() {
        assert_eq!(os_threshold("10"), Some((10, 0, 0)));
        assert_eq!(os_threshold("Vista"), Some((6, 0, 0)));
        assert_eq!(os_threshold("12"), None);
        // Windows 10 and Windows 11 are both "10.0"; the build tells them apart
        let (w10, w11, w81) = (os_level(10, 0, 19045), os_level(10, 0, 22631), os_level(6, 3, 9600));
        assert!(w10 < os_threshold("11").unwrap() && w10 >= os_threshold("10").unwrap());
        assert!(w11 >= os_threshold("11").unwrap());
        // "<8.1" (8.1 or older) is met by 8.1 and not by 10
        assert!(w81 <= os_threshold("8.1").unwrap() && w10 > os_threshold("8.1").unwrap());
        // "<10" (10 or older) is met by Windows 10 and not by Windows 11
        assert!(w10 <= os_threshold("10").unwrap() && w11 > os_threshold("10").unwrap());
    }
}
