//! Macro expansion (`FindReplaceX`): `{VAR}`, `{{VAR}}`, `!@@!…!@@!`
//! (encrypted text), `%ShellFolder%`, `%ENVIRONMENT_VARIABLE%` and escapes
//! `<{` and `<}`.

use crate::ctx::{exit_code, Ctx, EXIT_CANCELLED, EXIT_CONFIG, FUNC_MACROS, SHELL_FOLDERS};
use crate::{checks, crypto, sys, ui, util};

/// The smallest macro of the text: the first `}` that closes a `{`, and the nearest `{` before it, so there is no
/// other macro inside. Returns the positions (in bytes) of both. A delimiter right after a '<' is escaped, and a `}`
/// with no `{` before it is just text. `{`, `}` and `<` are ASCII, so scanning the bytes is enough.
fn innermost(s: &str) -> Option<(usize, usize)> {
    let b = s.as_bytes();
    let mut open = None;
    for i in 0..b.len() {
        if i > 0 && b[i - 1] == b'<' {
            continue;
        }
        match b[i] {
            b'{' => open = Some(i),
            b'}' => {
                if let Some(o) = open {
                    return Some((o, i));
                }
            }
            _ => {}
        }
    }
    None
}

pub fn unescape(s: &str) -> String {
    s.replace("<{", "{").replace("<}", "}")
}

/// Expands `%NAME%` with environment variables (AutoIt Option ExpandEnvStrings).
pub fn expand_env(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(a) = rest.find('%') {
        out.push_str(&rest[..a]);
        let after = &rest[a + 1..];
        if let Some(b) = after.find('%') {
            let name = &after[..b];
            match (!name.is_empty()).then(|| std::env::var(name).ok()).flatten() {
                Some(v) => {
                    out.push_str(&v);
                    rest = &after[b + 1..];
                    continue;
                }
                None => {
                    out.push('%');
                    rest = after;
                    continue;
                }
            }
        } else {
            out.push('%');
            rest = after;
            break;
        }
    }
    out.push_str(rest);
    out
}

/// Names of the `%NAME%` tokens that are still in `s` after the shell folders and
/// the environment variables were expanded, that is, the ones that are not defined.
/// Text that does not look like a variable name (`%1`, `%~dp0`, `100% of 50%`) and the
/// variables that cmd.exe expands only at run time (`%CD%`, `%DATE%`...) are ignored.
pub fn undefined_env_names(s: &str) -> Vec<String> {
    const CMD_DYNAMIC: &[&str] =
        &["CD", "DATE", "TIME", "RANDOM", "ERRORLEVEL", "CMDEXTVERSION", "CMDCMDLINE", "HIGHESTNUMANODENUMBER"];
    let mut found = Vec::new();
    let mut rest = s;
    while let Some(a) = rest.find('%') {
        let after = &rest[a + 1..];
        let Some(b) = after.find('%') else { break };
        let name = &after[..b];
        let looks_like_name = name.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && !name.ends_with(' ')
            && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | ' ' | '(' | ')' | '-' | '.'));
        if looks_like_name && !CMD_DYNAMIC.iter().any(|d| d.eq_ignore_ascii_case(name)) {
            found.push(name.to_string());
            rest = &after[b + 1..];
        } else {
            rest = after;
        }
    }
    found
}

/// Stand-ins (Unicode private use) for the characters of a value that the macro syntax would read.
const LT: char = '\u{E002}';
const OPEN: char = '\u{E000}';
const CLOSE: char = '\u{E001}';
const ESC: char = '\u{E003}';

/// Puts ESC before each stand-in and before ESC itself, so a text that already has one of them is not read as a
/// neutralized character.
fn protect(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, LT | OPEN | CLOSE | ESC) {
            o.push(ESC);
        }
        o.push(c);
    }
    o
}

/// A value goes into the text with `<`, `{` and `}` replaced, so nothing in it can be taken for a macro or for an
/// escape, whatever it is (a CLSID in the command line, the text of the clipboard, a `<` at its end).
fn neutralize(v: &str) -> String {
    protect(v).replace('<', &LT.to_string()).replace('{', &OPEN.to_string()).replace('}', &CLOSE.to_string())
}

