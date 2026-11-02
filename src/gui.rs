//! Windows of the `-crypt`, `-license` and `-about` modes (plain Win32).
//!
//! IMPROVEMENTS over the AutoIt version:
//!  * The encryption module reacts to EN_CHANGE/CBN_SELCHANGE instead of
//!    polling the fields every 100 ms (`While 1: Sleep(100): CryptChange()`),
//!    which kept the CPU awake all the time.
//!  * The guide texts ("Write the password…") are *cue banners*, not real
//!    text; in the original they were encrypted as if they were user data.
//!  * The Encrypt/Decrypt combo is a closed list (the original was editable and any
//!    typed value produced empty output).

use std::cell::{Cell, RefCell};
use std::ptr::null_mut;
use std::sync::OnceLock;

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::ctx::{PROGRAM_NAME, PROGRAM_VERSION};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;

use crate::ui::{self, make_ctrl, make_window};

const SS_NOTIFY: i32 = 0x100;
const SS_ICON: i32 = 3;
use crate::util::{self, from_w, w};
use crate::{clean, crypto};

static SCRIPT_FULL: OnceLock<String> = OnceLock::new();
static SCRIPT_DIR: OnceLock<String> = OnceLock::new();

const ID_PASS: i32 = 101;
const ID_COMBO: i32 = 102;
const ID_INPUT: i32 = 103;
const ID_OUTPUT: i32 = 104;
const ID_COPY: i32 = 105;
const ID_SAVE: i32 = 106;
const ID_LINK: i32 = 107;

const HOMEPAGE: &str = "https://github.com/Hanatarou/gaP";

const WS_DEFAULT_GUI: u32 = WS_MINIMIZEBOX | WS_CAPTION | WS_POPUP | WS_SYSMENU;
const EM_SETCUEBANNER_: u32 = 0x1501;
const EN_CHANGE_: u32 = 0x0300;
const CBN_SELCHANGE_: u32 = 1;
const CB_ADDSTRING_: u32 = 0x0143;
const CB_GETCURSEL_: u32 = 0x0147;
const CB_SETCURSEL_: u32 = 0x014E;

thread_local! {
    static OUTPUT: Cell<HWND> = const { Cell::new(null_mut()) };
    static PASS: Cell<HWND> = const { Cell::new(null_mut()) };
    static INPUT: Cell<HWND> = const { Cell::new(null_mut()) };
    static COMBO: Cell<HWND> = const { Cell::new(null_mut()) };
    static LINK: Cell<HWND> = const { Cell::new(null_mut()) };
    static BRUSH_OUT: Cell<HBRUSH> = const { Cell::new(null_mut()) };
    static LICENSE_TEXT: RefCell<String> = const { RefCell::new(String::new()) };
}

fn get_text(h: HWND) -> String {
    unsafe {
        let n = GetWindowTextLengthW(h);
        if n <= 0 {
            return String::new();
        }
        let mut buf = vec![0u16; n as usize + 1];
        GetWindowTextW(h, buf.as_mut_ptr(), n + 1);
        from_w(&buf)
    }
}

fn set_cue(h: HWND, text: &str) {
    unsafe {
        SendMessageW(h, EM_SETCUEBANNER_, 1, w(text).as_ptr() as LPARAM);
    }
}

/// `CloseGUI()`: cleans traces and quits.
fn close_gui() -> ! {
    clean::clean_all(SCRIPT_FULL.get().map(|s| s.as_str()).unwrap_or(""), None);
    std::process::exit(0)
}

// ----------------------------------------------------------------- -crypt

fn crypt_change() {
    let pass = get_text(PASS.with(|c| c.get()));
    let input = get_text(INPUT.with(|c| c.get()));
    let sel = unsafe { SendMessageW(COMBO.with(|c| c.get()), CB_GETCURSEL_, 0, 0) };
    let out = if !pass.is_empty() && !input.is_empty() {
        if sel == 1 {
            // Decrypt
            crypto::decrypt(&input, &pass).unwrap_or_default()
        } else {
            crypto::encrypt(&input, &pass)
        }
    } else {
        String::new()
    };
    unsafe {
        SetWindowTextW(OUTPUT.with(|c| c.get()), w(&out).as_ptr());
    }
}

