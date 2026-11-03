//! gaP: great! another Portabilizer — Rust port of the AutoIt program
//! (v2.1.2.0; the last AutoIt version was 1.1.5.0).
#![windows_subsystem = "windows"]
#![allow(clippy::too_many_arguments)]

mod checks;
mod clean;
mod crypto;
mod ctx;
mod gui;
mod ini;
mod ipc;
mod ops_fs;
mod ops_misc;
mod ops_reg;
mod reg;
mod sys;
mod tasks;
mod ui;
mod util;
mod vars;

use ctx::{exit_code, exit_now, Ctx, Section, EXIT_CANCELLED, EXIT_CONFIG};
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
        // FileInstall(..., flag 0): does not overwrite.
        let p = format!("{}\\gaP_Help.txt", ctx.script_dir.trim_end_matches('\\'));
        if !std::path::Path::new(&p).exists() {
            let _ = std::fs::write(&p, HELP_TXT);
        }
        clean::clean_all(&ctx.script_full, None);
        exit_now();
    }

    launcher(&mut ctx);
}

/// Normal flow (`Case Else` of the original Switch).
fn launcher(ctx: &mut Ctx) {
    let has_ini = util::is_file(&ctx.ini_path);
    if has_ini {
        // The .ini is read once; every `[GENERAL]` key and both task sections come from this copy.
        ctx.ini = ini::IniDoc::load(&ctx.ini_path);
    }
    checks::read_only_init(ctx);

    if has_ini {
        if std::fs::File::open(&ctx.ini_path).is_err() {
            ctx.msg(&format!("An error occurred when opening and reading the \"{}.ini\" file.", ctx.exe_name));
            exit_code(EXIT_CONFIG);
        }
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
        // No .ini and none created is a configuration problem, unless the user refused to create it: that is a
        // question that was cancelled.
        let mut code = EXIT_CONFIG;
        if !ctx.is_readonly {
            if ui::ok_cancel(&ctx.title, "No \".ini\" file found. Do you want to create a new one?") {
                // IMPROVEMENT: the original ignored the failure to create the .ini.
                match std::fs::write(&ctx.ini_path, INI_TEMPLATE) {
                    Ok(()) => code = 0,
                    Err(_) => ctx.msg(&format!("The \"{}.ini\" file could not be created.", ctx.exe_name)),
                }
            } else {
                code = EXIT_CANCELLED;
            }
        }
        clean::clean_all(&ctx.script_full, None);
        exit_code(code);
    }

    clean_traces(ctx);

    ctx.before = tasks::init_section(ctx, Section::Before);
    ctx.after = tasks::init_section(ctx, Section::After);

    tasks::process_before(ctx);

    run_main_app(ctx);

    clean_traces(ctx);

    if ctx.run_after {
        tasks::process_after(ctx);
    }
    clean_traces(ctx);
}

/// Cleans the registry traces of gaP and of the main application (once it is known).
fn clean_traces(ctx: &Ctx) {
    clean::clean_all(&ctx.script_full, ctx.main_app.as_deref());
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

/// `RunMainAppProc`: starts the main application and waits for it. While it runs, command lines sent by other
/// instances (`!multiple`) start it again.
fn run_main_app(ctx: &mut Ctx) {
    start_main(ctx);
    if !ctx.wait_main {
        return;
    }
    loop {
        let running = sys::running_children();
        if running.is_empty() {
            break;
        }
        // Sleeps until a process ends or another instance sends a command line. A single wait takes up to 63
        // processes: with more, it also wakes up twice a second to look at the rest.
        let event = ctx.ipc.as_ref().map(|s| s.event());
        sys::wait_any(&running, event, if running.len() > 63 { 500 } else { sys::FOREVER });
        let mut received = Vec::new();
        if let Some(server) = &ctx.ipc {
            while let Some(cmd) = server.try_recv() {
                received.push(cmd);
            }
        }
        for cmd in received {
            ctx.cmd_raw = cmd;
            start_main(ctx);
        }
    }
    if !ctx.waitprocess.is_empty() {
        while sys::window_title_exists(&ctx.waitprocess) {
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
    }
    log_leftovers(ctx);
}

/// Debug log: what still runs of the processes gaP started at the moment it decides the main application is closed
/// and runs the AFTER section. A process listed here can still have the files and registry keys the AFTER section
/// is going to change.
fn log_leftovers(ctx: &Ctx) {
    if ctx.debug.is_none() {
        return;
    }
    let left = sys::launched_descendants();
    if left.is_empty() {
        ctx.log("[Main application closed] nothing started by gaP is running");
        return;
    }
    ctx.log("[Main application closed] still running (pid, parent pid, name):");
    for (pid, parent, name) in left {
        ctx.log(&format!("  {}, {}, {}", pid, parent, name));
    }
}
