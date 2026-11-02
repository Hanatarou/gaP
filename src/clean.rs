//! Cleaning of registry traces: "MuiCache" and "App Paths"
//! (`CleanMUICache` / `CleanAppPaths` / keyword `clean`).

use crate::{reg, util};

/// MuiCache of the current user (Windows 10 and 11 keep it here).
const MUI_KEY: &str = "HKCU\\Software\\Classes\\Local Settings\\Software\\Microsoft\\Windows\\Shell\\MuiCache";

fn clean_mui_one(path: &str) {
    // The original removed only the "<path>" value; modern Windows also writes
    // "<path>.FriendlyAppName" and "<path>.ApplicationCompany".
    let names = [path.to_string(), format!("{}.FriendlyAppName", path), format!("{}.ApplicationCompany", path)];
    reg::delete_values(MUI_KEY, &names, reg::View::Native);
}

fn clean_app_paths_one(path: &str) {
    let name = util::file_name(path);
    if !util::has_extension(&name) {
        return;
    }
    let rel = format!("Software\\Microsoft\\Windows\\CurrentVersion\\App Paths\\{}", name);
    for (root, views) in [
        ("HKCU", &[reg::View::Native][..]),
        ("HKLM", &[reg::View::X64, reg::View::X86][..]),
    ] {
        for v in views {
            let key = format!("{}\\{}", root, rel);
            if let Some(def) = reg::read_string(&key, "", *v) {
                if util::contains_ic(&def, path) {
                    let _ = reg::delete_tree(&key, *v);
                }
            }
        }
    }
}

/// Automatic cleaning: gaP itself and (if already known) the main application.
pub fn clean_all(script_full: &str, main_app: Option<&str>) {
    clean_file(script_full);
    if let Some(m) = main_app {
        clean_file(m);
    }
}

/// Keyword `clean = <exe>`: only the given executable.
pub fn clean_file(path: &str) {
    clean_mui_one(path);
    clean_app_paths_one(path);
}
