//! System services: OS architecture/version, administrator, processes,
//! single instance, windows, ping, language and shell folders.

use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::NetworkManagement::IpHelper::*;
use windows_sys::Win32::Security::*;
use windows_sys::Win32::System::Diagnostics::ToolHelp::*;
use windows_sys::Win32::System::SystemInformation::*;
use windows_sys::Win32::System::Threading::*;
use windows_sys::Win32::UI::Shell::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use windows_sys::Win32::System::Environment::GetCommandLineW;

use crate::util::{self, from_w, w};

// ------------------------------------------------------------ OS / version

/// Architecture of the host OS (not of the process): "x86" or "x64".
pub fn os_arch() -> &'static str {
    unsafe {
        let mut si: SYSTEM_INFO = std::mem::zeroed();
        GetNativeSystemInfo(&mut si);
        match si.Anonymous.Anonymous.wProcessorArchitecture {
            PROCESSOR_ARCHITECTURE_INTEL => "x86",
            _ => "x64", // AMD64, ARM64 (x64 emulation) and others
        }
    }
}

#[repr(C)]
struct OsVersionInfoW {
    size: u32,
    major: u32,
    minor: u32,
    build: u32,
    platform: u32,
    csd: [u16; 128],
}

#[link(name = "ntdll")]
extern "system" {
    fn RtlGetVersion(info: *mut OsVersionInfoW) -> i32;
}

/// Real (major, minor, build). The original read `CurrentVersion` from the registry,
/// which on Windows 10/11 is still "6.3".
pub fn os_version() -> (u32, u32, u32) {
    unsafe {
        let mut v: OsVersionInfoW = std::mem::zeroed();
        v.size = std::mem::size_of::<OsVersionInfoW>() as u32;
        RtlGetVersion(&mut v);
        (v.major, v.minor, v.build)
    }
}

/// Readable name, used in the header of the debug log.
pub fn os_name() -> String {
    let (ma, mi, b) = os_version();
    let n = match (ma, mi) {
        (10, 0) if b >= 22000 => "Windows 11".to_string(),
        (10, 0) => "Windows 10".to_string(),
        _ => format!("Windows {}.{}", ma, mi),
    };
    format!("{} (build {})", n, b)
}

pub fn is_admin() -> bool {
    unsafe {
        let mut tok: HANDLE = null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut tok) == 0 {
            return false;
        }
        let mut el = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut ret = 0u32;
        let ok = GetTokenInformation(
            tok, TokenElevation, &mut el as *mut _ as *mut _, std::mem::size_of::<TOKEN_ELEVATION>() as u32, &mut ret,
        );
        CloseHandle(tok);
        ok != 0 && el.TokenIsElevated != 0
    }
}

/// Restarts this executable with the "runas" verb. true = the launch was accepted.
pub fn relaunch_elevated(exe: &str, params: &str, dir: &str) -> bool {
    unsafe {
        let r = ShellExecuteW(
            null_mut(), w("runas").as_ptr(), w(exe).as_ptr(),
            if params.is_empty() { null() } else { w(params).as_ptr() },
            w(dir).as_ptr(), SW_SHOWNORMAL,
        );
        (r as usize) > 32
    }
}

/// Raw command line after the program name (`$CmdLineRaw`).
pub fn cmdline_raw() -> String {
    unsafe {
        let p = GetCommandLineW();
        let mut n = 0;
        while *p.add(n) != 0 {
            n += 1;
        }
        let full = String::from_utf16_lossy(std::slice::from_raw_parts(p, n));
        let mut chars = full.chars().peekable();
        if chars.peek() == Some(&'"') {
            chars.next();
            for c in chars.by_ref() {
                if c == '"' {
                    break;
                }
            }
        } else {
            for c in chars.by_ref() {
                if c == ' ' || c == '\t' {
                    break;
                }
            }
        }
        chars.collect::<String>().trim().to_string()
    }
}

