//! Engine of the BEFORE / AFTER sections: .ini reading, x86/x64 filter,
//! keyword normalization, section/parameter validation and execution.

use crate::ctx::{error_count, exit_code, Ctx, Prog, Section, TaskKind, EXIT_BACKUP};
use crate::{clean, ops_fs, ops_misc, ops_reg, util};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Allowed {
    Both,
    BeforeOnly,
    AfterOnly,
}

struct Spec {
    name: &'static str,
    min: usize,
    max: usize,
    allowed: Allowed,
}

use Allowed::*;

/// Keyword table (internal name, min/max number of segments, section).
const SPECS: &[Spec] = &[
    Spec { name: "clean", min: 1, max: 1, allowed: Both },
    Spec { name: "dir", min: 1, max: 2, allowed: BeforeOnly },
    Spec { name: "file", min: 1, max: 2, allowed: BeforeOnly },
    Spec { name: "regkey", min: 1, max: 3, allowed: BeforeOnly },
    Spec { name: "regvalue", min: 1, max: 3, allowed: BeforeOnly },
    Spec { name: "pdirp", min: 1, max: 1, allowed: BeforeOnly },
    Spec { name: "pfilep", min: 1, max: 1, allowed: BeforeOnly },
    Spec { name: "pregkeyp", min: 1, max: 2, allowed: BeforeOnly },
    Spec { name: "pregvaluep", min: 1, max: 2, allowed: BeforeOnly },
    Spec { name: "rtldir", min: 1, max: 3, allowed: Both },
    Spec { name: "ltrdir", min: 1, max: 3, allowed: Both },
    Spec { name: "rtlfile", min: 1, max: 3, allowed: Both },
    Spec { name: "ltrfile", min: 1, max: 3, allowed: Both },
    Spec { name: "rtlregkey", min: 2, max: 4, allowed: Both },
    Spec { name: "ltrregkey", min: 2, max: 4, allowed: Both },
    Spec { name: "rtlregvalue", min: 2, max: 4, allowed: Both },
    Spec { name: "ltrregvalue", min: 2, max: 4, allowed: Both },
    Spec { name: "plusdir", min: 1, max: 1, allowed: Both },
    Spec { name: "plusfile", min: 1, max: 3, allowed: Both },
    Spec { name: "plusregkey", min: 1, max: 2, allowed: Both },
    Spec { name: "plusregvalue", min: 3, max: 6, allowed: Both },
    Spec { name: "minusdir", min: 1, max: 2, allowed: Both },
    Spec { name: "minusfile", min: 1, max: 1, allowed: Both },
    Spec { name: "minusregkey", min: 1, max: 3, allowed: Both },
    Spec { name: "minusregvalue", min: 2, max: 3, allowed: Both },
    Spec { name: "attributes", min: 1, max: 2, allowed: Both },
    Spec { name: "batch", min: 1, max: 2, allowed: Both },
    Spec { name: "clip", min: 1, max: 1, allowed: Both },
    Spec { name: "date", min: 1, max: 3, allowed: Both },
    Spec { name: "delay", min: 1, max: 1, allowed: Both },
    Spec { name: "envvar", min: 2, max: 2, allowed: Both },
    Spec { name: "giniread", min: 4, max: 5, allowed: Both },
    Spec { name: "giniwrite", min: 2, max: 4, allowed: Both },
    Spec { name: "keeponly", min: 6, max: 6, allowed: Both },
    Spec { name: "killprocess", min: 1, max: 1, allowed: Both },
    Spec { name: "message", min: 1, max: 2, allowed: Both },
    Spec { name: "nowait", min: 0, max: 1, allowed: Both },
    Spec { name: "quit", min: 0, max: 1, allowed: Both },
    Spec { name: "regdll", min: 1, max: 2, allowed: Both },
    Spec { name: "regexport", min: 1, max: 3, allowed: Both },
    Spec { name: "regimport", min: 0, max: 2, allowed: Both },
    Spec { name: "regmerge", min: 2, max: 2, allowed: Both },
    Spec { name: "replace", min: 3, max: 3, allowed: Both },
    Spec { name: "replaceline", min: 3, max: 3, allowed: Both },
    Spec { name: "restore", min: 0, max: 1, allowed: AfterOnly },
    Spec { name: "grun", min: 1, max: 4, allowed: Both },
    Spec { name: "service", min: 2, max: 4, allowed: Both },
    Spec { name: "gstringreplace", min: 4, max: 4, allowed: Both },
    Spec { name: "symlink", min: 2, max: 4, allowed: BeforeOnly },
    Spec { name: "systemrefresh", min: 0, max: 1, allowed: Both },
    Spec { name: "uservar", min: 2, max: 2, allowed: Both },
];