unsafe extern "system" fn crypt_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_COMMAND => {
            let id = (wp & 0xFFFF) as i32;
            let code = ((wp >> 16) & 0xFFFF) as u32;
            match id {
                ID_PASS | ID_INPUT if code == EN_CHANGE_ => crypt_change(),
                ID_COMBO if code == CBN_SELCHANGE_ => crypt_change(),
                ID_COPY => {
                    // The original re-read the field (CryptChange) and only copied if it was not empty.
                    crypt_change();
                    let t = get_text(OUTPUT.with(|c| c.get()));
                    if !t.is_empty() {
                        ui::clip_put(&t);
                    }
                }
                _ => {}
            }
            return 0;
        }
        WM_CTLCOLORSTATIC => {
            let h = lp as HWND;
            let hdc = wp as HDC;
            if h == OUTPUT.with(|c| c.get()) {
                SetBkColor(hdc, 0x00E1FFFF); // 0xFFFFE1 (RGB) as COLORREF
                return BRUSH_OUT.with(|b| b.get()) as LRESULT;
            }
            SetBkMode(hdc, TRANSPARENT as i32);
            return GetStockObject(WHITE_BRUSH) as LRESULT;
        }
        WM_CLOSE => close_gui(),
        WM_DESTROY => {
            PostQuitMessage(0);
            return 0;
        }
        _ => {}
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

pub fn run_crypt(script_full: &str) -> ! {
    let _ = SCRIPT_FULL.set(script_full.to_string());
    unsafe {
        ui::init_common();
        let title = format!("Encryption and Decryption Module - {}", PROGRAM_NAME);
        let hwnd = make_window("gaPCryptWnd", &title, 530, 262, Some(crypt_proc), 0x00FF_FFFF, WS_DEFAULT_GUI);
        BRUSH_OUT.with(|b| b.set(CreateSolidBrush(0x00E1FFFF)));
        let tab = WS_TABSTOP;
        make_ctrl(hwnd, "STATIC", "Password:", 0, 0, 8, 18, 56, 17, -1);
        let pass = make_ctrl(hwnd, "EDIT", "", tab | ES_AUTOHSCROLL as u32, WS_EX_CLIENTEDGE, 70, 16, 270, 21, ID_PASS);
        let combo = make_ctrl(hwnd, "COMBOBOX", "", tab | WS_VSCROLL | CBS_DROPDOWNLIST as u32, 0, 360, 16, 161, 120, ID_COMBO);
        make_ctrl(hwnd, "STATIC", "Input text:", 0, 0, 8, 50, 60, 17, -1);
        let input = make_ctrl(hwnd, "EDIT", "", tab | ES_AUTOHSCROLL as u32, WS_EX_CLIENTEDGE, 70, 48, 451, 21, ID_INPUT);
        make_ctrl(hwnd, "STATIC", "Output text:", 0, 0, 8, 82, 60, 17, -1);
        let output = make_ctrl(
            hwnd, "EDIT", "", tab | ES_AUTOHSCROLL as u32 | ES_READONLY as u32, WS_EX_CLIENTEDGE, 70, 80, 451, 21, ID_OUTPUT,
        );
        make_ctrl(hwnd, "BUTTON", "Copy to clipboard", tab | BS_PUSHBUTTON as u32, 0, 392, 112, 129, 85, ID_COPY);
        let warn = make_ctrl(
            hwnd, "STATIC", "WARNING: This encryption module uses AES 256 bit algorithm to encrypt or decrypt your data. There is no perfect encryption available and gaP encryption and decryption module IS NOT an exception. However, choosing a strong password will make decoding very hard for the average computer user.\r\n\r\nWARNING: Even if you launch the main application with encrypted command line parameters, gaP will pass unencrypted data to the application which may be visible to other applications.",
            0, 0, 8, 110, 370, 144, -1,
        );
        // Font 7 pt, bold.
        let hdc = GetDC(null_mut());
        let dpi = GetDeviceCaps(hdc, LOGPIXELSY as i32);
        ReleaseDC(null_mut(), hdc);
        let font = CreateFontW(
            -(7 * dpi / 72), 0, 0, 0, 700, 0, 0, 0, 1, 0, 0, 0, 0, w("MS Shell Dlg").as_ptr(),
        );
        SendMessageW(warn, WM_SETFONT, font as WPARAM, 1);

        for s in ["Encrypt", "Decrypt"] {
            SendMessageW(combo, CB_ADDSTRING_, 0, w(s).as_ptr() as LPARAM);
        }
        SendMessageW(combo, CB_SETCURSEL_, 0, 0);
        set_cue(pass, "Write the password to encrypt or decrypt here.");
        set_cue(input, "Write the text to encrypt or decrypt here.");
        set_cue(output, "Read the encrypted or decrypted text here.");
        PASS.with(|c| c.set(pass));
        INPUT.with(|c| c.set(input));
        OUTPUT.with(|c| c.set(output));
        COMBO.with(|c| c.set(combo));

        ShowWindow(hwnd, SW_SHOW);
        SetFocus(pass);
        let code = ui::run_loop(hwnd);
        std::process::exit(code)
    }
}

