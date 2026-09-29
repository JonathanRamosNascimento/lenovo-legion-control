//! Translations for the Legion Control user interfaces (CLI and GUI).
//!
//! English is the source language: every message in the code base is
//! written in English and translated **at presentation time**. This keeps
//! the daemon (which runs as root with the system locale) and the JSON
//! output language-neutral, while the CLI and GUI show messages in the
//! user's language — including messages that arrive from the daemon.
//!
//! * [`t`] translates a static literal (exact match, zero allocation).
//! * [`tr`] translates any message, including formatted ones, using the
//!   `{}` templates of the catalog (e.g. `"fan {} does not exist (1..={})"`)
//!   and, as a fallback, translating `"; "`, `" — "` and `", "`-separated
//!   segments independently. Unknown text is returned unchanged.
//!
//! The language comes from [`set_lang`], or [`detect`] (`LEGION_LANG`,
//! `LANGUAGE`, `LC_ALL`, `LC_MESSAGES`, `LANG`).

use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

mod catalog;

/// Supported languages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Lang {
    /// English (source language).
    En,
    /// Brazilian Portuguese.
    PtBr,
}

impl Lang {
    /// All languages, for selectors.
    pub const ALL: [Lang; 2] = [Lang::En, Lang::PtBr];

    /// BCP-47-ish code (`en`, `pt-BR`).
    pub fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::PtBr => "pt-BR",
        }
    }

    /// Name in the language itself.
    pub fn native_name(self) -> &'static str {
        match self {
            Lang::En => "English",
            Lang::PtBr => "Português (Brasil)",
        }
    }

    /// Parses a locale or language code (`pt_BR.UTF-8`, `pt-BR`, `pt`,
    /// `en_US`, `C`). Unknown languages return `None`.
    pub fn parse(s: &str) -> Option<Lang> {
        let s = s.trim().to_ascii_lowercase();
        let base = s.split(['.', '@']).next().unwrap_or("").replace('-', "_");
        match base.as_str() {
            "" => None,
            "c" | "posix" => Some(Lang::En),
            b if b == "en" || b.starts_with("en_") => Some(Lang::En),
            b if b == "pt" || b.starts_with("pt_") => Some(Lang::PtBr),
            _ => None,
        }
    }
}

const UNSET: u8 = 0;
static CURRENT: AtomicU8 = AtomicU8::new(UNSET);

fn encode(l: Lang) -> u8 {
    match l {
        Lang::En => 1,
        Lang::PtBr => 2,
    }
}

/// Language from the environment (`LEGION_LANG` first, then the usual
/// POSIX locale variables). Defaults to English.
pub fn detect() -> Lang {
    detect_from(|k| std::env::var(k).ok())
}

/// [`detect`] with an injectable environment (for tests).
pub fn detect_from(env: impl Fn(&str) -> Option<String>) -> Lang {
    if let Some(l) = env("LEGION_LANG").and_then(|v| Lang::parse(&v)) {
        return l;
    }
    // LANGUAGE is a priority list, and is ignored when LC_ALL/LANG is "C"
    if let Some(list) = env("LANGUAGE") {
        if let Some(l) = list.split(':').find_map(Lang::parse) {
            return l;
        }
    }
    for k in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Some(v) = env(k).filter(|v| !v.is_empty()) {
            return Lang::parse(&v).unwrap_or(Lang::En);
        }
    }
    Lang::En
}

/// Sets the language for the whole process.
pub fn set_lang(l: Lang) {
    CURRENT.store(encode(l), Ordering::Relaxed);
}

/// Current language (detected on first use when not set).
pub fn lang() -> Lang {
    match CURRENT.load(Ordering::Relaxed) {
        1 => Lang::En,
        2 => Lang::PtBr,
        _ => {
            let l = detect();
            set_lang(l);
            l
        }
    }
}

struct Compiled {
    exact: HashMap<&'static str, &'static str>,
    templates: Vec<(Vec<&'static str>, &'static str)>,
}