const RUN_FIRST_BEFORE: &[&str] = &["uservar", "envvar", "giniread", "gstringreplace"];
const RUN_FIRST_AFTER: &[&str] = &["nowait", "quit", "uservar", "envvar", "giniread", "gstringreplace"];

/// Applies the x86/x64 filter and converts "->dir" into "ltrdir", etc.
/// `None` = item discarded because of the OS architecture.
fn normalize(raw_key: &str, os_arch: &str) -> Option<String> {
    let has86 = util::contains_ic(raw_key, "x86");
    let has64 = util::contains_ic(raw_key, "x64");
    let mut k = raw_key.to_string();
    match (os_arch, has86, has64) {
        ("x86", true, false) => k = util::replace_all_ic(&k, "x86", ""),
        ("x64", false, true) => k = util::replace_all_ic(&k, "x64", ""),
        (_, false, false) => {}
        _ => return None,
    }
    let k = k.trim().to_lowercase();
    let k = k.replace(['(', ')'], "p").replace("->", "ltr").replace("<-", "rtl").replace('-', "minus").replace('+', "plus");
    Some(match k.as_str() {
        "stringreplace" => "gstringreplace".to_string(),
        "run" => "grun".to_string(),
        "iniread" => "giniread".to_string(),
        "iniwrite" => "giniwrite".to_string(),
        _ => k,
    })
}

/// `BeforeSecIni` / `AfterSecIni`.
pub fn init_section(ctx: &mut Ctx, sec: Section) -> Vec<TaskKind> {
    let pairs = ctx.ini.section_pairs(sec.name()).unwrap_or_default();
    ctx.raw(&format!("[{} Section Tasks List - Raw]", sec.name()));
    for (k, v) in &pairs {
        ctx.raw(&format!("{}|{}", k, v));
    }
    let mut out = Vec::new();
    for (k, v) in pairs {
        match normalize(&k, ctx.os_arch) {
            None => out.push(TaskKind::Done),
            Some(kw) => {
                let args = if v.is_empty() { Vec::new() } else { util::split_args(&v) };
                out.push(TaskKind::User { kw, args, raw_key: k, raw_val: v });
            }
        }
    }
    ctx.raw(&format!("[{} Section Tasks List - Initialized]", sec.name()));
    for t in &out {
        if let TaskKind::User { kw, raw_val, .. } = t {
            ctx.raw(&format!("{}|{}", kw, raw_val));
        }
    }
    out
}

fn run_prog(ctx: &mut Ctx, p: &Prog) {
    ctx.log(&format!("PROG {:?}", p));
    match p {
        Prog::LtrDir { .. } | Prog::LtrFile { .. } => ops_fs::run_prog_fs(ctx, p),
        Prog::SymlinkRemove { link, is_dir } => ops_misc::remove_symlink(link, *is_dir),
        _ => ops_reg::run_prog_reg(ctx, p),
    }
}

/// Runs a user item and says in the debug log how it went.
fn exec_user(ctx: &mut Ctx, sec: Section, kw: &str, args: &[String], raw_key: &str) {
    let before = error_count();
    ctx.log(&format!("{} :: {:?}", kw, args));
    exec_user_item(ctx, sec, kw, args, raw_key);
    match error_count() - before {
        0 => ctx.ok(&format!("{} done", kw)),
        n => ctx.err(&format!("{} finished with {} error(s)", kw, n)),
    }
}

