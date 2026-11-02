//! Macro expansion (`FindReplaceX`): `{VAR}`, `{{VAR}}`, `!@@!…!@@!`
//! (encrypted text), `%ShellFolder%`, `%ENVIRONMENT_VARIABLE%` and escapes
//! `<{` and `<}`.

use crate::ctx::{exit_code, Ctx, EXIT_CANCELLED, EXIT_CONFIG, FUNC_MACROS, SHELL_FOLDERS};
use crate::{checks, crypto, sys, ui, util};

/// Looks for unescaped `open … close` (a delimiter right after a '<' is escaped).
/// Returns (start, end_exclusive, inner content), the positions in bytes. `{`, `}` and `<` are ASCII, so
/// scanning the bytes is enough and the text is never decoded.
fn find_braced(s: &str, depth: usize) -> Option<(usize, usize, String)> {
    let b = s.as_bytes();
    let n = b.len();
    let unescaped = |i: usize| -> bool { i == 0 || b[i - 1] != b'<' };
    let run = |i: usize, c: u8| -> bool { b[i..i + depth].iter().all(|&x| x == c) };
    let mut i = 0;
    while i + depth <= n {
        if run(i, b'{') && unescaped(i) {
            let mut j = i + depth;
            while j + depth <= n {
                if run(j, b'}') && unescaped(j) {
                    return Some((i, j + depth, s[i + depth..j].to_string()));
                }
                j += 1;
            }
            return None;
        }
        i += 1;
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
        let mut guard = 0;
        loop {
            guard += 1;
            if guard > 500 {
                self.msg("Macro expansion limit reached (possible recursive variable).");
                break;
            }
            // 1) encrypted text
            if let Some(a) = s.find(crypto::MARK) {
                let from = a + crypto::MARK.len();
                if let Some(b) = s[from..].find(crypto::MARK) {
                    let end = from + b + crypto::MARK.len();
                    let token = s[a..end].to_string();
                    let plain = self.decrypt_token(&token);
                    s.replace_range(a..end, &plain);
                    continue;
                }
            }
            // 2) {{VAR}}: value with doubled '\'
            if let Some((a, b, inner)) = find_braced(&s, 2) {
                let name = unescape(&inner);
                match self.lookup_macro(&name) {
                    Some(v) => {
                        s.replace_range(a..b, &v.replace('\\', "\\\\"));
                        continue;
                    }
                    None => {
                        self.msg(&format!("The \"{}\" variable isn't defined and can't be used.", name));
                        return s;
                    }
                }
            }
            // 3) {VAR}
            if let Some((a, b, inner)) = find_braced(&s, 1) {
                let name = unescape(&inner);
                match self.lookup_macro(&name) {
                    Some(v) => {
                        s.replace_range(a..b, &v);
                        continue;
                    }
                    None => {
                        self.msg(&format!("The \"{}\" variable isn't defined and can't be used.", name));
                        return s;
                    }
                }
            }
            break;
        }
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
        unescape(&s)
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
    fn braces_escape() {
        assert_eq!(find_braced("a {X} b", 1).map(|t| t.2), Some("X".to_string()));
        assert!(find_braced("a <{X<} b", 1).is_none());
        // only "<{" and "<}" escape; the old "<<" is not special any more
        assert!(find_braced("a <<{X} b", 1).is_none());
        assert_eq!(find_braced("a < {X} b", 1).map(|t| t.2), Some("X".to_string()));
        assert_eq!(find_braced("{{A}}", 2).map(|t| t.2), Some("A".to_string()));
        assert_eq!(unescape("<{x<}<<"), "{x}<<");
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
