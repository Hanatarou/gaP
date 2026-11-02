//! Reading/writing of INI files (replaces IniRead, IniReadSection,
//! IniWrite, IniDelete and the manual `iniread`/`iniwrite` routines).
//!
//! The file is handled as a list of lines, preserving comments,
//! line terminators and encoding (UTF-8/UTF-16/ANSI).

use crate::util::{self, Enc};

pub struct IniDoc {
    pub lines: Vec<(String, String)>, // (content, terminator)
    pub enc: Enc,
    pub existed: bool,
}

fn is_header(line: &str) -> Option<String> {
    let t = line.trim();
    if t.len() >= 2 && t.starts_with('[') && t.ends_with(']') {
        Some(t[1..t.len() - 1].trim().to_string())
    } else {
        None
    }
}

fn is_comment(line: &str) -> bool {
    line.trim_start().starts_with(';')
}

/// Splits "key = value" (first `=`), trimming both.
fn kv(line: &str) -> Option<(String, String)> {
    if is_comment(line) || is_header(line).is_some() {
        return None;
    }
    let i = line.find('=')?;
    let k = line[..i].trim();
    if k.is_empty() {
        return None;
    }
    Some((k.to_string(), line[i + 1..].trim().to_string()))
}

/// Removes outer quotes as GetPrivateProfileString (IniRead) does.
fn strip_quotes(v: &str) -> String {
    let b = v.as_bytes();
    if b.len() >= 2 && ((b[0] == b'"' && b[b.len() - 1] == b'"') || (b[0] == b'\'' && b[b.len() - 1] == b'\'')) {
        v[1..v.len() - 1].to_string()
    } else {
        v.to_string()
    }
}

impl IniDoc {
    /// A document with no lines (before the .ini is loaded).
    pub fn empty() -> IniDoc {
        IniDoc { lines: Vec::new(), enc: Enc::Ansi, existed: false }
    }

    pub fn load(path: &str) -> IniDoc {
        match util::read_text(path) {
            Some((s, enc)) => IniDoc { lines: util::split_lines_keep(&s), enc, existed: true },
            None => IniDoc { lines: Vec::new(), enc: Enc::Ansi, existed: false },
        }
    }

    pub fn save(&self, path: &str) -> std::io::Result<()> {
        let mut s = String::new();
        for (l, e) in &self.lines {
            s.push_str(l);
            s.push_str(e);
        }
        util::write_text(path, &s, self.enc)
    }

    /// Range [start, end) of the body lines of the section (`None` = global area,
    /// before the first section). `Some(header_idx)` is also returned.
    fn section_range(&self, section: Option<&str>) -> Option<(usize, usize)> {
        match section {
            None => {
                let end = self.lines.iter().position(|(l, _)| is_header(l).is_some()).unwrap_or(self.lines.len());
                Some((0, end))
            }
            Some(name) => {
                let mut start = None;
                for (i, (l, _)) in self.lines.iter().enumerate() {
                    if let Some(h) = is_header(l) {
                        if let Some(s) = start {
                            return Some((s, i));
                        }
                        if util::eq_ic(&h, name) {
                            start = Some(i + 1);
                        }
                    }
                }
                start.map(|s| (s, self.lines.len()))
            }
        }
    }

    /// IniReadSection: (key, value) pairs in order, without handling quotes.
    pub fn section_pairs(&self, section: &str) -> Option<Vec<(String, String)>> {
        let (a, b) = self.section_range(Some(section))?;
        let v: Vec<_> = self.lines[a..b].iter().filter_map(|(l, _)| kv(l)).collect();
        Some(v)
    }

    /// IniRead: first value of the key (outer quotes removed).
    pub fn get(&self, section: Option<&str>, key: &str) -> Option<String> {
        let (a, b) = self.section_range(section)?;
        self.lines[a..b]
            .iter()
            .filter_map(|(l, _)| kv(l))
            .find(|(k, _)| util::eq_ic(k, key))
            .map(|(_, v)| strip_quotes(&v))
    }

    fn eol(&self) -> String {
        self.lines.iter().find(|(_, e)| !e.is_empty()).map(|(_, e)| e.clone()).unwrap_or_else(|| "\r\n".to_string())
    }

    fn find_key(&self, section: Option<&str>, key: &str) -> Option<usize> {
        let (a, b) = self.section_range(section)?;
        (a..b).find(|&i| kv(&self.lines[i].0).map(|(k, _)| util::eq_ic(&k, key)).unwrap_or(false))
    }

    fn ensure_last_eol(&mut self) {
        let eol = self.eol();
        if let Some(last) = self.lines.last_mut() {
            if last.1.is_empty() {
                last.1 = eol;
            }
        }
    }