/// Runs a user item (validating section and number of parameters).
fn exec_user_item(ctx: &mut Ctx, sec: Section, kw: &str, args: &[String], raw_key: &str) {
    let Some(spec) = SPECS.iter().find(|s| s.name == kw) else {
        ctx.msg(&format!("Unknown keyword \"{}\" in the \"{}\" section was ignored.", raw_key, sec.name()));
        return;
    };
    let ok_sec = match spec.allowed {
        Both => true,
        BeforeOnly => sec == Section::Before,
        AfterOnly => sec == Section::After,
    };
    if !ok_sec {
        ctx.wrong_section(raw_key, sec);
        return;
    }
    if args.len() < spec.min || args.len() > spec.max {
        ctx.msg(&format!(
            "The \"{}\" keyword in the \"{}\" section has {} segment(s) but {} expected. Item skipped.",
            raw_key,
            sec.name(),
            args.len(),
            if spec.min == spec.max { spec.min.to_string() } else { format!("{} to {}", spec.min, spec.max) }
        ));
        return;
    }
    match kw {
        "clean" => ops_misc::clean_kw(ctx, args),
        "dir" => ops_fs::dir_kw(ctx, args),
        "file" => ops_fs::file_kw(ctx, args),
        "regkey" => ops_reg::regkey_kw(ctx, args),
        "regvalue" => ops_reg::regvalue_kw(ctx, args),
        "pdirp" => ops_fs::pdirp_kw(ctx, args),
        "pfilep" => ops_fs::pfilep_kw(ctx, args),
        "pregkeyp" => ops_reg::pregkeyp_kw(ctx, args),
        "pregvaluep" => ops_reg::pregvaluep_kw(ctx, args),
        "rtldir" => ops_fs::ltr_dir(ctx, args, true),
        "ltrdir" => ops_fs::ltr_dir(ctx, args, false),
        "rtlfile" => ops_fs::ltr_file(ctx, args, true),
        "ltrfile" => ops_fs::ltr_file(ctx, args, false),
        "rtlregkey" => ops_reg::ltr_regkey(ctx, args, true),
        "ltrregkey" => ops_reg::ltr_regkey(ctx, args, false),
        "rtlregvalue" => ops_reg::ltr_regvalue(ctx, args, true),
        "ltrregvalue" => ops_reg::ltr_regvalue(ctx, args, false),
        "plusdir" => ops_fs::plus_dir(ctx, args),
        "plusfile" => ops_fs::plus_file(ctx, args),
        "plusregkey" => ops_reg::plus_regkey(ctx, args),
        "plusregvalue" => ops_reg::plus_regvalue(ctx, args),
        "minusdir" => ops_fs::minus_dir(ctx, args),
        "minusfile" => ops_fs::minus_file(ctx, args),
        "minusregkey" => ops_reg::minus_regkey(ctx, args),
        "minusregvalue" => ops_reg::minus_regvalue(ctx, args),
        "attributes" => ops_fs::attributes(ctx, args),
        "batch" => ops_fs::batch(ctx, args),
        "clip" => ops_misc::clip(ctx, args),
        "date" => ops_fs::date(ctx, args),
        "delay" => ops_misc::delay(ctx, args),
        "envvar" => ops_misc::envvar(ctx, args),
        "giniread" => ops_misc::giniread(ctx, args),
        "giniwrite" => ops_misc::giniwrite(ctx, args),
        "keeponly" => ops_fs::keeponly(ctx, args),
        "killprocess" => ops_misc::killprocess(ctx, args),
        "message" => ops_misc::message(ctx, args),
        "nowait" => ops_misc::nowait(ctx, sec, args),
        "quit" => ops_misc::quit(ctx, args),
        "regdll" => ops_misc::regdll(ctx, args),
        "regexport" => ops_reg::regexport_kw(ctx, args),
        "regimport" => ops_reg::regimport_kw(ctx, args),
        "regmerge" => ops_fs::regmerge(ctx, args),
        "replace" => ops_fs::replace(ctx, args),
        "replaceline" => ops_fs::replaceline(ctx, args),
        "restore" => run_all_prog(ctx, args),
        "grun" => ops_misc::grun(ctx, args),
        "service" => ops_misc::service(ctx, args),
        "gstringreplace" => ops_misc::stringreplace(ctx, args),
        "symlink" => ops_misc::symlink(ctx, args),
        "systemrefresh" => ops_misc::systemrefresh(ctx, args),
        "uservar" => ops_misc::uservar(ctx, args),
        _ => {}
    }
}

