//! Basic Win32 dialogs: MsgBox, InputBox (with password mask),
//! FileSelectFolder, FileOpenDialog and clipboard.

use std::cell::RefCell;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows_sys::Win32::System::DataExchange::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::*;
use windows_sys::Win32::UI::Controls::Dialogs::*;
use windows_sys::Win32::UI::Shell::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use windows_sys::Win32::System::Ole::CF_UNICODETEXT;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;

use crate::util::{from_w, w};


// ------------------------------------------------------------------ MsgBox

pub fn msgbox_raw(title: &str, text: &str, flags: u32) -> i32 {
    unsafe { MessageBoxW(null_mut(), w(text).as_ptr(), w(title).as_ptr(), flags | MB_SETFOREGROUND) }
}

/// MsgBox(64, …): information with OK.
pub fn info(title: &str, text: &str) {
    msgbox_raw(title, text, MB_OK | MB_ICONINFORMATION);
}

/// Always-on-top information (keyword `message`).
pub fn info_topmost(title: &str, text: &str) {
    msgbox_raw(title, text, MB_OK | MB_ICONINFORMATION | MB_TOPMOST);
}

/// MsgBox(33, …): OK/Cancel. true = OK.
pub fn ok_cancel(title: &str, text: &str) -> bool {
    msgbox_raw(title, text, MB_OKCANCEL | MB_ICONINFORMATION) == IDOK
}

pub fn ok_cancel_topmost(title: &str, text: &str) -> bool {
    msgbox_raw(title, text, MB_OKCANCEL | MB_ICONINFORMATION | MB_TOPMOST) == IDOK
}

// --------------------------------------------------------------- Clipboard

