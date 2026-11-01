//! Limpeza de rastros no registro: "MuiCache" e "App Paths"
//! (`CleanMUICache` / `CleanAppPaths` / keyword `clean`).

use std::sync::Mutex;

use crate::{reg, sys, util};

static MAIN_APP: Mutex<Option<String>> = Mutex::new(None);

/// Registra o aplicativo principal (equivale a `IsDeclared("MainApp")`).
pub fn set_main_app(p: &str) {
    if let Ok(mut g) = MAIN_APP.lock() {
        *g = Some(p.to_string());
    }
}

fn main_app() -> Option<String> {
    MAIN_APP.lock().ok().and_then(|g| g.clone())
}

fn mui_key() -> &'static str {
    if sys::os_version().0 >= 6 {
        "HKCU\\Software\\Classes\\Local Settings\\Software\\Microsoft\\Windows\\Shell\\MuiCache"
    } else {
        "HKCU\\Software\\Microsoft\\Windows\\ShellNoRoam\\MUICache"
    }
}

fn clean_mui_one(path: &str) {
    let key = mui_key();
    // O original removia só o valor "<caminho>"; o Windows moderno também grava
    // "<caminho>.FriendlyAppName" e "<caminho>.ApplicationCompany".
    for suffix in ["", ".FriendlyAppName", ".ApplicationCompany"] {
        let name = format!("{}{}", path, suffix);
        if reg::value_exists(key, &name, reg::View::Native) {
            reg::delete_value(key, &name, reg::View::Native);
        }
    }
}

fn clean_app_paths_one(path: &str) {
    let name = util::file_name(path);
    if !util::has_extension(&name) {
        return;
    }
    let rel = format!("Software\\Microsoft\\Windows\\CurrentVersion\\App Paths\\{}", name);
    for (root, views) in [
        ("HKCU", vec![reg::View::Native]),
        ("HKLM", vec![reg::View::X64, reg::View::X86]),
    ] {
        for v in views {
            let key = format!("{}\\{}", root, rel);
            if let Some(def) = reg::read_string(&key, "", v) {
                if util::contains_ic(&def, path) {
                    reg::delete_tree(&key, v);
                }
            }
        }
    }
}

/// Limpeza automática: o próprio gaP e (se já conhecido) o aplicativo principal.
pub fn clean_all(script_full: &str) {
    clean_mui_one(script_full);
    clean_app_paths_one(script_full);
    if let Some(m) = main_app() {
        clean_mui_one(&m);
        clean_app_paths_one(&m);
    }
}

/// Keyword `clean = <exe>`: somente o executável indicado.
pub fn clean_file(path: &str) {
    clean_mui_one(path);
    clean_app_paths_one(path);
}
