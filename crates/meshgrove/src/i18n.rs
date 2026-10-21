//! Language selection and locale-aware number formatting.
//! German is the default; English is used when the system (or the user) asks for it.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    De,
    En,
}

impl Lang {
    fn from_tag(tag: &str) -> Lang {
        if tag.to_ascii_lowercase().starts_with("en") { Lang::En } else { Lang::De }
    }

    fn code(self) -> &'static str {
        match self {
            Lang::De => "de",
            Lang::En => "en",
        }
    }

    fn current() -> Lang {
        Lang::from_tag(&rust_i18n::locale())
    }

    fn decimal_sep(self) -> char {
        if self == Lang::De { ',' } else { '.' }
    }

    fn thousands_sep(self) -> char {
        if self == Lang::De { '.' } else { ',' }
    }
}

/// `override_tag` comes from the settings ("de"/"en"); `None` follows the system locale.
pub fn init(override_tag: Option<&str>) {
    let tag = override_tag.map(str::to_owned).or_else(sys_locale::get_locale).unwrap_or_default();
    rust_i18n::set_locale(Lang::from_tag(&tag).code());
}

/// "12,5" (de) / "12.5" (en) with one decimal — model dimensions in mm.
pub fn decimal(value: f32) -> String {
    decimal_in(Lang::current(), value)
}

/// "1.234.567" (de) / "1,234,567" (en).
pub fn count(n: usize) -> String {
    count_in(Lang::current(), n)
}

/// "1,4 MB" (de) / "1.4 MB" (en), decimal units like Dolphin.
pub fn file_size(bytes: u64) -> String {
    file_size_in(Lang::current(), bytes)
}

/// "21.10.2026 14:05" (de) / "2026-10-21 14:05" (en), local time.
pub fn datetime(time: std::time::SystemTime) -> String {
    let local: chrono::DateTime<chrono::Local> = time.into();
    let pattern = if Lang::current() == Lang::De { "%d.%m.%Y %H:%M" } else { "%Y-%m-%d %H:%M" };
    local.format(pattern).to_string()
}

/// "20,0 × 10,0 × 5,0 mm"
pub fn dimensions(size: glam::Vec3) -> String {
    format!("{} × {} × {} mm", decimal(size.x), decimal(size.y), decimal(size.z))
}

fn decimal_in(lang: Lang, value: f32) -> String {
    format!("{value:.1}").replace('.', &lang.decimal_sep().to_string())
}

fn count_in(lang: Lang, n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(lang.thousands_sep());
        }
        out.push(ch);
    }
    out
}

fn file_size_in(lang: Lang, bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    format!("{} {}", decimal_in(lang, value as f32), UNITS[unit])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn german_number_formats() {
        assert_eq!(decimal_in(Lang::De, 12.46), "12,5");
        assert_eq!(count_in(Lang::De, 1234), "1.234");
        assert_eq!(count_in(Lang::De, 4_192_676), "4.192.676");
        assert_eq!(count_in(Lang::De, 999), "999");
        assert_eq!(file_size_in(Lang::De, 1_400_000), "1,4 MB");
        assert_eq!(file_size_in(Lang::De, 209_600_000), "209,6 MB");
        assert_eq!(file_size_in(Lang::De, 512), "512 B");
    }

    #[test]
    fn english_number_formats() {
        assert_eq!(decimal_in(Lang::En, 12.46), "12.5");
        assert_eq!(count_in(Lang::En, 4_192_676), "4,192,676");
        assert_eq!(file_size_in(Lang::En, 1_400_000), "1.4 MB");
    }

    #[test]
    fn language_detection_defaults_to_german() {
        assert_eq!(Lang::from_tag("de-DE"), Lang::De);
        assert_eq!(Lang::from_tag("en-US"), Lang::En);
        assert_eq!(Lang::from_tag("fr-FR"), Lang::De);
        assert_eq!(Lang::from_tag(""), Lang::De);
    }

    /// Every `t!("key")` used in the code must exist in the German file (and so in both).
    #[test]
    fn every_used_key_is_translated() {
        let de = include_str!("../locales/de.yml");
        let known: std::collections::HashSet<&str> =
            de.lines().filter_map(|l| l.split_once(':').map(|(k, _)| k.trim())).collect();
        let mut missing = Vec::new();
        let mut stack = vec![std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/src"))];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                let code = std::fs::read_to_string(&path).unwrap();
                for line in code.lines().filter(|l| !l.trim_start().starts_with("//")) {
                    for (i, _) in line.match_indices("t!(\"") {
                        // `format!(` ends in `t!(` too: require a word boundary before the macro.
                        if line[..i].chars().last().is_some_and(|c| c.is_alphanumeric() || c == '_') {
                            continue;
                        }
                        let rest = &line[i + 4..];
                        let key = &rest[..rest.find('"').unwrap_or(rest.len())];
                        if !known.contains(key) {
                            missing.push(format!("{key} ({})", path.file_name().unwrap().to_string_lossy()));
                        }
                    }
                }
            }
        }
        missing.sort();
        missing.dedup();
        assert!(missing.is_empty(), "keys missing in de.yml: {missing:#?}");
    }

    /// Every key must exist in both locale files, so no string silently falls back.
    #[test]
    fn locale_files_have_the_same_keys() {
        let keys = |text: &str| -> std::collections::BTreeSet<String> {
            text.lines()
                .filter_map(|l| l.split_once(':').map(|(k, _)| k.trim().to_owned()))
                .filter(|k| !k.is_empty() && !k.starts_with('#') && k != "_version")
                .collect()
        };
        let de = keys(include_str!("../locales/de.yml"));
        let en = keys(include_str!("../locales/en.yml"));
        assert_eq!(de.difference(&en).collect::<Vec<_>>(), Vec::<&String>::new(), "missing in en.yml");
        assert_eq!(en.difference(&de).collect::<Vec<_>>(), Vec::<&String>::new(), "missing in de.yml");
    }
}