// --------------------------------------------------------------- -license

/// The LICENSE file of the repository, embedded in the program: "Save license" writes the text as it is on GitHub.
const LICENSE_FILE: &str = include_str!("../LICENSE");

/// The license as it is saved: the lines of the LICENSE file, with Windows line ends.
pub fn license_text() -> String {
    format!("{}\r\n", LICENSE_FILE.replace("\r\n", "\n").trim_end().replace('\n', "\r\n"))
}

/// The license as it is shown in the window: one line per paragraph (the control wraps it to the width of the window).
fn license_display() -> String {
    LICENSE_FILE
        .replace("\r\n", "\n")
        .trim_end()
        .split("\n\n")
        .map(|p| p.replace('\n', " "))
        .collect::<Vec<_>>()
        .join("\r\n\r\n")
}

unsafe extern "system" fn license_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_COMMAND if (wp & 0xFFFF) as i32 == ID_SAVE => {
            let dir = SCRIPT_DIR.get().cloned().unwrap_or_default();
            let path = format!("{}\\License.txt", dir.trim_end_matches('\\'));
            let text = LICENSE_TEXT.with(|t| t.borrow().clone());
            // AutoIt mode 42 = overwrite + create folder + UTF-16 LE with BOM
            if util::write_text(&path, &text, util::Enc::Utf16Le).is_err() {
                ui::info(
                    &format!("License - {}", PROGRAM_NAME),
                    "An error occurred when creating or reading the License file.",
                );
            }
            return 0;
        }
        WM_CTLCOLORSTATIC => {
            SetBkMode(wp as HDC, TRANSPARENT as i32);
            return GetStockObject(WHITE_BRUSH) as LRESULT;
        }
        WM_CLOSE => close_gui(),
        WM_DESTROY => {
            PostQuitMessage(0);
            return 0;
        }
        _ => {}
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

pub fn run_license(script_full: &str, script_dir: &str) -> ! {
    let _ = SCRIPT_FULL.set(script_full.to_string());
    let _ = SCRIPT_DIR.set(script_dir.to_string());
    let text = license_text();
    LICENSE_TEXT.with(|t| *t.borrow_mut() = text.clone());
    unsafe {
        ui::init_common();
        let title = format!("License - {}", PROGRAM_NAME);
        let hwnd = make_window("gaPLicenseWnd", &title, 530, 345, Some(license_proc), 0x00FF_FFFF, WS_DEFAULT_GUI);
        make_ctrl(hwnd, "STATIC", &license_display(), 0, 0, 8, 8, 515, 290, -1);
        make_ctrl(hwnd, "BUTTON", "Save license", WS_TABSTOP | BS_DEFPUSHBUTTON as u32, 0, 200, 305, 130, 30, ID_SAVE);
        ShowWindow(hwnd, SW_SHOW);
        let code = ui::run_loop(hwnd);
        std::process::exit(code)
    }
}

