//! Talking to the first instance of the program: a named pipe it listens on.
//!
//! The other instances connect to the pipe, write their command line as one message (UTF-16, so accented
//! characters arrive intact) and leave. The first instance reads the messages in a thread of its own and wakes
//! the loop that waits for the main application through an event.
//!
//! The pipe is only open to the user who runs the program (elevated or not) and to the system, and never to other
//! computers. Its name has the session number: users of other sessions have their own.

use std::ptr::{null, null_mut};
use std::sync::mpsc::{channel, Receiver, Sender};

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Security::Authorization::*;
use windows_sys::Win32::Security::*;
use windows_sys::Win32::Storage::FileSystem::*;
use windows_sys::Win32::System::Pipes::*;
use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows_sys::Win32::System::Threading::*;

use crate::util::{from_w, w};

/// Largest command line accepted (in bytes of the message).
const MAX_MESSAGE: usize = 1 << 20;

/// `\\.\pipe\gaP.<session>.<hash>`; `hash` identifies this executable.
pub fn pipe_name(hash: &str) -> String {
    let mut session = 0u32;
    unsafe {
        if ProcessIdToSessionId(GetCurrentProcessId(), &mut session) == 0 {
            session = 0;
        }
    }
    format!("\\\\.\\pipe\\gaP.{}.{}", session, hash)
}

/// The receiving end, owned by the first instance.
pub struct Server {
    rx: Receiver<String>,
    /// Auto-reset event, signaled when a message is waiting.
    event: usize,
}

impl Server {
    /// Creates the pipe and starts listening. `None` when the pipe cannot be created (the program then runs
    /// without receiving command lines from other instances).
    pub fn start(name: &str) -> Option<Server> {
        let wname = w(name);
        unsafe {
            let (psd, sa) = security();
            // The pipe of a copy that is about to end (the one that started this elevated copy) can still exist
            // for a moment: try again for a second.
            let mut pipe = INVALID_HANDLE_VALUE;
            for _ in 0..20 {
                pipe = CreateNamedPipeW(
                    wname.as_ptr(),
                    PIPE_ACCESS_INBOUND | FILE_FLAG_FIRST_PIPE_INSTANCE,
                    PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                    1,
                    0,
                    0,
                    0,
                    if psd.is_null() { null() } else { &sa },
                );
                if pipe != INVALID_HANDLE_VALUE || GetLastError() != ERROR_ACCESS_DENIED {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            if !psd.is_null() {
                LocalFree(psd as _);
            }
            if pipe == INVALID_HANDLE_VALUE {
                return None;
            }
            let event = CreateEventW(null(), 0, 0, null());
            if event.is_null() {
                CloseHandle(pipe);
                return None;
            }
            let (tx, rx) = channel();
            let (p, e) = (pipe as usize, event as usize);
            std::thread::Builder::new().spawn(move || serve(p, e, tx)).ok()?;
            Some(Server { rx, event: event as usize })
        }
    }

    /// Handle to wait on together with the processes.
    pub fn event(&self) -> HANDLE {
        self.event as HANDLE
    }

    /// The next command line received, if any.
    pub fn try_recv(&self) -> Option<String> {
        self.rx.try_recv().ok()
    }
}

/// Security descriptor of the pipe: full access to the current user and to the system, nothing else.
/// Returns (descriptor to free with `LocalFree`, attributes pointing to it); a null descriptor means the default.
unsafe fn security() -> (PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES) {
    let sa = SECURITY_ATTRIBUTES { nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: null_mut(), bInheritHandle: 0 };
    let Some(sid) = user_sid() else { return (null_mut(), sa) };
    let sddl = w(&format!("D:P(A;;GA;;;{})(A;;GA;;;SY)", sid));
    let mut psd: PSECURITY_DESCRIPTOR = null_mut();
    if ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), 1, &mut psd, null_mut()) == 0 {
        return (null_mut(), sa);
    }
    (psd, SECURITY_ATTRIBUTES { lpSecurityDescriptor: psd, ..sa })
}

