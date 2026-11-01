//! Macro expansion (`FindReplaceX`): `{VAR}`, `{{VAR}}`, `!@@!…!@@!`
//! (encrypted text), `%ShellFolder%`, `%ENVIRONMENT_VARIABLE%` and escapes
//! `<{`, `<}`, `<<`.

use crate::ctx::{exit_now, Ctx, FUNC_MACROS};
use crate::langtable::LANGS;
use crate::{crypto, sys, ui, util};

/// Looks for unescaped `open … close` (even number of '<' before each
/// delimiter). Returns (start, end_exclusive, inner content).
fn find_braced(s: &str, depth: usize) -> Option<(usize, usize, String)> {
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    let unescaped = |i: usize| -> bool {
        let mut k = 0;
        let mut j = i;
        while j > 0 && chars[j - 1] == '<' {
            k += 1;
            j -= 1;
        }
        k % 2 == 0
    };
    let mut i = 0;
    while i + depth <= n {
        if (0..depth).all(|k| chars[i + k] == '{') && unescaped(i) {
            let mut j = i + depth;
            while j + depth <= n {
                if (0..depth).all(|k| chars[j + k] == '}') && unescaped(j) {
                    let inner: String = chars[i + depth..j].iter().collect();
                    let start_b: usize = chars[..i].iter().map(|c| c.len_utf8()).sum();
                    let end_b: usize = chars[..j + depth].iter().map(|c| c.len_utf8()).sum();
                    return Some((start_b, end_b, inner));
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
    s.replace("<{", "{").replace("<}", "}").replace("<<", "<")
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

fn lang_row() -> Option<(&'static str, &'static str)> {
    let code = sys::os_lang();
    LANGS.iter().find(|(c, _, _)| *c == code).map(|(_, n, d)| (*n, *d))
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
            "LANG" => match lang_row() {
                // "English - United States" -> "English"
                Some((_, d)) => d.rfind(" -").map(|i| d[..i].to_string()).unwrap_or_else(|| d.to_string()),
                None => "-1".into(),
            },
            "LOCALE" => match lang_row() {
                // "en-US" -> "EN"
                Some((n, _)) => n.split('-').next().unwrap_or(n).to_uppercase(),
                None => "-1".into(),
            },
            "LOCALENAME" => lang_row().map(|(n, _)| n.to_string()).unwrap_or_else(|| "-1".into()),
            "LOCALENAME_SHORT" => match lang_row() {
                Some((n, _)) => n.split('-').next().unwrap_or(n).to_lowercase(),
                None => "-1".into(),
            },
            "COUNTRY" => match lang_row() {
                // "English - United States" -> "United States"
                Some((_, d)) => d.find("- ").map(|i| d[i + 2..].to_string()).unwrap_or_else(|| d.to_string()),
                None => "-1".into(),
            },
            _ => String::new(),
        }
    }

    /// Resolves the name of a macro (already without escapes). `None` = undefined.
    fn lookup_macro(&mut self, name: &str) -> Option<String> {
        if let Some(v) = self.get_var(name) {
            return Some(v.to_string());
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
                exit_now();
            };
            if let Some(t) = crypto::decrypt(token, &pw) {
                self.pw_cache = Some(pw);
                return t;
            }
            ui::info(&self.title, "Wrong password or corrupted encrypted text. Please, try again.");
        }
        exit_now();
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
        // shell folders and environment variables
        for (name, val) in self.shell.clone() {
            if !val.is_empty() {
                s = util::replace_all_ic(&s, &format!("%{}%", name), &val);
            }
        }
        s = expand_env(&s);
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
        assert_eq!(find_braced("a <<{X} b", 1).map(|t| t.2), Some("X".to_string()));
        assert_eq!(find_braced("{{A}}", 2).map(|t| t.2), Some("A".to_string()));
        assert_eq!(unescape("<{x<}<<"), "{x}<");
    }

    #[test]
    fn env() {
        std::env::set_var("GAPTEST", "ok");
        assert_eq!(expand_env("a%GAPTEST%b%NOPE_X%c"), "aokb%NOPE_X%c");
    }
}