// ------------------------------------------------------------------ -about

unsafe extern "system" fn about_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_COMMAND => {
            if (wp & 0xFFFF) as i32 == ID_LINK {
                ui::shell_open(HOMEPAGE);
            }
            return 0;
        }
        WM_SETCURSOR => {
            if wp as HWND == LINK.with(|c| c.get()) {
                SetCursor(LoadCursorW(null_mut(), IDC_HAND));
                return 1;
            }
        }
        WM_CTLCOLORSTATIC => {
            let hdc = wp as HDC;
            if lp as HWND == LINK.with(|c| c.get()) {
                SetTextColor(hdc, 0x00C0_0000); // 0x0000C0 (RGB) as COLORREF
            }
            SetBkMode(hdc, TRANSPARENT as i32);
            return GetStockObject(WHITE_BRUSH) as LRESULT;
        }
        WM_CLOSE => close_gui(),
        WM_DESTROY => {
            PostQuitMessage(0);
            return 0;
        }
        _ => {}
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

pub fn run_about(script_full: &str) -> ! {
    let _ = SCRIPT_FULL.set(script_full.to_string());
    unsafe {
        ui::init_common();
        let title = format!("About - {}", PROGRAM_NAME);
        let hwnd = make_window("gaPAboutWnd", &title, 390, 178, Some(about_proc), 0x00FF_FFFF, WS_DEFAULT_GUI);
        make_ctrl(hwnd, "STATIC", "ABOUT", 0, 0, 8, 8, 47, 13, -1);
        make_ctrl(hwnd, "STATIC", &format!("Program name: {}", PROGRAM_NAME), 0, 0, 8, 32, 220, 21, -1);
        make_ctrl(hwnd, "STATIC", &format!("Program version: {}", PROGRAM_VERSION), 0, 0, 8, 55, 167, 21, -1);
        make_ctrl(hwnd, "STATIC", "Author: Bruno Eduardo", 0, 0, 8, 79, 130, 21, -1);
        make_ctrl(hwnd, "STATIC", "Collaborators: tproli and all The Portable Freeware Collection Forums members", 0, 0, 8, 102, 380, 13, -1);
        make_ctrl(hwnd, "STATIC", "Credits: AutoIt creators, AutoIt Forums members and Claude (Rust port)", 0, 0, 8, 125, 380, 13, -1);
        make_ctrl(hwnd, "STATIC", "gaP GitHub:", 0, 0, 8, 149, 64, 17, -1);
        let link = make_ctrl(hwnd, "STATIC", "gaP GitHub", SS_NOTIFY as u32, 0, 70, 149, 85, 17, ID_LINK);
        LINK.with(|c| c.set(link));
        let font = CreateFontW(-ui::scale(11), 0, 0, 0, 700, 0, 1, 0, 1, 0, 0, 0, 0, w("MS Sans Serif").as_ptr());
        SendMessageW(link, WM_SETFONT, font as WPARAM, 1);
        // 80x80 logo from the embedded icon (the original used a BMP).
        let logo = make_ctrl(hwnd, "STATIC", "", SS_ICON as u32, 0, 300, 8, 80, 80, -1);
        // LoadIconWithScaleDown reduces the 256x256 image of the icon to 80x80 with smoothing; LoadImageW would
        // pick the 48x48 one and enlarge it (blurry). LoadImageW stays as the fallback.
        let mut icon: HICON = null_mut();
        let res_id = std::ptr::without_provenance::<u16>(1);
        if windows_sys::Win32::UI::Controls::LoadIconWithScaleDown(ui::hinstance(), res_id, ui::scale(80), ui::scale(80), &mut icon) < 0 {
            icon = LoadImageW(ui::hinstance(), res_id, IMAGE_ICON, ui::scale(80), ui::scale(80), LR_DEFAULTCOLOR);
        }
        if !icon.is_null() {
            SendMessageW(logo, STM_SETIMAGE, IMAGE_ICON as WPARAM, icon as LPARAM);
        }
        ShowWindow(hwnd, SW_SHOW);
        let code = ui::run_loop(hwnd);
        std::process::exit(code)
    }
}
