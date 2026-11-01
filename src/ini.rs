//! Leitura/escrita de arquivos INI (substitui IniRead, IniReadSection,
//! IniWrite, IniDelete e as rotinas manuais de `iniread`/`iniwrite`).
//!
//! O arquivo é manipulado como lista de linhas, preservando comentários,
//! terminadores de linha e codificação (UTF-8/UTF-16/ANSI).

use crate::util::{self, Enc};

pub struct IniDoc {
    pub lines: Vec<(String, String)>, // (conteúdo, terminador)
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

/// Divide "chave = valor" (primeiro `=`), com trim em ambos.
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

/// Remove aspas externas como faz GetPrivateProfileString (IniRead).
fn strip_quotes(v: &str) -> String {
    let b = v.as_bytes();
    if b.len() >= 2 && ((b[0] == b'"' && b[b.len() - 1] == b'"') || (b[0] == b'\'' && b[b.len() - 1] == b'\'')) {
        v[1..v.len() - 1].to_string()
    } else {
        v.to_string()
    }
}

impl IniDoc {
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

    /// Faixa [início, fim) de linhas de corpo da seção (`None` = área global,
    /// antes da primeira seção). `Some(idx_cabecalho)` também é retornado.
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

    /// IniReadSection: pares (chave, valor) em ordem, sem tratar aspas.
    pub fn section_pairs(&self, section: &str) -> Option<Vec<(String, String)>> {
        let (a, b) = self.section_range(Some(section))?;
        let v: Vec<_> = self.lines[a..b].iter().filter_map(|(l, _)| kv(l)).collect();
        Some(v)
    }

    /// IniRead: primeiro valor da chave (aspas externas removidas).
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

    /// IniWrite / escrita global.
    pub fn set(&mut self, section: Option<&str>, key: &str, value: &str) {
        let text = format!("{}={}", key, value);
        let eol = self.eol();
        if let Some(i) = self.find_key(section, key) {
            self.lines[i].0 = text;
            return;
        }
        match section {
            None => {
                // Antes do original: inseria antes da primeira linha que não é
                // comentário (e nada ocorria em arquivo vazio). Agora também
                // funciona em arquivo vazio/só com comentários.
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

    /// IniDelete(chave). Retorna true se removeu.
    pub fn delete_key(&mut self, section: Option<&str>, key: &str) -> bool {
        if let Some(i) = self.find_key(section, key) {
            self.lines.remove(i);
            true
        } else {
            false
        }
    }

    /// Remove todas as chaves da área global (`--NULL`).
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

    /// IniDelete(seção inteira).
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

/// Atalho: IniRead(arquivo, seção, chave, padrão).
pub fn read(path: &str, section: &str, key: &str, default: &str) -> String {
    IniDoc::load(path).get(Some(section), key).unwrap_or_else(|| default.to_string())
}