/// `restore = 1`: runs the pending automatic tasks of the AFTER section now.
fn run_all_prog(ctx: &mut Ctx, args: &[String]) {
    let v = if args.is_empty() { 0 } else { ctx.expand_num(&args[0]) };
    if v == 1 {
        run_pending_prog(ctx);
    }
}

/// `BeforeSecProc`.
pub fn process_before(ctx: &mut Ctx) {
    let mut tasks = std::mem::take(&mut ctx.before);
    ctx.phase.set("BEFORE");
    for pass in 0..2 {
        if pass == 1 {
            stop_on_leftover_backups(ctx, &tasks);
        }
        for (i, t) in tasks.iter_mut().enumerate() {
            let now = matches!(&*t, TaskKind::User { kw, .. } if (pass == 0) == RUN_FIRST_BEFORE.contains(&kw.as_str()));
            if now {
                if let TaskKind::User { kw, args, raw_key, .. } = std::mem::replace(t, TaskKind::Done) {
                    ctx.item.set(i + 1);
                    exec_user(ctx, Section::Before, &kw, &args, &raw_key);
                }
            }
        }
    }
    ctx.item.set(0);
    ctx.log("[Before Section Tasks List - Processed]");
    log_state(ctx);
}

/// `_backup` is the name gaP gives, during a run, to the original registry key, value, file or folder of `regkey`,
/// `regvalue`, `dir`, `file` and their `(...)` forms, and removes when the run ends. One that is still there is the
/// original data of a run that did not end: copying over it would lose it. gaP stops before changing anything.
fn stop_on_leftover_backups(ctx: &mut Ctx, tasks: &[TaskKind]) {
    let mut found = Vec::new();
    for t in tasks {
        if let TaskKind::User { kw, args, .. } = t {
            let left = match kw.as_str() {
                "regkey" | "regvalue" | "pregkeyp" | "pregvaluep" => ops_reg::leftover_backup(ctx, kw, args),
                "dir" | "file" | "pdirp" | "pfilep" => ops_fs::leftover_backup(ctx, kw, args),
                _ => None,
            };
            found.extend(left);
        }
    }
    if found.is_empty() {
        return;
    }
    for b in &found {
        ctx.err(&format!("leftover backup \"{}\"", b));
    }
    ctx.msg(&format!(
        "An earlier run did not finish and left its original data in:\n\n{}\n\nRestore or delete {} before running again. Nothing was changed.",
        found.join("\n"),
        if found.len() == 1 { "it" } else { "them" }
    ));
    clean::clean_all(&ctx.script_full, None);
    exit_code(EXIT_BACKUP);
}