/// The final text: the escapes `<{` and `<}` become `{` and `}`, the stand-ins become what they were and a
/// character after ESC is kept as it is.
fn restore(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    let text = unescape(s);
    let mut it = text.chars();
    while let Some(c) = it.next() {
        match c {
            ESC => {
                if let Some(n) = it.next() {
                    o.push(n);
                }
            }
            LT => o.push('<'),
            OPEN => o.push('{'),
            CLOSE => o.push('}'),
            _ => o.push(c),
        }
    }
    o
}

/// Replaces `{{NAME}}` (the value with doubled backslashes) and `{NAME}`: the smallest macro first, again and again, so
/// a macro can be inside the name of another one (`{PREFIX_{LANGCODE}}`). The text is never read again beyond that:
/// a value goes in with its braces neutralized, so each step takes one macro out and none comes in, and the loop
/// always ends. An undefined name is reported by the caller. `Err` = (the text up to the problem, the name).
fn expand_macros(s: &str, mut lookup: impl FnMut(&str) -> Option<String>) -> Result<String, (String, String)> {
    let mut s = protect(s);
    while let Some((a, b)) = innermost(&s) {
        let t = s.as_bytes();
        // `{{NAME}}`: the macro is wrapped by one more pair of unescaped braces
        let doubled = a > 0 && t.get(b + 1) == Some(&b'}') && t[a - 1] == b'{' && (a == 1 || t[a - 2] != b'<');
        let (from, to) = if doubled { (a - 1, b + 2) } else { (a, b + 1) };
        let name = restore(&s[a + 1..b]);
        match lookup(&name) {
            Some(v) => {
                let v = neutralize(&v);
                s.replace_range(from..to, &if doubled { v.replace('\\', "\\\\") } else { v });
            }
            None => return Err((restore(&s), name)),
        }
    }
    Ok(restore(&s))
}

impl Ctx {
    /// Value of a "function" macro (computed on every use).
    fn func_macro(&mut self, name: &str) -> String {
        let t = util::local_now();
        let h = t.hour;
        let h12 = if h.is_multiple_of(12) { 12 } else { h % 12 };
        match name {
            "LANGCODE" => sys::os_lang().to_string(),
            "USERDIR" => ui::select_folder("Please, select a directory for {USERDIR} macro:").unwrap_or_default(),
            "USERFILE" => ui::select_file("Please, select a file for {USERFILE} macro:").unwrap_or_default(),
            "USERTEXT" => ui::input_box("Please, define an input text for {USERTEXT} macro:", "Input text:", false)
                .unwrap_or_default(),
            "CLIPBOARD" => ui::clip_get(),
            "d" => format!("{:02}", t.day),
            "j" => t.day.to_string(),
            "m" => format!("{:02}", t.month),
            "n" => t.month.to_string(),
            "Y" => format!("{:04}", t.year),
            "y" => format!("{:02}", t.year % 100),
            "H" => format!("{:02}", h),
            "G" => h.to_string(),
            "h" => format!("{:02}", h12),
            "g" => h12.to_string(),
            "i" => format!("{:02}", t.min),
            "s" => format!("{:02}", t.sec),
            "U" => util::unix_now().to_string(),
            // The language of the system comes from the system itself: "pt-BR", "Portuguese", "Brazil".
            "LANG" => sys::locale().map(|(_, lang, _)| lang).unwrap_or_else(|| "-1".into()),
            "LOCALE" => match sys::locale() {
                // "en-US" -> "EN"
                Some((n, _, _)) => n.split('-').next().unwrap_or(&n).to_uppercase(),
                None => "-1".into(),
            },
            "LOCALENAME" => sys::locale().map(|(n, _, _)| n).unwrap_or_else(|| "-1".into()),
            "LOCALENAME_SHORT" => match sys::locale() {
                Some((n, _, _)) => n.split('-').next().unwrap_or(&n).to_lowercase(),
                None => "-1".into(),
            },
            "COUNTRY" => sys::locale().map(|(_, _, country)| country).unwrap_or_else(|| "-1".into()),
            _ => String::new(),
        }
    }

    /// Resolves the name of a macro (already without escapes). `None` = undefined.
    fn lookup_macro(&mut self, name: &str) -> Option<String> {
        if let Some(v) = self.get_var(name) {
            return Some(v.to_string());
        }
        if name == "JAVAEXE" {
            // looked for in the registry the first time it is used
            let exe = checks::java_exe_text();
            self.add_var("JAVAEXE", &exe);
            return Some(exe);
        }
        if FUNC_MACROS.contains(&name) {
            return Some(self.func_macro(name));
        }
        None
    }