// ---------------------------------------------------------------- processes

pub struct ProcInfo {
    pub pid: u32,
    pub parent: u32,
    pub name: String,
}

pub fn list_processes() -> Vec<ProcInfo> {
    let mut out = Vec::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return out;
        }
        let mut pe: PROCESSENTRY32W = std::mem::zeroed();
        pe.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        if Process32FirstW(snap, &mut pe) != 0 {
            loop {
                out.push(ProcInfo {
                    pid: pe.th32ProcessID,
                    parent: pe.th32ParentProcessID,
                    name: from_w(&pe.szExeFile),
                });
                if Process32NextW(snap, &mut pe) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snap);
    }
    out
}

/// Handles of the processes gaP started and does not wait for on the spot (the main application and the `run`
/// items without `wait`), as `usize` so the list can be a static.
static CHILDREN: Mutex<Vec<usize>> = Mutex::new(Vec::new());

/// Ids of every process gaP started (also the ones it waited for), to find what they left running (debug log only).
static LAUNCHED: Mutex<Vec<u32>> = Mutex::new(Vec::new());

/// Processes alive now that gaP started or that descend from one it started, as (id, parent id, name). The tree is
/// followed through the parent ids of the snapshot, so a process whose parent already ended is still found when the
/// id of that parent is one gaP started. The system keeps the id of the parent, not a link to it: a process whose
/// parent is a dead descendant (not one gaP started) is not found.
pub fn launched_descendants() -> Vec<(u32, u32, String)> {
    let me = unsafe { GetCurrentProcessId() };
    let started: Vec<u32> = LAUNCHED.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let mut tree = started.clone();
    let all = list_processes();
    let mut found: Vec<(u32, u32, String)> = Vec::new();
    loop {
        let before = tree.len();
        for p in &all {
            if p.pid != me && p.pid != p.parent && !tree.contains(&p.pid) && tree.contains(&p.parent) {
                tree.push(p.pid);
                found.push((p.pid, p.parent, p.name.clone()));
            }
        }
        if tree.len() == before {
            break;
        }
    }
    // The processes gaP started that are still running are listed too.
    for p in &all {
        if started.contains(&p.pid) && !found.iter().any(|f| f.0 == p.pid) {
            found.push((p.pid, p.parent, p.name.clone()));
        }
    }
    found
}

/// The processes that gaP started and that are still running. The handles of the ones that already ended are
/// closed and dropped.
pub fn running_children() -> Vec<HANDLE> {
    let mut list = CHILDREN.lock().unwrap_or_else(|e| e.into_inner());
    list.retain(|&h| unsafe {
        if WaitForSingleObject(h as HANDLE, 0) == WAIT_TIMEOUT {
            true
        } else {
            CloseHandle(h as HANDLE);
            false
        }
    });
    list.iter().map(|&h| h as HANDLE).collect()
}

/// Timeout of "no limit" for `wait_any`.
pub const FOREVER: u32 = INFINITE;

/// Most handles one wait can take.
const MAX_WAIT: usize = 64;

/// Sleeps until one of `children` ends, `extra` is signaled or `timeout_ms` passes. With more handles than a
/// single wait takes, the first ones are waited for and the caller should use a timeout.
pub fn wait_any(children: &[HANDLE], extra: Option<HANDLE>, timeout_ms: u32) {
    let mut list: Vec<HANDLE> = children.iter().copied().take(MAX_WAIT - 1).collect();
    list.extend(extra);
    if !list.is_empty() {
        unsafe {
            WaitForMultipleObjects(list.len() as u32, list.as_ptr(), 0, timeout_ms);
        }
    }
}