fn compiled() -> &'static Compiled {
    static C: OnceLock<Compiled> = OnceLock::new();
    C.get_or_init(|| {
        let mut exact = HashMap::new();
        let mut templates = Vec::new();
        for (en, pt) in catalog::PT_BR {
            if en.contains("{}") {
                templates.push((en.split("{}").collect(), *pt));
            } else {
                exact.insert(*en, *pt);
            }
        }
        Compiled { exact, templates }
    })
}

/// Translates a static literal (exact match only).
pub fn t(s: &'static str) -> &'static str {
    t_in(lang(), s)
}

/// [`t`] for an explicit language.
pub fn t_in(l: Lang, s: &'static str) -> &'static str {
    match l {
        Lang::En => s,
        Lang::PtBr => compiled().exact.get(s).copied().unwrap_or(s),
    }
}

/// Matches `s` against a template split on `{}`; returns the captures.
fn match_template<'a>(parts: &[&str], s: &'a str) -> Option<Vec<&'a str>> {
    let mut rest = s.strip_prefix(parts[0])?;
    let mut caps = Vec::with_capacity(parts.len() - 1);
    let last = parts.len() - 1;
    for (i, lit) in parts.iter().enumerate().skip(1) {
        if i == last {
            // the final literal must be a suffix
            let cap = rest.strip_suffix(lit)?;
            caps.push(cap);
            rest = "";
        } else {
            if lit.is_empty() {
                return None; // adjacent placeholders are ambiguous
            }
            let pos = rest.find(lit)?;
            caps.push(&rest[..pos]);
            rest = &rest[pos + lit.len()..];
        }
    }
    (rest.is_empty() && caps.iter().all(|c| !c.is_empty())).then_some(caps)
}

/// Fills `{}` (sequential) or `{0}`, `{1}`... (indexed) placeholders.
fn fill(template: &str, args: &[String]) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut next = 0;
    let mut chars = template.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if c == '{' {
            let close = template[i..].find('}').map(|p| i + p);
            if let Some(end) = close {
                let inner = &template[i + 1..end];
                let idx = if inner.is_empty() {
                    let n = next;
                    next += 1;
                    Some(n)
                } else {
                    inner.parse::<usize>().ok()
                };
                if let Some(arg) = idx.and_then(|n| args.get(n)) {
                    out.push_str(arg);
                    while chars.peek().is_some_and(|(j, _)| *j <= end) {
                        chars.next();
                    }
                    continue;
                }
            }
        }
        out.push(c);
    }
    out
}

const SEPARATORS: [&str; 3] = ["; ", " — ", ", "];

fn translate(s: &str, depth: u8) -> String {
    let c = compiled();
    if let Some(p) = c.exact.get(s) {
        return (*p).to_owned();
    }
    let trimmed = s.trim_end_matches(['.', ' ']);
    if trimmed.len() != s.len() {
        if let Some(p) = c.exact.get(trimmed) {
            return format!("{p}{}", &s[trimmed.len()..]);
        }
    }
    if depth == 0 {
        return s.to_owned();
    }
    for (parts, pt) in &c.templates {
        if let Some(caps) = match_template(parts, s) {
            let args: Vec<String> = caps.iter().map(|a| translate(a, depth - 1)).collect();
            return fill(pt, &args);
        }
    }
    for sep in SEPARATORS {
        if s.contains(sep) {
            return s
                .split(sep)
                .map(|seg| translate(seg, depth - 1))
                .collect::<Vec<_>>()
                .join(sep);
        }
    }
    s.to_owned()
}

/// Translates any message (see the crate documentation).
pub fn tr(s: &str) -> String {
    tr_in(lang(), s)
}

