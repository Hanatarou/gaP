//! Engine of the BEFORE / AFTER sections: .ini reading, x86/x64 filter,
//! keyword normalization, section/parameter validation and execution.

use crate::ctx::{exit_now, Ctx, Prog, Section, TaskKind};
use crate::ini::IniDoc;
use crate::{ops_fs, ops_misc, ops_reg, util};
use util::eq_ic;

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
    Spec { name: "giniread", min: 4, max: 4, allowed: Both },
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
    let doc = IniDoc::load(&ctx.ini_path);
    let pairs = doc.section_pairs(sec.name()).unwrap_or_default();
    ctx.log(&format!("[{} Section Tasks List - Raw]", sec.name().to_string() + ""));
    for (k, v) in &pairs {
        ctx.log(&format!("{}|{}", k, v));
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
    ctx.log(&format!("[{} Section Tasks List - Initialized]", sec.name()));
    for t in &out {
        if let TaskKind::User { kw, raw_val, .. } = t {
            ctx.log(&format!("{}|{}", kw, raw_val));
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

/// Runs a user item (validating section and number of parameters).
fn exec_user(ctx: &mut Ctx, sec: Section, kw: &str, args: &[String], raw_key: &str) {
    ctx.log(&format!("EXEC [{}] {} :: {:?}", sec.name(), kw, args));
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
        let list = std::mem::take(&mut ctx.after_prog);
        for p in &list {
            run_prog(ctx, p);
        }
    }
}

/// `BeforeSecProc`.
pub fn process_before(ctx: &mut Ctx) {
    let mut tasks = std::mem::take(&mut ctx.before);
    for pass in 0..2 {
        for t in tasks.iter_mut() {
            if let TaskKind::User { kw, args, raw_key, .. } = t.clone() {
                let first = RUN_FIRST_BEFORE.contains(&kw.as_str());
                if (pass == 0) == first {
                    exec_user(ctx, Section::Before, &kw, &args, &raw_key);
                    *t = TaskKind::Done;
                }
            }
        }
    }
    ctx.log("[Before Section Tasks List - Processed]");
    log_state(ctx);
}

/// `AfterSecProc`.
pub fn process_after(ctx: &mut Ctx) {
    let mut tasks = std::mem::take(&mut ctx.after);
    for t in tasks.iter_mut() {
        if let TaskKind::User { kw, args, raw_key, .. } = t.clone() {
            if RUN_FIRST_AFTER.contains(&kw.as_str()) {
                exec_user(ctx, Section::After, &kw, &args, &raw_key);
                *t = TaskKind::Done;
            }
        }
    }
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
        let list = std::mem::take(&mut ctx.after_prog);
        for p in &list {
            run_prog(ctx, p);
        }
    }
    for t in tasks.iter_mut() {
        if let TaskKind::User { kw, args, raw_key, .. } = t.clone() {
            exec_user(ctx, Section::After, &kw, &args, &raw_key);
            *t = TaskKind::Done;
        }
    }
    // restore = 1 never reached (e.g., removed by architecture): leave nothing pending.
    if deferred && !ctx.after_prog.is_empty() {
        let list = std::mem::take(&mut ctx.after_prog);
        for p in &list {
            run_prog(ctx, p);
        }
    }
    ctx.log("[After Section Tasks List - Processed]");
    log_state(ctx);
}

fn log_state(ctx: &Ctx) {
    if ctx.debug.is_none() {
        return;
    }
    ctx.log("[Variables List]");
    for (k, v) in &ctx.vars {
        ctx.log(&format!("{}|{}", k, v));
    }
}

#[allow(dead_code)]
fn _unused() {
    let _ = exit_now as fn() -> !;
    let _ = eq_ic("", "");
}