/// ProcessClose by name prefix (case-insensitive; optional trailing `*`).
/// Never terminates gaP itself.
pub fn kill_by_prefix(pattern: &str) -> usize {
    let pat = pattern.trim_end_matches('*');
    let me = unsafe { GetCurrentProcessId() };
    let mut n = 0;
    for p in list_processes() {
        if p.pid != me && crate::util::starts_with_ic(&p.name, pat) {
            unsafe {
                let h = OpenProcess(PROCESS_TERMINATE, 0, p.pid);
                if !h.is_null() {
                    if TerminateProcess(h, 1) != 0 {
                        n += 1;
                    }
                    CloseHandle(h);
                }
            }
        }
    }
    n
}

/// Run/RunWait. `cmdline` already contains the quoted executable and the arguments. A process that is not waited
/// for is remembered (see `running_children`).
pub fn launch(cmdline: &str, cwd: &str, show: u16, wait: bool) -> Result<(), u32> {
    unsafe {
        let mut si: STARTUPINFOW = std::mem::zeroed();
        si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        si.dwFlags = STARTF_USESHOWWINDOW;
        si.wShowWindow = show;
        let mut pi: PROCESS_INFORMATION = std::mem::zeroed();
        let mut cl = w(cmdline);
        let cwd_w = w(cwd);
        let ok = CreateProcessW(
            null(), cl.as_mut_ptr(), null(), null(), 0, CREATE_UNICODE_ENVIRONMENT,
            null(), if cwd.is_empty() { null() } else { cwd_w.as_ptr() }, &si, &mut pi,
        );
        if ok == 0 {
            return Err(GetLastError());
        }
        CloseHandle(pi.hThread);
        LAUNCHED.lock().unwrap_or_else(|e| e.into_inner()).push(pi.dwProcessId);
        if wait {
            WaitForSingleObject(pi.hProcess, INFINITE);
            CloseHandle(pi.hProcess);
        } else {
            CHILDREN.lock().unwrap_or_else(|e| e.into_inner()).push(pi.hProcess as usize);
        }
        Ok(())
    }
}

/// Runs `exe args…` without a window and waits (replaces `RunWait(@ComSpec & " /C …")`).
pub fn run_hidden_wait(exe: &str, args: &[&str]) -> bool {
    use std::os::windows::process::CommandExt;
    std::process::Command::new(exe)
        .args(args)
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Architecture of this process (gaP itself): "x86" or "x64".
pub fn gap_arch() -> &'static str {
    if cfg!(target_pointer_width = "64") {
        "x64"
    } else {
        "x86"
    }
}

/// Windows directory (`C:\Windows`); never redirected.
pub fn windows_dir() -> String {
    use std::sync::OnceLock;
    static DIR: OnceLock<String> = OnceLock::new();
    DIR.get_or_init(|| unsafe {
        let mut buf = [0u16; 260];
        let n = GetWindowsDirectoryW(buf.as_mut_ptr(), 260);
        String::from_utf16_lossy(&buf[..n as usize])
    })
    .clone()
}

/// A tool of the system folder that matches the architecture of the main application
/// (`SysWOW64` for x86, the real `System32` for x64), whatever the architecture of gaP is.
/// On 32-bit Windows there is only `System32`.
pub fn system32_for(app_arch: &str, name: &str) -> String {
    if os_arch() != "x64" {
        return system32(name);
    }
    let dir = match (app_arch, gap_arch()) {
        ("x86", _) => "SysWOW64",
        (_, "x64") => "System32",
        _ => "Sysnative", // gaP x86 reaching the 64-bit System32
    };
    format!("{}\\{}\\{}", windows_dir(), dir, name)
}

/// `util::rewrite_system32` for this machine: a path as the main application sees it.
pub fn io_path(app_arch: &str, p: &str) -> String {
    util::rewrite_system32(p, &windows_dir(), os_arch(), gap_arch(), app_arch)
}

pub fn system32(name: &str) -> String {
    unsafe {
        let mut buf = [0u16; 260];
        let n = GetSystemDirectoryW(buf.as_mut_ptr(), 260);
        format!("{}\\{}", String::from_utf16_lossy(&buf[..n as usize]), name)
    }
}