    /// IniWrite / global write.
    pub fn set(&mut self, section: Option<&str>, key: &str, value: &str) {
        let text = format!("{}={}", key, value);
        let eol = self.eol();
        if let Some(i) = self.find_key(section, key) {
            self.lines[i].0 = text;
            return;
        }
        match section {
            None => {
                // The original inserted before the first line that is not a
                // comment (and nothing happened in an empty file). It now also
                // works in an empty file or one with only comments.
                let (_, end) = self.section_range(None).unwrap();
                let pos = self.lines[..end].iter().position(|(l, _)| !is_comment(l)).unwrap_or(end);
                if pos >= self.lines.len() {
                    self.ensure_last_eol();
                    self.lines.push((text, eol));
                } else {
                    self.lines.insert(pos, (text, eol));
                }
            }
            Some(name) => match self.section_range(Some(name)) {
                Some((a, b)) => {
                    let mut pos = b;
                    while pos > a && self.lines[pos - 1].0.trim().is_empty() {
                        pos -= 1;
                    }
                    if pos == self.lines.len() {
                        self.ensure_last_eol();
                    }
                    self.lines.insert(pos, (text, eol));
                    if pos == self.lines.len() - 1 && self.lines[pos].1.is_empty() {
                        self.lines[pos].1 = self.eol();
                    }
                }
                None => {
                    if !self.lines.is_empty() {
                        self.ensure_last_eol();
                        self.lines.push((String::new(), eol.clone()));
                    }
                    self.lines.push((format!("[{}]", name), eol.clone()));
                    self.lines.push((text, eol));
                }
            },
        }
    }

    /// IniDelete(key). Returns true if it removed something.
    pub fn delete_key(&mut self, section: Option<&str>, key: &str) -> bool {
        if let Some(i) = self.find_key(section, key) {
            self.lines.remove(i);
            true
        } else {
            false
        }
    }

    /// Removes all keys of the global area (`--NULL`).
    pub fn delete_global_keys(&mut self) {
        if let Some((a, b)) = self.section_range(None) {
            let mut i = b;
            while i > a {
                i -= 1;
                if kv(&self.lines[i].0).is_some() {
                    self.lines.remove(i);
                }
            }
        }
    }

    /// IniDelete(whole section).
    pub fn delete_section(&mut self, name: &str) -> bool {
        let mut start = None;
        for (i, (l, _)) in self.lines.iter().enumerate() {
            if let Some(h) = is_header(l) {
                if let Some(s) = start {
                    self.lines.drain(s..i);
                    return true;
                }
                if util::eq_ic(&h, name) {
                    start = Some(i);
                }
            }
        }
        if let Some(s) = start {
            self.lines.drain(s..);
            return true;
        }
        false
    }

    pub fn new_file_encoding(&mut self, sample: &str) {
        if !self.existed && !sample.is_ascii() {
            self.enc = Enc::Utf8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(text: &str) -> IniDoc {
        IniDoc { lines: util::split_lines_keep(text), enc: Enc::Utf8, existed: true }
    }

    fn text(d: &IniDoc) -> String {
        d.lines.iter().map(|(l, e)| format!("{}{}", l, e)).collect()
    }

    #[test]
    fn get_ignores_case_and_strips_quotes() {
        let d = doc("; c\r\n[General]\r\nApplication = \"App\\a.exe\"\r\nother=1\r\n[AFTER]\r\nx=2\r\n");
        assert_eq!(d.get(Some("GENERAL"), "application").as_deref(), Some("App\\a.exe"));
        assert_eq!(d.get(Some("general"), "OTHER").as_deref(), Some("1"));
        assert_eq!(d.get(Some("GENERAL"), "x"), None);
        assert_eq!(d.get(Some("AFTER"), "x").as_deref(), Some("2"));
        assert_eq!(d.get(Some("MISSING"), "x"), None);
    }

    #[test]
    fn section_pairs_keep_order_and_skip_comments() {
        let d = doc("[BEFORE]\n; note\nb=2\na=1\n\n[AFTER]\nc=3\n");
        let p = d.section_pairs("before").unwrap();
        assert_eq!(p, vec![("b".to_string(), "2".to_string()), ("a".to_string(), "1".to_string())]);
        assert!(d.section_pairs("nope").is_none());
    }

    #[test]
    fn set_replaces_or_appends_inside_the_section() {
        let mut d = doc("[A]\r\nk=1\r\n\r\n[B]\r\nz=9\r\n");
        d.set(Some("A"), "k", "2");
        d.set(Some("A"), "n", "3");
        d.set(Some("C"), "q", "4");
        assert_eq!(text(&d), "[A]\r\nk=2\r\nn=3\r\n\r\n[B]\r\nz=9\r\n\r\n[C]\r\nq=4\r\n");
    }

    #[test]
    fn delete_key_and_section() {
        let mut d = doc("[A]\nk=1\nj=2\n[B]\nz=9\n");
        assert!(d.delete_key(Some("A"), "K"));
        assert!(!d.delete_key(Some("A"), "k"));
        assert!(d.delete_section("a"));
        assert_eq!(text(&d), "[B]\nz=9\n");
        assert!(!d.delete_section("A"));
    }
}
