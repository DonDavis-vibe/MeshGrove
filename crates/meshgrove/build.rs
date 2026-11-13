//! Gives meshgrove.exe its icon in Explorer and the taskbar, and embeds the interface themes.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn main() {
    if let Err(e) = builtin_themes() {
        eprintln!("themes: {e}");
        std::process::exit(1);
    }
    if let Err(e) = icon_data() {
        eprintln!("icon data: {e}");
        std::process::exit(1);
    }
    // The translations are compiled in (rust-i18n): edit a locale file, get a new build.
    println!("cargo:rerun-if-changed=locales");
    println!("cargo:rerun-if-changed=../../packaging/icons/meshgrove.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("../../packaging/icons/meshgrove.ico");
        resource.set("ProductName", "Meshgrove");
        resource.set("FileDescription", "Meshgrove - gallery for 3D-print files");
        resource.set("CompanyName", "German Redneck Engineering");
        resource.set("LegalCopyright", "Davis, German Redneck Engineering - GPL-3.0-or-later");
        if let Err(e) = resource.compile() {
            println!("cargo:warning=no icon for meshgrove.exe: {e}");
        }
    }
}

/// SolidCraft's theme files (assets/themes, kept byte-identical to upstream) as a table of
/// `(id, JSON text)` sorted by id, the way SolidCraft's `crates/ui-egui/build.rs` embeds them
/// (MIT OR Apache-2.0). Meshgrove has no workspace tabs, and the kit refuses a file naming a tab
/// it does not know (one refused file drops them all), so per-tab tables are left out here.
fn builtin_themes() -> Result<(), String> {
    // SolidCraft's themes and Meshgrove's own two (assets/themes-meshgrove, made by
    // scripts/meshgrove-themes.py). Ids must not collide across the two.
    let mut files: Vec<PathBuf> = Vec::new();
    for dir in ["themes", "themes-meshgrove"] {
        let dir = assets().join(dir);
        println!("cargo:rerun-if-changed={}", dir.display());
        for entry in std::fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))? {
            let path = entry.map_err(|e| e.to_string())?.path();
            if path.extension().is_some_and(|e| e == "json") {
                files.push(path);
            }
        }
    }
    files.sort_by(|a, b| a.file_stem().cmp(&b.file_stem()));
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").map_err(|e| e.to_string())?);
    let mut out = String::from("pub(crate) const BUILTIN: &[(&str, &str)] = &[\n");
    for mut path in files {
        let id = path.file_stem().and_then(|s| s.to_str()).ok_or("theme file name is not UTF-8")?.to_owned();
        let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut value: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        let object = value.as_object_mut().ok_or_else(|| format!("{}: not an object", path.display()))?;
        if object.remove("tabs").is_some() {
            path = out_dir.join(format!("theme-{id}.json"));
            let text = serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?;
            std::fs::write(&path, text).map_err(|e| e.to_string())?;
        }
        let path = path.canonicalize().map_err(|e| e.to_string())?;
        out += &format!("    ({id:?}, include_str!({:?})),\n", path.display().to_string());
    }
    out += "];\n";
    std::fs::write(out_dir.join("builtin_themes.rs"), out).map_err(|e| e.to_string())
}

fn assets() -> PathBuf {
    Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("../../assets")
}

