//! Serviços de sistema: arquitetura/versão do SO, administrador, processos,
//! instância única, janelas, ping, idioma e pastas do shell.

use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::NetworkManagement::IpHelper::*;
use windows_sys::Win32::Security::*;
use windows_sys::Win32::System::Diagnostics::ToolHelp::*;
use windows_sys::Win32::System::ProcessStatus::K32EmptyWorkingSet;
use windows_sys::Win32::System::SystemInformation::*;
use windows_sys::Win32::System::Threading::*;
use windows_sys::Win32::UI::Shell::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use windows_sys::Win32::Storage::FileSystem::SYNCHRONIZE;
use windows_sys::Win32::System::Environment::GetCommandLineW;

use crate::util::{from_w, w};

// ------------------------------------------------------------- SO / versão

/// Arquitetura do SO hospedeiro (não do processo): "x86" ou "x64".
pub fn os_arch() -> &'static str {
    unsafe {
        let mut si: SYSTEM_INFO = std::mem::zeroed();
        GetNativeSystemInfo(&mut si);
        match si.Anonymous.Anonymous.wProcessorArchitecture {
            PROCESSOR_ARCHITECTURE_INTEL => "x86",
            _ => "x64", // AMD64, ARM64 (emulação x64) e demais
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

/// (major, minor, build) reais. O original lia `CurrentVersion` do registro,
/// que no Windows 10/11 continua "6.3".
pub fn os_version() -> (u32, u32, u32) {
    unsafe {
        let mut v: OsVersionInfoW = std::mem::zeroed();
        v.size = std::mem::size_of::<OsVersionInfoW>() as u32;
        RtlGetVersion(&mut v);
        (v.major, v.minor, v.build)
    }
}

/// Nome legível, usado no cabeçalho do log de depuração.
pub fn os_name() -> String {
    let (ma, mi, b) = os_version();
    let n = match (ma, mi) {
        (5, 1) => "Windows XP".to_string(),
        (5, 2) => "Windows XP x64/2003".to_string(),
        (6, 0) => "Windows Vista".to_string(),
        (6, 1) => "Windows 7".to_string(),
        (6, 2) => "Windows 8".to_string(),
        (6, 3) => "Windows 8.1".to_string(),
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

/// Reinicia este executável com o verbo "runas". true = o lançamento foi aceito.
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

pub fn reduce_memory() {
    unsafe {
        K32EmptyWorkingSet(GetCurrentProcess());
    }
}

/// Linha de comando bruta depois do nome do programa (`$CmdLineRaw`).
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

// ---------------------------------------------------------------- processos

pub struct ProcInfo {
    #[allow(dead_code)]
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
                out.push(ProcInfo { pid: pe.th32ProcessID, parent: pe.th32ParentProcessID, name: from_w(&pe.szExeFile) });
                if Process32NextW(snap, &mut pe) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snap);
    }
    out
}

fn ft_to_u64(ft: FILETIME) -> u64 {
    ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64
}

unsafe fn creation_time(h: HANDLE) -> u64 {
    let (mut c, mut e, mut k, mut u): (FILETIME, FILETIME, FILETIME, FILETIME) = std::mem::zeroed();
    if GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u) != 0 {
        ft_to_u64(c)
    } else {
        0
    }
}

/// Processos-filho vivos do processo atual (`_WinAPI_EnumChildProcess`).
/// Ignora PIDs reaproveitados (filhos "criados antes" de nós) e já encerrados.
pub fn live_children() -> Vec<u32> {
    let me = unsafe { GetCurrentProcessId() };
    let my_start = unsafe { creation_time(GetCurrentProcess()) };
    let mut v = Vec::new();
    for p in list_processes() {
        if p.parent != me || p.pid == me {
            continue;
        }
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE, 0, p.pid);
            if h.is_null() {
                // sem acesso: assume vivo se está listado
                v.push(p.pid);
                continue;
            }
            let mut code = 0u32;
            let alive = GetExitCodeProcess(h, &mut code) != 0 && code == STILL_ACTIVE as u32;
            let start = creation_time(h);
            CloseHandle(h);
            if alive && start >= my_start {
                v.push(p.pid);
            }
        }
    }
    v
}

