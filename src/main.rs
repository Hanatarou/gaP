//! gaP: great! another Portabilizer — Rust port of the AutoIt program
//! (v2.1.3.0; the last AutoIt version was 1.1.5.0).
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
        if !std::path::Path::new(&p).exists() && std::fs::write(&p, HELP_TXT).is_err() {
            ctx.msg(&format!("The help file \"{}\" could not be created.", p));
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
        if !ctx.ini.existed {
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
        if ctx.is_readonly {
            ctx.msg(&format!("The \"{}.ini\" file does not exist and can't be created: the drive is read only.", ctx.exe_name));
        } else {
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
    ctx::log_end(0);
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
    match sys::launch(&cl, &ctx.main_workdir, ctx.show_state, false) {
        Ok(()) => ctx.ok(&format!("launch {} (working folder \"{}\")", cl, ctx.main_workdir)),
        Err(e) => start_failed(ctx, &cl, e),
    }
}

fn start_failed(ctx: &Ctx, cl: &str, e: u32) {
    // IMPROVEMENT: the original ignored the return value of Run().
    ctx.err(&format!("launch {} failed (error {})", cl, e));
    ctx.msg(&format!(
        "The main application \"{}\" could not be started (error {}).",
        ctx.main_app.clone().unwrap_or_default(),
        e
    ));
}

/// The command lines that other instances sent and gaP did not start yet.
fn take_commands(ctx: &Ctx) -> Vec<String> {
    let mut received = Vec::new();
    if let Some(server) = &ctx.ipc {
        while let Some(cmd) = server.try_recv() {
            received.push(cmd);
        }
    }
    received
}

/// `RunMainAppProc`: starts the main application and waits for it. While it runs, command lines sent by other
/// instances (`!multiple`) start it again.
fn run_main_app(ctx: &mut Ctx) {
    ctx.phase.set("RUN");
    start_main(ctx);
    if !ctx.wait_main {
        ctx.log("the main application is not waited for (!nowait)");
        return;
    }
    loop {
        let running = sys::running_children();
        if running.is_empty() {
            // A command line sent by another instance just before the last process ended was accepted, so it runs.
            let late = take_commands(ctx);
            if late.is_empty() {
                break;
            }
            for cmd in late {
                ctx.cmd_raw = cmd;
                start_main(ctx);
            }
            continue;
        }
        // Sleeps until a process ends or another instance sends a command line. A single wait takes up to 63
        // processes: with more, it also wakes up twice a second to look at the rest.
        let event = ctx.ipc.as_ref().map(|s| s.event());
        sys::wait_any(&running, event, if running.len() > 63 { 500 } else { sys::FOREVER });
        for cmd in take_commands(ctx) {
            ctx.cmd_raw = cmd;
            start_main(ctx);
        }
    }
    if !ctx.waitprocess.is_empty() {
        ctx.phase.set("WAIT");
        ctx.log(&format!("waiting for a window or process \"{}\"", ctx.waitprocess));
        while sys::window_title_exists(&ctx.waitprocess) || sys::process_exists(&ctx.waitprocess) {
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        ctx.ok("nothing matches \"!waitprocess\" any more");
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