/// The kit's icon set (assets/icons: the palette's roles and accents, which theme files name,
/// and the SVG masters), compiled as SolidCraft's `crates/ui-egui/build.rs` does (`icon_data`,
/// MIT OR Apache-2.0), here in the form LightGrove carries it.
fn icon_data() -> Result<(), String> {
    let root = assets().join("icons");
    println!("cargo:rerun-if-changed={}", root.display());
    let palette = std::fs::read_to_string(root.join("palette.toml")).map_err(|e| format!("palette.toml: {e}"))?;
    let palette = palette.parse::<toml::Table>().map_err(|e| format!("palette.toml: {e}"))?;
    let hex = |v: Option<&toml::Value>, what: &str| -> Result<String, String> {
        let s = v.and_then(toml::Value::as_str).ok_or_else(|| format!("{what}: missing hex"))?;
        let ok = s.len() == 7 && s.starts_with('#') && s[1..].bytes().all(|b| b.is_ascii_hexdigit());
        if ok { Ok(s.to_ascii_lowercase()) } else { Err(format!("{what}: `{s}` is not #rrggbb")) }
    };
    let rgb = |h: &str| format!("[0x{}, 0x{}, 0x{}]", &h[1..3], &h[3..5], &h[5..7]);
    let mut out = String::new();

    let roles = palette.get("roles").and_then(toml::Value::as_table).ok_or("palette.toml: no [roles]")?;
    let names: Vec<&String> = roles.keys().collect();
    let _ = writeln!(out, "pub(crate) const ROLE_NAMES: &[&str] = &{names:?};");
    let (mut lights, mut darks, mut source_hex) = (Vec::new(), Vec::new(), Vec::new());
    for (name, role) in roles {
        lights.push(rgb(&hex(role.get("light"), &format!("role {name} light"))?));
        darks.push(rgb(&hex(role.get("dark"), &format!("role {name} dark"))?));
        source_hex.push(hex(role.get("source").or_else(|| role.get("light")), &format!("role {name} source"))?);
    }
    let _ = writeln!(out, "pub(crate) const ROLE_SOURCE_HEX: &[&str] = &{source_hex:?};");
    let _ = writeln!(
        out,
        "pub(crate) const PALETTE: [&[[u8; 3]]; 2] = [&[{}], &[{}]];",
        lights.join(", "),
        darks.join(", ")
    );

    let mut accent_names = Vec::new();
    let mut accents = Vec::new();
    for (accent, roles_of) in palette.get("accents").and_then(toml::Value::as_table).into_iter().flatten() {
        let roles_of = roles_of.as_table().ok_or_else(|| format!("accent {accent}: not a table"))?;
        let mut items = Vec::new();
        for (role, v) in roles_of {
            let idx =
                names.iter().position(|n| *n == role).ok_or_else(|| format!("accent {accent}: unknown role {role}"))?;
            let v = match v.get("use").and_then(toml::Value::as_str) {
                Some(link) => {
                    roles_of.get(link).ok_or_else(|| format!("accent {accent}: unknown linked role {link}"))?
                }
                None => v,
            };
            let l = hex(v.get("light"), &format!("accent {accent} {role} light"))?;
            let d = hex(v.get("dark"), &format!("accent {accent} {role} dark"))?;
            items.push(format!("({idx}, {}, {})", rgb(&l), rgb(&d)));
        }
        accent_names.push(accent.clone());
        accents.push(format!("&[{}]", items.join(", ")));
    }
    let _ = writeln!(out, "pub(crate) const ACCENT_NAMES: &[&str] = &{accent_names:?};");
    let _ = writeln!(out, "pub(crate) const ACCENTS: &[&[(usize, [u8; 3], [u8; 3])]] = &[{}];", accents.join(", "));

    let mut variants = Vec::new();
    for (vision, group) in palette.get("variants").and_then(toml::Value::as_table).into_iter().flatten() {
        if !matches!(vision.as_str(), "red-green" | "blue-yellow") {
            return Err(format!("unknown icon colour-vision group {vision}"));
        }
        for (accent, overrides) in group.as_table().ok_or("invalid variant group")? {
            let a = accent_names
                .iter()
                .position(|n| n == accent)
                .ok_or_else(|| format!("variant: unknown accent {accent}"))?;
            for (role, value) in overrides.as_table().ok_or("invalid variant roles")? {
                let r = names.iter().position(|n| *n == role).ok_or_else(|| format!("variant: unknown role {role}"))?;
                let l = hex(value.get("light"), "variant light")?;
                let d = hex(value.get("dark"), "variant dark")?;
                variants.push(format!("({vision:?}, {a}, {r}, {}, {})", rgb(&l), rgb(&d)));
            }
        }
    }
    let _ = writeln!(
        out,
        "pub(crate) const ACCENT_VARIANTS: &[(&str, usize, usize, [u8; 3], [u8; 3])] = &[{}];",
        variants.join(", ")
    );

    // The masters, by family, as upstream; only the families we copied exist.
    let mut svgs = Vec::new();
    let mut ids = std::collections::BTreeSet::new();
    for family in ["model", "small", "flat", "workspace"] {
        let dir = root.join("svg").join(family);
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        let mut files: Vec<_> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
        files.sort();
        for f in files.iter().filter(|f| f.extension().is_some_and(|e| e == "svg")) {
            let id = f.file_stem().and_then(|s| s.to_str()).ok_or("bad file name")?;
            if family != "small" {
                ids.insert(id.to_owned());
            }
            let abs = f.canonicalize().map_err(|e| e.to_string())?;
            svgs.push(format!("({id:?}, {family:?}, include_str!({:?}))", abs.display().to_string()));
        }
    }
    let _ = writeln!(out, "pub(crate) static SVGS: &[(&str, &str, &str)] = &[{}];", svgs.join(", "));
    // Upstream's aliases (`warning` -> status_warning, ...), for the masters we copied.
    let aliases = std::fs::read_to_string(root.join("aliases.toml")).map_err(|e| format!("aliases.toml: {e}"))?;
    let aliases = aliases.parse::<toml::Table>().map_err(|e| format!("aliases.toml: {e}"))?;
    let mut entries = Vec::new();
    for (alias, v) in aliases.get("alias").and_then(toml::Value::as_table).into_iter().flatten() {
        let (master, accent) = match v {
            toml::Value::String(m) => (m.clone(), String::new()),
            toml::Value::Table(t) => (
                t.get("master")
                    .and_then(toml::Value::as_str)
                    .ok_or_else(|| format!("alias {alias}: no master"))?
                    .to_owned(),
                t.get("accent").and_then(toml::Value::as_str).unwrap_or("").to_owned(),
            ),
            _ => return Err(format!("alias {alias}: expected a string or a table")),
        };
        if ids.contains(&master) && (accent.is_empty() || accent_names.contains(&accent)) {
            entries.push(format!("({alias:?}, {master:?}, {accent:?})"));
        }
    }
    let _ = writeln!(out, "pub(crate) const ALIASES: &[(&str, &str, &str)] = &[{}];", entries.join(", "));

    // Default accents for the icons we have; upstream's palette lists hundreds we did not copy.
    let mut defaults = std::collections::BTreeMap::new();
    for (accent, icons) in palette.get("icon-accents").and_then(toml::Value::as_table).into_iter().flatten() {
        for icon in icons.as_array().ok_or("icon-accents: expected icon array")? {
            let icon = icon.as_str().ok_or("icon-accents: expected icon id")?;
            if ids.contains(icon) {
                defaults.insert(icon.to_owned(), accent.clone());
            }
        }
    }
    let defaults: Vec<_> = defaults.iter().map(|(id, accent)| format!("({id:?}, {accent:?})")).collect();
    let _ = writeln!(out, "pub(crate) const ICON_ACCENTS: &[(&str, &str)] = &[{}];", defaults.join(", "));
    let dest = Path::new(&std::env::var("OUT_DIR").map_err(|e| e.to_string())?).join("icon_data.rs");
    std::fs::write(dest, out).map_err(|e| e.to_string())
}