// ------------------------------------------------------------------ windows

unsafe extern "system" fn enum_cb(hwnd: HWND, lp: LPARAM) -> i32 {
    let v = &mut *(lp as *mut Vec<(HWND, String)>);
    let n = GetWindowTextLengthW(hwnd);
    if n > 0 {
        let mut buf = vec![0u16; n as usize + 1];
        GetWindowTextW(hwnd, buf.as_mut_ptr(), n + 1);
        v.push((hwnd, from_w(&buf)));
    }
    1
}

/// A top-level window (visible or not) exists whose title contains `part`
/// (WinTitleMatchMode 2), ignoring windows of this process.
pub fn window_title_exists(part: &str) -> bool {
    let mut v: Vec<(HWND, String)> = Vec::new();
    unsafe {
        EnumWindows(Some(enum_cb), &mut v as *mut _ as LPARAM);
        let me = GetCurrentProcessId();
        v.iter().any(|(h, t)| {
            let mut pid = 0u32;
            GetWindowThreadProcessId(*h, &mut pid);
            pid != me && crate::util::contains_ic(t, part)
        })
    }
}

/// A process other than this one runs from an executable named `name` (with or without `.exe`, case ignored). It
/// finds the programs that have no window, such as the ones that live in the notification area.
pub fn process_exists(name: &str) -> bool {
    let me = std::process::id();
    let want = name.trim().to_lowercase();
    let want = want.strip_suffix(".exe").unwrap_or(&want).to_string();
    !want.is_empty()
        && list_processes().iter().any(|p| {
            let n = p.name.to_lowercase();
            p.pid != me && n.strip_suffix(".exe").unwrap_or(&n) == want
        })
}

// ---------------------------------------------------------- single instance

/// The mutex of this process (0 = none), kept open until the process ends or `release_singleton`.
static SINGLETON: AtomicUsize = AtomicUsize::new(0);

pub enum Singleton {
    /// This is the first instance: it owns the mutex until the process ends.
    First,
    /// Another instance of the same executable already runs in this session.
    Existing,
}

/// Creates the named mutex that tells the instances of an executable apart. `Local\` keeps it per session.
pub fn acquire_singleton(name: &str) -> Singleton {
    unsafe {
        let h = CreateMutexW(null(), 0, w(&format!("Local\\{}", name)).as_ptr());
        let err = GetLastError();
        if h.is_null() {
            // The mutex exists and belongs to a process with more rights (an elevated copy of the program):
            // opening it is denied, and that also means another instance runs.
            return if err == ERROR_ACCESS_DENIED { Singleton::Existing } else { Singleton::First };
        }
        if err == ERROR_ALREADY_EXISTS {
            CloseHandle(h);
            Singleton::Existing
        } else {
            // The handle is kept open on purpose: the system releases it when the process ends.
            SINGLETON.store(h as usize, Ordering::SeqCst);
            Singleton::First
        }
    }
}

/// Lets go of the mutex before this process starts its elevated copy, which must find itself the first instance.
pub fn release_singleton() {
    let h = SINGLETON.swap(0, Ordering::SeqCst);
    if h != 0 {
        unsafe {
            CloseHandle(h as HANDLE);
        }
    }
}

// ------------------------------------------------------------------ network

fn ping_ip(ip: std::net::Ipv4Addr, timeout_ms: u32) -> bool {
    unsafe {
        let h = IcmpCreateFile();
        if h == INVALID_HANDLE_VALUE || h.is_null() {
            return false;
        }
        let data = [0x61u8; 32];
        // ICMP_ECHO_REPLY has pointers in it: its buffer has to be aligned for it, and a `Vec<u8>` is not.
        let mut reply = vec![0u64; (std::mem::size_of::<ICMP_ECHO_REPLY>() + 64).div_ceil(8)];
        let addr = u32::from_ne_bytes(ip.octets());
        let n = IcmpSendEcho(
            h, addr, data.as_ptr() as _, data.len() as u16, null(), reply.as_mut_ptr() as _, (reply.len() * 8) as u32, timeout_ms,
        );
        IcmpCloseHandle(h);
        if n == 0 {
            return false;
        }
        let r = &*(reply.as_ptr() as *const ICMP_ECHO_REPLY);
        r.Status == 0
    }
}