/// [`tr`] for an explicit language.
pub fn tr_in(l: Lang, s: &str) -> String {
    match l {
        Lang::En => s.to_owned(),
        Lang::PtBr => {
            // translate line by line so multi-line messages work too
            s.split('\n')
                .map(|l| translate(l, 4))
                .collect::<Vec<_>>()
                .join("\n")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn pt(s: &str) -> String {
        tr_in(Lang::PtBr, s)
    }

    #[test]
    fn parses_locales() {
        assert_eq!(Lang::parse("pt_BR.UTF-8"), Some(Lang::PtBr));
        assert_eq!(Lang::parse("pt-BR"), Some(Lang::PtBr));
        assert_eq!(Lang::parse("pt"), Some(Lang::PtBr));
        assert_eq!(Lang::parse("en_US.UTF-8"), Some(Lang::En));
        assert_eq!(Lang::parse("C"), Some(Lang::En));
        assert_eq!(Lang::parse("de_DE"), None);
        assert_eq!(Lang::parse(""), None);
    }

    #[test]
    fn detection_order() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                pairs
                    .iter()
                    .find(|(a, _)| *a == k)
                    .map(|(_, v)| v.to_string())
            }
        };
        assert_eq!(detect_from(env(&[("LANG", "pt_BR.UTF-8")])), Lang::PtBr);
        assert_eq!(
            detect_from(env(&[("LANG", "pt_BR.UTF-8"), ("LEGION_LANG", "en")])),
            Lang::En
        );
        assert_eq!(
            detect_from(env(&[("LANGUAGE", "de:pt_BR"), ("LANG", "en_US")])),
            Lang::PtBr
        );
        assert_eq!(
            detect_from(env(&[("LC_ALL", "C"), ("LANG", "pt_BR")])),
            Lang::En
        );
        assert_eq!(detect_from(env(&[])), Lang::En);
    }

    #[test]
    fn catalog_is_consistent() {
        let mut seen = HashSet::new();
        for (en, pt) in catalog::PT_BR {
            assert!(seen.insert(*en), "duplicate catalog key: {en:?}");
            assert!(!pt.is_empty(), "empty translation for {en:?}");
            let holes = en.matches("{}").count();
            let seq = pt.matches("{}").count();
            let idx = (0..holes)
                .filter(|i| pt.contains(&format!("{{{i}}}")))
                .count();
            assert!(
                seq == holes || idx == holes,
                "placeholder mismatch: {en:?} -> {pt:?}"
            );
            if en.contains("{}") {
                assert!(!en.contains("{}{}"), "adjacent placeholders in {en:?}");
            }
        }
    }

    #[test]
    fn translates_exact_templates_and_segments() {
        assert_eq!(
            pt("Not supported on this hardware."),
            "Não suportado neste hardware."
        );
        assert_eq!(
            pt("fan target must be 0 (auto) or between 1800 and 4600 RPM, got 100"),
            "o alvo da ventoinha deve ser 0 (automático) ou estar entre 1800 e 4600 RPM; recebido 100"
        );
        // nested: template capture translated too
        assert_eq!(
            pt("Fan 1: 4000 RPM, Fan 2: 3900 RPM"),
            "Ventoinha 1: 4000 RPM, Ventoinha 2: 3900 RPM"
        );
        // segments separated by "; "
        assert_eq!(
            pt(
                "manual fan control is disabled; set \"enable_fan_control\": true in /etc/legion-control/daemon.json"
            ),
            "o controle manual das ventoinhas está desativado; defina \"enable_fan_control\": true em /etc/legion-control/daemon.json"
        );
        assert_eq!(
            pt("BAT0 75% (Not charging), health 102%, 55 cycles"),
            "BAT0 75% (Não está carregando), saúde 102%, 55 ciclos"
        );
        // unknown text is untouched
        assert_eq!(pt("lenovo_wmi_other"), "lenovo_wmi_other");
        assert_eq!(
            pt("37 sensors; CPU 69 °C, GPU 51 °C"),
            "37 sensores; CPU 69 °C, GPU 51 °C"
        );
        // multi-line
        assert_eq!(
            pt("Idle\nNot supported on this hardware."),
            "Ocioso\nNão suportado neste hardware."
        );
    }

    #[test]
    fn english_is_identity() {
        assert_eq!(
            tr_in(Lang::En, "Not supported on this hardware."),
            "Not supported on this hardware."
        );
        assert_eq!(t_in(Lang::En, "Battery"), "Battery");
        assert_eq!(t_in(Lang::PtBr, "Battery"), "Bateria");
    }

    #[test]
    fn fill_supports_indexed_placeholders() {
        let a = vec!["x".to_string(), "y".to_string()];
        assert_eq!(fill("{1} then {0}", &a), "y then x");
        assert_eq!(fill("{} and {}", &a), "x and y");
        assert_eq!(fill("keep {braces}", &a), "keep {braces}");
    }
}
