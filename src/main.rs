//! gaP: great! another Portabilizer — Rust port of the AutoIt program
//! (v2.0.0.0; the last AutoIt version was 1.1.5.0).
#![windows_subsystem = "windows"]
#![allow(clippy::too_many_arguments)]

mod checks;
mod clean;
mod crypto;
mod ctx;
mod gui;
mod ini;
mod langtable;
mod ops_fs;
mod ops_misc;
mod ops_reg;
mod reg;
mod sys;
mod tasks;
mod ui;
mod util;
mod vars;

use ctx::{exit_now, Ctx, Section};
use util::eq_ic;

static HELP_TXT: &[u8] = include_bytes!("../res/gaP_Help.txt");
static INI_TEMPLATE: &[u8] = include_bytes!("../res/gaP.ini");

fn main() {
    let mut ctx = Ctx::new();
    let cmd = ctx.cmd_raw.clone();

    if eq_ic(&cmd, "-crypt") {
        gui::run_crypt(&ctx.script_full);
    } else if eq_ic(&cmd, "-license") {
        gui::run_license(&ctx.script_full, &ctx.script_dir);
    } else if eq_ic(&cmd, "-about") {
        gui::run_about(&ctx.script_full);
    } else if eq_ic(&cmd, "-help") {
        sys::reduce_memory();
        // FileInstall(..., flag 0): does not overwrite.
        let p = format!("{}\\gaP_Help.txt", ctx.script_dir.trim_end_matches('\\'));
        if !std::path::Path::new(&p).exists() {
            let _ = std::fs::write(&p, HELP_TXT);
        }
        clean::clean_all(&ctx.script_full);
        exit_now();
    }

    launcher(&mut ctx);
}

/// Normal flow (`Case Else` of the original Switch).
fn launcher(ctx: &mut Ctx) {
    sys::reduce_memory();
    checks::read_only_init(ctx);

    if util::is_file(&ctx.ini_path) {
        if std::fs::File::open(&ctx.ini_path).is_err() {
            ctx.msg(&format!("An error occurred when opening and reading the \"{}.ini\" file.", ctx.exe_name));
            exit_now();
        }
        checks::java_early(ctx);
        checks::main_app_init(ctx);
        checks::cmdline_init(ctx);
        checks::workdir_init(ctx);
        checks::multiple_init(ctx);
        checks::debug_log_init(ctx);
        checks::admin_init(ctx);
        checks::dotnet_init(ctx);
        checks::java_full(ctx);
        checks::online_init(ctx);
        checks::os_init(ctx);
        checks::wait_init(ctx);
    } else {
        if !ctx.is_readonly
            && ui::ok_cancel(&ctx.title, "No \".ini\" file found. Do you want to create a new one?")
            && std::fs::write(&ctx.ini_path, INI_TEMPLATE).is_err()
        {
            // IMPROVEMENT: the original ignored the failure to create the .ini.
            ctx.msg(&format!("The \"{}.ini\" file could not be created.", ctx.exe_name));
        }
        clean::clean_all(&ctx.script_full);
        exit_now();
    }

    sys::reduce_memory();
    clean::clean_all(&ctx.script_full);

    ctx.before = tasks::init_section(ctx, Section::Before);
    ctx.after = tasks::init_section(ctx, Section::After);
    sys::reduce_memory();

    tasks::process_before(ctx);
    sys::reduce_memory();

    run_main_app(ctx);

    sys::reduce_memory();
    clean::clean_all(&ctx.script_full);

    if ctx.run_after {
        tasks::process_after(ctx);
    }
    clean::clean_all(&ctx.script_full);
}

fn main_command_line(ctx: &Ctx) -> String {
    // The path gaP launches from: in `System32` it is the one that matches the application.
    let app = ctx.io_path(&ctx.main_app.clone().unwrap_or_default());
    let mut cl = format!("\"{}\"", app);
    for part in [ctx.main_cmdline.as_str(), ctx.cmd_raw.as_str()] {
        if !part.is_empty() {
            cl.push(' ');
            cl.push_str(part);
        }
    }
    cl
}

fn start_main(ctx: &Ctx) {
    let cl = main_command_line(ctx);
    if let Err(e) = sys::launch(&cl, &ctx.main_workdir, ctx.show_state, false) {
        // IMPROVEMENT: the original ignored the return value of Run().
        ctx.msg(&format!(
            "The main application \"{}\" could not be started (error {}).",
            ctx.main_app.clone().unwrap_or_default(),
            e
        ));
    }
}

/// Reads (and consumes) the `@#$gapnew@#$` file left by another instance.
fn take_new_cmdline(path: &str, remove_dir: bool) -> Option<String> {
    if !std::path::Path::new(path).is_file() {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    if remove_dir {
        if let Some(par) = std::path::Path::new(path).parent() {
            let _ = std::fs::remove_dir_all(par);
        }
    } else {
        let _ = std::fs::remove_file(path);
    }
    Some(util::ansi_to_string(&bytes))
}

/// `RunMainAppProc`.
fn run_main_app(ctx: &mut Ctx) {
    start_main(ctx);
    if !ctx.wait_main {
        return;
    }
    let (local, shared) = checks::gapnew_paths(ctx, std::process::id());
    while !sys::live_children().is_empty() {
        std::thread::sleep(std::time::Duration::from_millis(250));
        let mut new = take_new_cmdline(&local, false);
        if new.is_none() {
            new = take_new_cmdline(&shared, true);
        }
        if let Some(n) = new {
            ctx.cmd_raw = n;
            start_main(ctx);
        }
    }
    if !ctx.waitprocess.is_empty() {
        while sys::window_title_exists(&ctx.waitprocess) {
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
    }
}
