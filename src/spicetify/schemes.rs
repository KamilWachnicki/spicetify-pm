//! Port of the marketplace's `parseIni` used for `color.ini` scheme files:
//! `[section]` headers, `key = value` pairs, `;` comments, `xrdb` lines skipped.

use std::collections::BTreeMap;

pub type SchemeIni = BTreeMap<String, BTreeMap<String, String>>;

pub fn parse_ini(data: &str) -> SchemeIni {
    let section_re = |line: &str| -> Option<String> {
        let t = line.trim();
        t.strip_prefix('[')
            .and_then(|r| r.strip_suffix(']'))
            .map(|s| s.trim().to_owned())
    };
    let param_re = |line: &str| -> Option<(String, String)> {
        let (key, val) = line.split_once('=')?;
        let key = key.trim();
        if key.is_empty() {
            return None;
        }
        let val = val.trim();
        let val = match val.find(';') {
            Some(i) => val[..i].trim(),
            None => val,
        };
        Some((key.to_owned(), val.to_owned()))
    };

    let mut result = SchemeIni::new();
    let mut section: Option<String> = None;
    for line in data.split(['\n', '\r']) {
        let trimmed = line.trim();
        if trimmed.starts_with(';') {
            continue;
        }
        if trimmed.contains("xrdb") {
            continue;
        }
        if let Some((k, v)) = param_re(trimmed) {
            if let Some(sec) = &section {
                result.entry(sec.clone()).or_default().insert(k, v);
            }
        } else if let Some(sec) = section_re(trimmed) {
            section = Some(sec);
            result.entry(section.clone().unwrap()).or_default();
        }
    }
    result
}

/// The scheme to restore after a reinstall/lockfile-install: the previous
/// choice when it still exists among `available`, otherwise nothing (the
/// installer's default stands).
pub fn preferred_scheme(previous: Option<&str>, available: &[String]) -> Option<String> {
    previous
        .filter(|s| !s.is_empty())
        .filter(|s| available.iter().any(|a| a == s))
        .map(str::to_owned)
}

/// Serialize a scheme map back to INI (port of `unparseIni`);
/// reserved for scheme-editing features.
#[cfg_attr(not(test), expect(dead_code))]
pub fn unparse_ini(data: &SchemeIni) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    for (name, entries) in data {
        let _ = writeln!(out, "[{name}]");
        for (k, v) in entries {
            let _ = writeln!(out, "{k}={v}");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "; comment\n[base]\ntext=ffffff\nmain=1db954; trailing comment\nxrdb thing=skip\n\n[dark]\ntext=000000\n";

    #[test]
    fn parses_sections_and_strips_inline_comments() {
        let ini = parse_ini(SAMPLE);
        assert_eq!(ini["base"]["text"], "ffffff");
        assert_eq!(ini["base"]["main"], "1db954");
        assert_eq!(ini["dark"]["text"], "000000");
        assert_eq!(ini["base"].len(), 2);
        assert_eq!(ini.len(), 2);
    }

    #[test]
    fn handles_crlf() {
        let ini = parse_ini("[a]\r\nx=1\r\n");
        assert_eq!(ini["a"]["x"], "1");
    }

    #[test]
    fn preferred_scheme_only_when_available() {
        let avail = vec!["Base".to_owned(), "Ocean".to_owned()];
        assert_eq!(
            preferred_scheme(Some("Base"), &avail),
            Some("Base".to_owned())
        );
        assert_eq!(preferred_scheme(Some("Gone"), &avail), None);
        assert_eq!(preferred_scheme(None, &avail), None);
        assert_eq!(preferred_scheme(Some(""), &avail), None);
    }

    #[test]
    fn unparses_back() {
        let mut ini = SchemeIni::new();
        let mut sec = BTreeMap::new();
        sec.insert("text".to_owned(), "ffffff".to_owned());
        ini.insert("base".to_owned(), sec);
        assert_eq!(unparse_ini(&ini), "[base]\ntext=ffffff\n");
    }
}