/// `AfterSecProc`.
pub fn process_after(ctx: &mut Ctx) {
    let mut tasks = std::mem::take(&mut ctx.after);
    ctx.phase.set("AFTER");
    for (i, t) in tasks.iter_mut().enumerate() {
        let now = matches!(&*t, TaskKind::User { kw, .. } if RUN_FIRST_AFTER.contains(&kw.as_str()));
        if now {
            if let TaskKind::User { kw, args, raw_key, .. } = std::mem::replace(t, TaskKind::Done) {
                ctx.item.set(i + 1);
                exec_user(ctx, Section::After, &kw, &args, &raw_key);
            }
        }
    }
    ctx.item.set(0);
    // With `restore = 1` present, the automatic tasks wait for the keyword
    // (in the original they always ran before, making `restore` useless).
    let mut deferred = false;
    for t in &tasks {
        if let TaskKind::User { kw, args, .. } = t {
            if kw == "restore" && !args.is_empty() && ctx.expand_num(&args[0]) == 1 {
                deferred = true;
            }
        }
    }
    if !deferred {
        run_pending_prog(ctx);
    }
    for (i, t) in tasks.iter_mut().enumerate() {
        if let TaskKind::User { kw, args, raw_key, .. } = std::mem::replace(t, TaskKind::Done) {
            ctx.item.set(i + 1);
            exec_user(ctx, Section::After, &kw, &args, &raw_key);
        }
    }
    ctx.item.set(0);
    // restore = 1 never reached (e.g., removed by architecture): leave nothing pending.
    if deferred {
        run_pending_prog(ctx);
    }
    ctx.log("[After Section Tasks List - Processed]");
    log_state(ctx);
}

/// Runs the automatic tasks of the AFTER queue that are still pending.
fn run_pending_prog(ctx: &mut Ctx) {
    let list = std::mem::take(&mut ctx.after_prog);
    for p in &list {
        run_prog(ctx, p);
    }
}

fn log_state(ctx: &Ctx) {
    if ctx.debug.is_none() {
        return;
    }
    ctx.raw("[Variables List]");
    for (k, v) in &ctx.vars {
        ctx.raw(&format!("{}|{}", k, v));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbols_become_internal_names() {
        for (raw, want) in [
            ("->dir", "ltrdir"),
            ("<-file", "rtlfile"),
            ("(file)", "pfilep"),
            ("(regvalue)", "pregvaluep"),
            ("+regkey", "plusregkey"),
            ("-dir", "minusdir"),
            ("Run", "grun"),
            ("IniRead", "giniread"),
            ("iniwrite", "giniwrite"),
            ("StringReplace", "gstringreplace"),
            ("Clean", "clean"),
        ] {
            assert_eq!(normalize(raw, "x64").as_deref(), Some(want), "{raw}");
        }
    }

    #[test]
    fn architecture_filter() {
        // "x64" in the key: the item is for 64-bit Windows only; the word is removed when it applies
        assert_eq!(normalize("+regvalue x64", "x64").as_deref(), Some("plusregvalue"));
        assert_eq!(normalize("+regvalue x64", "x86"), None);
        assert_eq!(normalize("->dir x86", "x86").as_deref(), Some("ltrdir"));
        assert_eq!(normalize("->dir x86", "x64"), None);
        // both words, or none
        assert_eq!(normalize("->dir x86 x64", "x64"), None);
        assert_eq!(normalize("->dir", "x86").as_deref(), Some("ltrdir"));
    }

    #[test]
    fn every_keyword_of_the_template_has_a_spec() {
        for raw in [
            "dir", "file", "regkey", "regvalue", "(dir)", "(file)", "(regkey)", "(regvalue)", "->dir", "<-dir", "->file",
            "<-file", "->regkey", "<-regkey", "->regvalue", "<-regvalue", "+dir", "+file", "+regkey", "+regvalue",
            "-dir", "-file", "-regkey", "-regvalue", "attributes", "batch", "clean", "clip", "date", "delay", "envvar",
            "iniread", "iniwrite", "keeponly", "killprocess", "message", "nowait", "quit", "regdll", "regexport",
            "regimport", "regmerge", "replace", "replaceline", "restore", "run", "service", "stringreplace", "symlink",
            "systemrefresh", "uservar",
        ] {
            let kw = normalize(raw, "x64").unwrap();
            assert!(SPECS.iter().any(|s| s.name == kw), "{raw} -> {kw}");
        }
    }
}