    /// Decrypts a `!@@!…!@@!` token asking for the password (with cache and 3 attempts).
    fn decrypt_token(&mut self, token: &str) -> String {
        if let Some(pw) = self.pw_cache.clone() {
            if let Some(t) = crypto::decrypt(token, &pw) {
                return t;
            }
        }
        for _ in 0..3 {
            let Some(pw) = ui::input_box("Please, enter the password to decrypt data:", "Password:", true) else {
                exit_code(EXIT_CANCELLED);
            };
            if let Some(t) = crypto::decrypt(token, &pw) {
                self.pw_cache = Some(pw);
                return t;
            }
            ui::info(&self.title, "Wrong password or corrupted encrypted text. Please, try again.");
        }
        exit_code(EXIT_CONFIG);
    }

    /// `FindReplaceX`. `reg_key` = true removes the suffix "64" from the registry root
    /// (HKLM64 -> HKLM), the old flag `2`.
    pub fn expand_ex(&mut self, input: &str, reg_key: bool) -> String {
        let mut s = input.to_string();
        // 1) encrypted text: the decrypted text takes the place of the token and is expanded with the rest of the
        // text, but it is not searched for tokens again
        let mut pos = 0;
        while let Some(a) = s[pos..].find(crypto::MARK).map(|a| a + pos) {
            let from = a + crypto::MARK.len();
            let Some(b) = s[from..].find(crypto::MARK) else { break };
            let end = from + b + crypto::MARK.len();
            let token = s[a..end].to_string();
            let plain = self.decrypt_token(&token);
            s.replace_range(a..end, &plain);
            pos = a + plain.len();
        }
        // 2) `{{VAR}}`, `{VAR}` and the escapes `<{` `<}`: the values are put in as they are (see `expand_macros`)
        s = match expand_macros(&s, |name| self.lookup_macro(name)) {
            Ok(t) => t,
            Err((partial, name)) => {
                self.msg(&format!("The \"{}\" variable isn't defined and can't be used.", name));
                return partial;
            }
        };
        // shell folders and environment variables; a folder is read from the system only if it is used
        if s.contains('%') {
            for (i, (name, _, _)) in SHELL_FOLDERS.iter().enumerate() {
                let token = format!("%{}%", name);
                if util::contains_ic(&s, &token) {
                    let val = self.shell_value(i).to_string();
                    if !val.is_empty() {
                        s = util::replace_all_ic(&s, &token, &val);
                    }
                }
            }
            s = expand_env(&s);
        }
        // NEW (gaP-rs): `%NAME%` that is neither a shell folder nor an environment variable is
        // reported like an undefined `{VAR}` (before, it was silently kept as literal text).
        if let Some(name) = undefined_env_names(&s).first() {
            self.msg(&format!("The \"{}\" variable isn't defined and can't be used.", name));
            return s;
        }
        if reg_key {
            let first = s.split('\\').next().unwrap_or("").to_string();
            if first.len() > 2 && first.ends_with("64") {
                s = s.replacen(&first, &first[..first.len() - 2], 1);
            }
        }
        s
    }

    pub fn expand(&mut self, input: &str) -> String {
        self.expand_ex(input, false)
    }