/// ProcessClose por prefixo do nome (case-insensitive; `*` final opcional).
/// Nunca encerra o próprio gaP.
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

pub struct Launched {
    #[allow(dead_code)]
    pub pid: u32,
}

/// Run/RunWait. `cmdline` já contém o executável entre aspas e os argumentos.
pub fn launch(cmdline: &str, cwd: &str, show: u16, wait: bool) -> Result<Launched, u32> {
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
        if wait {
            WaitForSingleObject(pi.hProcess, INFINITE);
        }
        CloseHandle(pi.hThread);
        CloseHandle(pi.hProcess);
        Ok(Launched { pid: pi.dwProcessId })
    }
}

/// Executa `exe args…` sem janela e espera (substitui `RunWait(@ComSpec & " /C …")`).
pub fn run_hidden_wait(exe: &str, args: &[&str]) -> bool {
    use std::os::windows::process::CommandExt;
    std::process::Command::new(exe)
        .args(args)
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub fn system32(name: &str) -> String {
    unsafe {
        let mut buf = [0u16; 260];
        let n = GetSystemDirectoryW(buf.as_mut_ptr(), 260);
        format!("{}\\{}", String::from_utf16_lossy(&buf[..n as usize]), name)
    }
}

// ------------------------------------------------------------------ janelas

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

/// Existe janela de nível superior (visível ou não) cujo título contém `part`
/// (WinTitleMatchMode 2), ignorando janelas deste processo.
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

// ---------------------------------------------------------- instância única

unsafe extern "system" fn singleton_proc(h: HWND, m: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    DefWindowProcW(h, m, wp, lp)
}

/// `_SingletonPID`: devolve 0 se somos a primeira instância (e registra a
/// janela-marcador oculta) ou o PID da instância já existente.
pub fn singleton_pid(code: &str) -> u32 {
    unsafe {
        let title = w(code);
        let existing = FindWindowExW(HWND_MESSAGE, null_mut(), null(), title.as_ptr());
        if !existing.is_null() {
            let mut pid = 0u32;
            GetWindowThreadProcessId(existing, &mut pid);
            return pid;
        }
        let hi = crate::ui::hinstance();
        let cls = w("gaPSingletonMarker");
        let mut wc: WNDCLASSW = std::mem::zeroed();
        wc.lpfnWndProc = Some(singleton_proc);
        wc.hInstance = hi;
        wc.lpszClassName = cls.as_ptr();
        RegisterClassW(&wc);
        CreateWindowExW(0, cls.as_ptr(), title.as_ptr(), 0, 0, 0, 0, 0, HWND_MESSAGE, null_mut(), hi, null());
        0
    }
}

// --------------------------------------------------------------------- rede

fn ping_ip(ip: std::net::Ipv4Addr, timeout_ms: u32) -> bool {
    unsafe {
        let h = IcmpCreateFile();
        if h == INVALID_HANDLE_VALUE || h.is_null() {
            return false;
        }
        let data = [0x61u8; 32];
        let mut reply = vec![0u8; std::mem::size_of::<ICMP_ECHO_REPLY>() + 64];
        let addr = u32::from_ne_bytes(ip.octets());
        let n = IcmpSendEcho(
            h, addr, data.as_ptr() as _, data.len() as u16, null(), reply.as_mut_ptr() as _, reply.len() as u32, timeout_ms,
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

/// Conexão TCP de reserva (redes que bloqueiam ICMP).
pub fn tcp_reachable(host: &str, port: u16, timeout_ms: u64) -> bool {
    use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
    let Ok(mut addrs) = (host, port).to_socket_addrs() else { return false };
    addrs.any(|a: SocketAddr| TcpStream::connect_timeout(&a, std::time::Duration::from_millis(timeout_ms)).is_ok())
}

// ------------------------------------------------------------------- idioma

/// `@OSLang` em decimal (ex.: 1046 = pt-BR, 2057 = en-GB).
pub fn os_lang() -> u32 {
    unsafe { windows_sys::Win32::Globalization::GetSystemDefaultUILanguage() as u32 }
}

// -------------------------------------------------------------- pastas shell

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

/// Drive type / sistema de arquivos do volume que contém `path` ("NTFS"...).
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