fn resolve_v4(host: &str) -> Option<std::net::Ipv4Addr> {
    use std::net::ToSocketAddrs;
    if let Ok(ip) = host.parse::<std::net::Ipv4Addr>() {
        return Some(ip);
    }
    (host, 0).to_socket_addrs().ok()?.find_map(|a| match a.ip() {
        std::net::IpAddr::V4(v) => Some(v),
        _ => None,
    })
}

/// `Ping(host, 4000)` > 0.
pub fn ping(host: &str, timeout_ms: u32) -> bool {
    resolve_v4(host).map(|ip| ping_ip(ip, timeout_ms)).unwrap_or(false)
}

/// Fallback TCP connection (networks that block ICMP).
pub fn tcp_reachable(host: &str, port: u16, timeout_ms: u64) -> bool {
    use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
    let Ok(mut addrs) = (host, port).to_socket_addrs() else { return false };
    addrs.any(|a: SocketAddr| TcpStream::connect_timeout(&a, std::time::Duration::from_millis(timeout_ms)).is_ok())
}

// ----------------------------------------------------------------- language

/// `@OSLang` in decimal (e.g., 1046 = pt-BR, 2057 = en-GB).
pub fn os_lang() -> u32 {
    unsafe { windows_sys::Win32::Globalization::GetSystemDefaultUILanguage() as u32 }
}

/// The language of the system as the system names it: (locale name "pt-BR", English name of the language
/// "Portuguese", English name of the country "Brazil").
pub fn locale() -> Option<(String, String, String)> {
    use windows_sys::Win32::Globalization::{
        GetLocaleInfoEx, LCIDToLocaleName, LOCALE_SENGLISHCOUNTRYNAME, LOCALE_SENGLISHLANGUAGENAME,
    };
    // LOCALE_NAME_MAX_LENGTH
    const NAME_MAX: usize = 85;
    unsafe {
        let mut name = [0u16; NAME_MAX];
        if LCIDToLocaleName(os_lang(), name.as_mut_ptr(), name.len() as i32, 0) <= 0 {
            return None;
        }
        let info = |kind: u32| -> String {
            let mut buf = [0u16; 128];
            if GetLocaleInfoEx(name.as_ptr(), kind, buf.as_mut_ptr(), buf.len() as i32) > 0 {
                from_w(&buf)
            } else {
                String::new()
            }
        };
        Some((from_w(&name), info(LOCALE_SENGLISHLANGUAGENAME), info(LOCALE_SENGLISHCOUNTRYNAME)))
    }
}

// ------------------------------------------------------------- shell folders

pub fn csidl_path(csidl: i32) -> Option<String> {
    unsafe {
        let mut buf = [0u16; 520];
        let r = SHGetFolderPathW(null_mut(), csidl, null_mut(), 0, buf.as_mut_ptr());
        if r == 0 {
            let s = from_w(&buf);
            if !s.is_empty() {
                return Some(s);
            }
        }
        None
    }
}

/// Drive type / file system of the volume that contains `path` ("NTFS"...).
pub fn fs_name(path: &str) -> String {
    use windows_sys::Win32::Storage::FileSystem::GetVolumeInformationW;
    let root: String = path.chars().take(1).collect::<String>() + ":\\";
    unsafe {
        let mut name = [0u16; 64];
        let ok = GetVolumeInformationW(
            w(&root).as_ptr(), null_mut(), 0, null_mut(), null_mut(), null_mut(), name.as_mut_ptr(), 64,
        );
        if ok != 0 {
            from_w(&name)
        } else {
            String::new()
        }
    }
}