    /// Expands and converts to a number (empty/NULL = 0).
    pub fn expand_num(&mut self, input: &str) -> i64 {
        let v = self.expand(input);
        util::auto_num(&v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smallest_macro() {
        assert_eq!(innermost("a {X} b"), Some((2, 4)));
        assert_eq!(innermost("{A{B}}"), Some((2, 4)));
        assert_eq!(innermost("} {A}"), Some((2, 4))); // a `}` with no `{` is text
        assert_eq!(innermost("a < {X} b"), Some((4, 6)));
        assert_eq!(innermost("{open"), None);
        assert_eq!(innermost("a <{X<} b"), None);
        // only "<{" and "<}" escape; the old "<<" is not special any more
        assert_eq!(innermost("a <<{X} b"), None);
        assert_eq!(unescape("<{x<}<<"), "{x}<<");
    }

    fn lookup(name: &str) -> Option<String> {
        match name {
            "ROOT" => Some(r"D:\Apps".to_string()),
            // values with braces and escapes in them: they have to come out as they are
            "PARAMETERS" => Some("::{20D04FE0-3AEA-1069-A2D8-08002B30309D} <{x<}".to_string()),
            "X" => Some("x".to_string()),
            "LANG" => Some("pt".to_string()),
            "PREFIX_pt" => Some("ola".to_string()),
            "LT" => Some("a<".to_string()),
            "PUA" => Some("\u{E000}\u{E001}\u{E002}\u{E003}x".to_string()),
            "BR" => Some("a{b}c<".to_string()),
            _ => None,
        }
    }

    #[test]
    fn macro_values_are_not_expanded_again() {
        assert_eq!(
            expand_macros("run {PARAMETERS}", lookup).as_deref(),
            Ok("run ::{20D04FE0-3AEA-1069-A2D8-08002B30309D} <{x<}")
        );
    }

    #[test]
    fn macros_and_escapes() {
        assert_eq!(expand_macros(r"{ROOT}\{X}.exe", lookup).as_deref(), Ok(r"D:\Apps\x.exe"));
        assert_eq!(expand_macros(r"{{ROOT}}\{X}", lookup).as_deref(), Ok(r"D:\\Apps\x"));
        assert_eq!(expand_macros("{X}{{X}}{X}", lookup).as_deref(), Ok("xxx"));
        // the escapes of the text around the macros are removed, an unclosed brace stays
        assert_eq!(expand_macros("<{literal<} {X} {open", lookup).as_deref(), Ok("{literal} x {open"));
        assert_eq!(expand_macros("plain", lookup).as_deref(), Ok("plain"));
    }

    #[test]
    fn a_macro_inside_the_name_of_another() {
        assert_eq!(expand_macros("{PREFIX_{LANG}}!", lookup).as_deref(), Ok("ola!"));
        assert_eq!(expand_macros(r"{{ROOT}}\{PREFIX_{LANG}}", lookup).as_deref(), Ok(r"D:\\Apps\ola"));
        // a name made with a macro that is not defined is reported with the name that came out
        assert_eq!(
            expand_macros("{PREFIX_{X}}", lookup),
            Err(("{PREFIX_x}".to_string(), "PREFIX_x".to_string()))
        );
    }

    #[test]
    fn a_value_can_not_change_what_comes_after_it() {
        // a "<" at the end of a value must not escape the "{" that follows it
        assert_eq!(expand_macros("{LT}{X}", lookup).as_deref(), Ok("a<x"));
        assert_eq!(expand_macros("{LT}<{X<}", lookup).as_deref(), Ok("a<{X}"));
    }

    #[test]
    fn text_with_the_stand_ins_comes_out_unchanged() {
        let pua = "\u{E000}\u{E001}\u{E002}\u{E003}";
        // in a value, and in the text itself
        assert_eq!(expand_macros("[{PUA}]", lookup).as_deref(), Ok(format!("[{}x]", pua).as_str()));
        assert_eq!(expand_macros(&format!("{}{{X}}", pua), lookup).as_deref(), Ok(format!("{}x", pua).as_str()));
    }

    #[test]
    fn an_undefined_name_made_with_a_value_is_shown_as_it_is() {
        assert_eq!(
            expand_macros("{q{BR}z}", lookup),
            Err(("{qa{b}c<z}".to_string(), "qa{b}c<z".to_string()))
        );
    }

    #[test]
    fn undefined_macro_is_reported_with_the_text_before_it() {
        assert_eq!(
            expand_macros("a {X} b {NOPE} c", lookup),
            Err(("a x b {NOPE} c".to_string(), "NOPE".to_string()))
        );
    }

    #[test]
    fn env() {
        std::env::set_var("GAPTEST", "ok");
        assert_eq!(expand_env("a%GAPTEST%b%NOPE_X%c"), "aokb%NOPE_X%c");
    }

    #[test]
    fn undefined_env_tokens() {
        assert_eq!(undefined_env_names(r"%Windows%\System32"), vec!["Windows"]);
        assert_eq!(undefined_env_names(r"%ProgramFiles(x86)%\App"), vec!["ProgramFiles(x86)"]);
        assert_eq!(undefined_env_names("%A% and %B%"), vec!["A", "B"]);
        // not variables: batch parameters, percent signs in text, cmd.exe run-time variables
        assert!(undefined_env_names("100% of 50%").is_empty());
        assert!(undefined_env_names("/c echo %1 %~dp0 %CD% %errorlevel%").is_empty());
        assert!(undefined_env_names("plain text").is_empty());
        assert!(undefined_env_names(r#""C:\App\App.exe" "%1""#).is_empty());
    }
}