/// Opens the clipboard, trying again for a moment: another program may be using it.
fn open_clipboard() -> bool {
    for _ in 0..10 {
        if unsafe { OpenClipboard(null_mut()) } != 0 {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    false
}

pub fn clip_put(text: &str) -> bool {
    unsafe {
        if !open_clipboard() {
            return false;
        }
        EmptyClipboard();
        let ok = if text.is_empty() {
            true
        } else {
            let data = w(text);
            let bytes = data.len() * 2;
            let h = GlobalAlloc(GMEM_MOVEABLE, bytes);
            if h.is_null() {
                false
            } else {
                let p = GlobalLock(h) as *mut u16;
                std::ptr::copy_nonoverlapping(data.as_ptr(), p, data.len());
                GlobalUnlock(h);
                if SetClipboardData(CF_UNICODETEXT as u32, h as HANDLE).is_null() {
                    GlobalFree(h);
                    false
                } else {
                    true
                }
            }
        };
        CloseClipboard();
        ok
    }
}

pub fn clip_get() -> String {
    unsafe {
        if !open_clipboard() {
            return String::new();
        }
        let mut out = String::new();
        let h = GetClipboardData(CF_UNICODETEXT as u32);
        if !h.is_null() {
            let p = GlobalLock(h as _) as *const u16;
            if !p.is_null() {
                // The text ends at the first NUL, or at the end of the block when the program that put it there
                // did not end it.
                let room = GlobalSize(h as _) / 2;
                let mut n = 0;
                while n < room && *p.add(n) != 0 {
                    n += 1;
                }
                out = String::from_utf16_lossy(std::slice::from_raw_parts(p, n));
                GlobalUnlock(h as _);
            }
        }
        CloseClipboard();
        out
    }
}

// ----------------------------------------------------------- Helper window

pub unsafe fn hinstance() -> HINSTANCE {
    GetModuleHandleW(null()) as HINSTANCE
}

/// DPI of the system (the manifest makes the program aware of it, so windows are not stretched).
pub fn dpi() -> i32 {
    use std::sync::OnceLock;
    static DPI: OnceLock<i32> = OnceLock::new();
    *DPI.get_or_init(|| match unsafe { windows_sys::Win32::UI::HiDpi::GetDpiForSystem() } {
        0 => 96,
        d => d as i32,
    })
}

/// A size designed for 96 DPI, scaled to the DPI of the system.
pub fn scale(v: i32) -> i32 {
    (v as i64 * dpi() as i64 / 96) as i32
}

/// The font of message boxes of the system (at the system DPI) for the controls; the old stock font if it cannot
/// be read.
pub unsafe fn gui_font() -> HFONT {
    use std::sync::OnceLock;
    static FONT: OnceLock<usize> = OnceLock::new();
    *FONT.get_or_init(|| {
        let mut ncm: NONCLIENTMETRICSW = std::mem::zeroed();
        ncm.cbSize = std::mem::size_of::<NONCLIENTMETRICSW>() as u32;
        let ok = windows_sys::Win32::UI::HiDpi::SystemParametersInfoForDpi(
            SPI_GETNONCLIENTMETRICS, ncm.cbSize, &mut ncm as *mut _ as *mut _, 0, dpi() as u32,
        );
        let font = if ok != 0 { CreateFontIndirectW(&ncm.lfMessageFont) } else { null_mut() };
        if font.is_null() {
            GetStockObject(DEFAULT_GUI_FONT) as usize
        } else {
            font as usize
        }
    }) as HFONT
}

pub unsafe fn make_ctrl(
    parent: HWND, class: &str, text: &str, style: u32, ex: u32,
    x: i32, y: i32, wd: i32, ht: i32, id: i32,
) -> HWND {
    let h = CreateWindowExW(
        ex, w(class).as_ptr(), w(text).as_ptr(), WS_CHILD | WS_VISIBLE | style,
        scale(x), scale(y), scale(wd), scale(ht), parent, id as usize as HMENU, hinstance(), null(),
    );
    if !h.is_null() {
        SendMessageW(h, WM_SETFONT, gui_font() as WPARAM, 1);
    }
    h
}

/// Creates a top-level window with a centered client area (cw x ch).
pub unsafe fn make_window(
    class: &str, title: &str, cw: i32, ch: i32, wndproc: WNDPROC, bg: COLORREF, style: u32,
) -> HWND {
    let hi = hinstance();
    let cls = w(class);
    let mut wc: WNDCLASSW = std::mem::zeroed();
    wc.lpfnWndProc = wndproc;
    wc.hInstance = hi;
    wc.hCursor = LoadCursorW(null_mut(), IDC_ARROW);
    wc.hbrBackground = CreateSolidBrush(bg);
    wc.lpszClassName = cls.as_ptr();
    wc.hIcon = LoadIconW(hi, std::ptr::without_provenance::<u16>(1));
    RegisterClassW(&wc);
    let mut r = RECT { left: 0, top: 0, right: scale(cw), bottom: scale(ch) };
    AdjustWindowRectEx(&mut r, style, 0, 0);
    let (ww, wh) = (r.right - r.left, r.bottom - r.top);
    let x = (GetSystemMetrics(SM_CXSCREEN) - ww) / 2;
    let y = (GetSystemMetrics(SM_CYSCREEN) - wh) / 2;
    CreateWindowExW(0, cls.as_ptr(), w(title).as_ptr(), style, x, y, ww, wh, null_mut(), null_mut(), hi, null())
}

pub fn init_common() {
    unsafe {
        use windows_sys::Win32::UI::Controls::{InitCommonControlsEx, ICC_STANDARD_CLASSES, INITCOMMONCONTROLSEX};
        let ic = INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_STANDARD_CLASSES,
        };
        InitCommonControlsEx(&ic);
        CoInitializeEx(null(), COINIT_APARTMENTTHREADED as u32);
    }
}

/// Message loop until WM_QUIT; returns the exit code.
pub unsafe fn run_loop(dlg: HWND) -> i32 {
    let mut msg: MSG = std::mem::zeroed();
    while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
        if !dlg.is_null() && IsDialogMessageW(dlg, &msg) != 0 {
            continue;
        }
        TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
    msg.wParam as i32
}

// --------------------------------------------------------------- InputBox

struct InputState {
    edit: HWND,
    result: Option<String>,
    done: bool,
}

thread_local! {
    static INPUT: RefCell<Option<InputState>> = const { RefCell::new(None) };
}

const ID_OK: i32 = 1;
const ID_CANCEL: i32 = 2;
const ID_EDIT: i32 = 100;

unsafe extern "system" fn input_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_COMMAND => {
            let id = (wp & 0xFFFF) as i32;
            if id == ID_OK || id == ID_CANCEL {
                INPUT.with(|s| {
                    if let Some(st) = s.borrow_mut().as_mut() {
                        if id == ID_OK {
                            let n = GetWindowTextLengthW(st.edit);
                            let mut buf = vec![0u16; n as usize + 2];
                            GetWindowTextW(st.edit, buf.as_mut_ptr(), n + 1);
                            st.result = Some(from_w(&buf));
                        }
                        st.done = true;
                    }
                });
                DestroyWindow(hwnd);
                return 0;
            }
        }
        WM_CLOSE => {
            INPUT.with(|s| {
                if let Some(st) = s.borrow_mut().as_mut() {
                    st.done = true;
                }
            });
            DestroyWindow(hwnd);
            return 0;
        }
        _ => {}
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

/// InputBox(title, prompt, "", mask?, 360, 125). `None` = canceled.
pub fn input_box(title: &str, prompt: &str, password: bool) -> Option<String> {
    unsafe {
        init_common();
        let style = WS_CAPTION | WS_SYSMENU | WS_POPUP | WS_VISIBLE;
        let hwnd = make_window("gaPInputBox", title, 360, 125, Some(input_proc), GetSysColor(COLOR_BTNFACE), style);
        if hwnd.is_null() {
            return None;
        }
        SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE);
        make_ctrl(hwnd, "STATIC", prompt, 0, 0, 12, 12, 330, 18, -1);
        let es = if password { ES_PASSWORD as u32 } else { 0 };
        let edit = make_ctrl(hwnd, "EDIT", "", WS_TABSTOP | ES_AUTOHSCROLL as u32 | es, WS_EX_CLIENTEDGE, 12, 34, 332, 23, ID_EDIT);
        make_ctrl(hwnd, "BUTTON", "OK", WS_TABSTOP | BS_DEFPUSHBUTTON as u32, 0, 190, 80, 75, 26, ID_OK);
        make_ctrl(hwnd, "BUTTON", "Cancel", WS_TABSTOP | BS_PUSHBUTTON as u32, 0, 270, 80, 75, 26, ID_CANCEL);
        INPUT.with(|s| *s.borrow_mut() = Some(InputState { edit, result: None, done: false }));
        SetForegroundWindow(hwnd);
        SetFocus(edit);
        let mut msg: MSG = std::mem::zeroed();
        loop {
            let done = INPUT.with(|s| s.borrow().as_ref().map(|x| x.done).unwrap_or(true));
            if done {
                break;
            }
            if GetMessageW(&mut msg, null_mut(), 0, 0) <= 0 {
                break;
            }
            if IsDialogMessageW(hwnd, &msg) != 0 {
                continue;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        INPUT.with(|s| s.borrow_mut().take()).and_then(|s| s.result)
    }
}

// ----------------------------------------------------------------- Pickers

unsafe extern "system" fn browse_cb(hwnd: HWND, msg: u32, _lp: LPARAM, data: LPARAM) -> i32 {
    if msg == BFFM_INITIALIZED {
        SendMessageW(hwnd, BFFM_SETSELECTIONW, 1, data);
    }
    0
}

/// FileSelectFolder(prompt, "%HomeDrive%", 1). `None` = canceled.
pub fn select_folder(prompt: &str) -> Option<String> {
    unsafe {
        init_common();
        let start = std::env::var("HOMEDRIVE").unwrap_or_else(|_| "C:".into()) + "\\";
        let wstart = w(&start);
        let wprompt = w(prompt);
        let mut name = [0u16; 260];
        let bi = BROWSEINFOW {
            hwndOwner: null_mut(),
            pidlRoot: null_mut(),
            pszDisplayName: name.as_mut_ptr(),
            lpszTitle: wprompt.as_ptr(),
            ulFlags: BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE,
            lpfn: Some(browse_cb),
            lParam: wstart.as_ptr() as LPARAM,
            iImage: 0,
        };
        let pidl = SHBrowseForFolderW(&bi);
        if pidl.is_null() {
            return None;
        }
        let mut path = [0u16; 32768 / 8];
        let ok = SHGetPathFromIDListW(pidl, path.as_mut_ptr());
        windows_sys::Win32::System::Com::CoTaskMemFree(pidl as _);
        if ok != 0 {
            Some(from_w(&path))
        } else {
            None
        }
    }
}

/// FileOpenDialog(prompt, "%HomeDrive%", "All (*.*)"). `None` = canceled.
pub fn select_file(prompt: &str) -> Option<String> {
    unsafe {
        init_common();
        let start = std::env::var("HOMEDRIVE").unwrap_or_else(|_| "C:".into()) + "\\";
        let wstart = w(&start);
        let wtitle = w(prompt);
        let filter: Vec<u16> = "All (*.*)\0*.*\0\0".encode_utf16().collect();
        let mut buf = vec![0u16; 32768];
        let mut ofn: OPENFILENAMEW = std::mem::zeroed();
        ofn.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
        ofn.lpstrFilter = filter.as_ptr();
        ofn.lpstrFile = buf.as_mut_ptr();
        ofn.nMaxFile = buf.len() as u32;
        ofn.lpstrInitialDir = wstart.as_ptr();
        ofn.lpstrTitle = wtitle.as_ptr();
        ofn.Flags = OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_EXPLORER | OFN_HIDEREADONLY;
        if GetOpenFileNameW(&mut ofn) != 0 {
            Some(from_w(&buf))
        } else {
            None
        }
    }
}

/// Opens a URL/file with the default program (ShellExecute).
pub fn shell_open(target: &str) {
    unsafe {
        ShellExecuteW(null_mut(), w("open").as_ptr(), w(target).as_ptr(), null(), null(), SW_SHOWNORMAL);
    }
}