/// SID of the user of this process, as text ("S-1-5-21-...").
unsafe fn user_sid() -> Option<String> {
    let mut token: HANDLE = null_mut();
    if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
        return None;
    }
    let mut len = 0u32;
    GetTokenInformation(token, TokenUser, null_mut(), 0, &mut len);
    // u64 elements: the buffer is aligned for TOKEN_USER
    let mut buf = vec![0u64; (len as usize).div_ceil(8).max(1)];
    let ok = GetTokenInformation(token, TokenUser, buf.as_mut_ptr() as *mut _, len, &mut len);
    CloseHandle(token);
    if ok == 0 {
        return None;
    }
    let user = &*(buf.as_ptr() as *const TOKEN_USER);
    let mut text: *mut u16 = null_mut();
    if ConvertSidToStringSidW(user.User.Sid, &mut text) == 0 || text.is_null() {
        return None;
    }
    let mut n = 0;
    while *text.add(n) != 0 {
        n += 1;
    }
    let s = from_w(std::slice::from_raw_parts(text, n));
    LocalFree(text as _);
    Some(s)
}

/// Thread of the first instance: accepts one client at a time on the single pipe instance, so the pipe never
/// disappears for the clients.
fn serve(pipe: usize, event: usize, tx: Sender<String>) {
    let pipe = pipe as HANDLE;
    let mut failures = 0;
    loop {
        let connected = unsafe { ConnectNamedPipe(pipe, null_mut()) != 0 || GetLastError() == ERROR_PIPE_CONNECTED };
        if connected {
            failures = 0;
            if let Some(text) = read_message(pipe) {
                if tx.send(text).is_ok() {
                    unsafe { SetEvent(event as HANDLE) };
                }
            }
        } else {
            failures += 1;
            if failures > 50 {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        unsafe { DisconnectNamedPipe(pipe) };
    }
}

/// Reads one message (UTF-16 little endian with a byte order mark).
fn read_message(pipe: HANDLE) -> Option<String> {
    let mut data: Vec<u8> = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let mut got = 0u32;
        let ok = unsafe { ReadFile(pipe, buf.as_mut_ptr() as *mut _, buf.len() as u32, &mut got, null_mut()) };
        data.extend_from_slice(&buf[..got as usize]);
        if ok != 0 {
            break; // the whole message was read
        }
        if unsafe { GetLastError() } != ERROR_MORE_DATA || data.len() > MAX_MESSAGE {
            return None;
        }
    }
    let units: Vec<u16> = data.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    Some(String::from_utf16_lossy(units.strip_prefix(&[0xFEFF]).unwrap_or(&units)))
}

/// Sends `text` to the first instance. `false` = nobody received it.
pub fn send(name: &str, text: &str) -> bool {
    let wname = w(name);
    let mut bytes: Vec<u8> = vec![0xFF, 0xFE];
    for u in text.encode_utf16() {
        bytes.extend_from_slice(&u.to_le_bytes());
    }
    // The pipe can be busy with another instance, or not exist yet (the first instance creates it right after
    // it creates its mutex): try again for a few seconds.
    for _ in 0..40 {
        unsafe {
            let h = CreateFileW(wname.as_ptr(), GENERIC_WRITE, 0, null(), OPEN_EXISTING, 0, null_mut());
            if h != INVALID_HANDLE_VALUE {
                let mut written = 0u32;
                let ok = WriteFile(h, bytes.as_ptr(), bytes.len() as u32, &mut written, null_mut()) != 0;
                if ok {
                    FlushFileBuffers(h);
                }
                CloseHandle(h);
                return ok;
            }
            match GetLastError() {
                ERROR_PIPE_BUSY => {
                    WaitNamedPipeW(wname.as_ptr(), 500);
                }
                ERROR_FILE_NOT_FOUND => std::thread::sleep(std::time::Duration::from_millis(50)),
                _ => return false,
            }
        }
    }
    false
}
