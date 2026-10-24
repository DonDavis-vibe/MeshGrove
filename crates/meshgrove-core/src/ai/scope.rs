//! What an AI may see: the libraries the user opened to its access class, and the ids it gets
//! for files there (`<library alias>/<path below the library>`, so no home folders end up in
//! prompts). Everything the AI tools return goes through a [`Scope`].

use std::net::IpAddr;
use std::path::{Component, Path, PathBuf};

use crate::config::{AccessClass, Config};
use crate::listing;

/// A library the AI may see.
#[derive(Debug, Clone, PartialEq)]
pub struct ScopedRoot {
    /// Its name in ids: the folder name, made unique.
    pub alias: String,
    pub path: PathBuf,
    /// Thumbnails may be sent along.
    pub images: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Scope {
    pub class: AccessClass,
    roots: Vec<ScopedRoot>,
    /// Libraries hidden from this class; they win over a visible library around them.
    hidden: Vec<PathBuf>,
}

impl Scope {
    /// Nothing is visible while AI is switched off.
    pub fn new(config: &Config, class: AccessClass) -> Scope {
        let mut roots = Vec::new();
        let mut hidden = Vec::new();
        for (path, alias) in aliases(&config.library_roots) {
            let library = config.ai.library(&path);
            if config.ai.enabled && library.access.allows(class) {
                roots.push(ScopedRoot { alias, path, images: library.images });
            } else {
                hidden.push(path);
            }
        }
        Scope { class, roots, hidden }
    }

    pub fn roots(&self) -> &[ScopedRoot] {
        &self.roots
    }

    pub fn is_empty(&self) -> bool {
        self.roots.is_empty()
    }

    /// The visible library `path` is in; the innermost library decides, so a hidden library
    /// inside a visible one stays hidden.
    pub fn root_of(&self, path: &Path) -> Option<&ScopedRoot> {
        let visible = self.roots.iter().filter(|r| path.starts_with(&r.path)).max_by_key(|r| depth(&r.path));
        let hidden = self.hidden.iter().filter(|h| path.starts_with(h)).map(|h| depth(h)).max();
        match (visible, hidden) {
            (Some(v), Some(h)) if h >= depth(&v.path) => None,
            (v, _) => v,
        }
    }

    pub fn allows(&self, path: &Path) -> bool {
        self.root_of(path).is_some()
    }

    pub fn images_allowed(&self, path: &Path) -> bool {
        self.root_of(path).is_some_and(|r| r.images)
    }

    /// `Garten/Rankhilfe/rose.stl` for a file of the library "Garten".
    pub fn id_of(&self, path: &Path) -> Option<String> {
        let root = self.root_of(path)?;
        let rel = path.strip_prefix(&root.path).ok()?;
        let parts: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
        Some(if parts.is_empty() { root.alias.clone() } else { format!("{}/{}", root.alias, parts.join("/")) })
    }

    /// The file an id stands for; `None` for ids outside what this scope may see (also for
    /// `..`, absolute paths and unknown libraries), so the answer never tells hidden from missing.
    pub fn resolve(&self, id: &str) -> Option<PathBuf> {
        let id = id.trim().trim_start_matches("meshgrove:");
        let (alias, rest) = id.split_once('/').unwrap_or((id, ""));
        let root = self.roots.iter().find(|r| r.alias == alias)?;
        let rel = Path::new(rest);
        if rel.components().any(|c| !matches!(c, Component::Normal(_))) {
            return None;
        }
        let path = if rest.is_empty() { root.path.clone() } else { root.path.join(rel) };
        self.allows(&path).then_some(path)
    }

    /// An SQL condition on the folder column `dir` (paths as stored): inside a visible library
    /// and not inside a hidden one. Its values are appended to `args`, numbered after them.
    pub(crate) fn sql(&self, dir: &str, args: &mut Vec<String>) -> String {
        if self.roots.is_empty() {
            return "0".into();
        }
        let below = |root: &Path, args: &mut Vec<String>| below_sql(dir, root, args);
        let visible: Vec<String> = self.roots.iter().map(|r| below(&r.path, args)).collect();
        let mut sql = format!("({})", visible.join(" OR "));
        for h in &self.hidden {
            if self.roots.iter().any(|r| h.starts_with(&r.path) && h != &r.path) {
                sql += &format!(" AND NOT {}", below(h, args));
            }
        }
        sql
    }
}

/// An SQL condition: folder column `dir` is `root` or below it. Values go to `args`.
pub(crate) fn below_sql(dir: &str, root: &Path, args: &mut Vec<String>) -> String {
    let r = root.to_string_lossy().into_owned();
    let sep = std::path::MAIN_SEPARATOR;
    // Everything after "root/" and before "root" + the character after the separator.
    let after = char::from_u32(sep as u32 + 1).unwrap_or('0');
    args.push(r.clone());
    args.push(format!("{r}{sep}"));
    args.push(format!("{r}{after}"));
    let n = args.len();
    format!("({dir} = ?{} OR ({dir} > ?{} AND {dir} < ?{}))", n - 2, n - 1, n)
}

fn depth(path: &Path) -> usize {
    path.components().count()
}

/// Each library's name in ids: its folder name, with "-2", "-3" … where names repeat.
pub fn aliases(roots: &[PathBuf]) -> Vec<(PathBuf, String)> {
    let mut out: Vec<(PathBuf, String)> = Vec::new();
    for root in roots {
        let base = match listing::file_name(root) {
            n if n.is_empty() || n.contains('/') || n.contains('\\') => "Bibliothek".to_owned(),
            n => n,
        };
        let mut alias = base.clone();
        let mut n = 2;
        while out.iter().any(|(_, a)| a.eq_ignore_ascii_case(&alias)) {
            alias = format!("{base}-{n}");
            n += 1;
        }
        out.push((root.clone(), alias));
    }
    out
}

/// Whether an endpoint URL looks local: loopback, `.localhost`/`.local`/`.lan` names, private
/// and link-local networks, CGNAT (Tailscale). Only a first guess; the user decides.
pub fn guess_local(url: &str) -> bool {
    let rest = url.split("://").nth(1).unwrap_or(url);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host_port = authority.rsplit('@').next().unwrap_or_default();
    let host = match host_port.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or_default(),
        None => host_port.split(':').next().unwrap_or_default(),
    }
    .to_lowercase();
    if host == "localhost" || [".localhost", ".local", ".lan", ".home.arpa"].iter().any(|s| host.ends_with(s)) {
        return true;
    }
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => {
            let [a, b, ..] = ip.octets();
            ip.is_loopback() || ip.is_private() || ip.is_link_local() || (a == 100 && (64..128).contains(&b))
        }
        Ok(IpAddr::V6(ip)) => ip.is_loopback() || (ip.segments()[0] & 0xfe00) == 0xfc00,
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AiAccess, LibraryAi};

    fn config(libraries: &[(&str, AiAccess, bool)]) -> Config {
        let mut c =
            Config { library_roots: libraries.iter().map(|(p, ..)| PathBuf::from(p)).collect(), ..Default::default() };
        c.ai.enabled = true;
        c.ai.libraries = libraries
            .iter()
            .map(|&(p, access, images)| LibraryAi { root: p.into(), access, images })
            .collect();
        c
    }

    #[test]
    fn libraries_show_to_the_classes_they_are_open_to() {
        let c = config(&[
            ("/lib/eigene", AiAccess::Cloud, true),
            ("/lib/kunden", AiAccess::LocalOnly, false),
            ("/lib/privat", AiAccess::None, false),
        ]);
        let cloud = Scope::new(&c, AccessClass::Cloud);
        let local = Scope::new(&c, AccessClass::Local);
        let aliases = |s: &Scope| s.roots().iter().map(|r| r.alias.clone()).collect::<Vec<_>>();
        assert_eq!(aliases(&cloud), ["eigene"]);
        assert_eq!(aliases(&local), ["eigene", "kunden"]);
        assert!(!local.allows(Path::new("/lib/privat/x.stl")));
        assert!(cloud.images_allowed(Path::new("/lib/eigene/a.stl")) && !local.images_allowed(Path::new("/lib/kunden/a.stl")));
        let off = Config { ai: crate::config::AiConfig { enabled: false, ..c.ai.clone() }, ..c.clone() };
        assert!(Scope::new(&off, AccessClass::Local).is_empty(), "switched off: nothing");
        let unlisted = Config { library_roots: vec!["/lib/neu".into()], ..c };
        assert!(Scope::new(&unlisted, AccessClass::Local).is_empty(), "new libraries start hidden");
    }

    #[test]
    fn a_hidden_library_inside_a_visible_one_stays_hidden() {
        let c = config(&[("/lib", AiAccess::Cloud, false), ("/lib/kunden", AiAccess::None, false)]);
        let s = Scope::new(&c, AccessClass::Cloud);
        assert!(s.allows(Path::new("/lib/garten/a.stl")));
        assert!(!s.allows(Path::new("/lib/kunden/a.stl")));
        assert_eq!(s.resolve("lib/kunden/a.stl"), None);
        let mut args = Vec::new();
        let sql = s.sql("d.path", &mut args);
        assert!(sql.contains("AND NOT"), "{sql}");
        assert_eq!(args.len(), 6);
    }

    #[test]
    fn ids_round_trip_and_reject_escapes() {
        let c = config(&[("/home/davis/3D", AiAccess::Cloud, false), ("/mnt/ntfs/3D", AiAccess::Cloud, false)]);
        let s = Scope::new(&c, AccessClass::Cloud);
        let file = Path::new("/mnt/ntfs/3D/Garten/rose.stl");
        let id = s.id_of(file).unwrap();
        assert_eq!(id, "3D-2/Garten/rose.stl", "repeated names get a number");
        assert_eq!(s.resolve(&id).as_deref(), Some(file));
        assert_eq!(s.resolve("meshgrove:3D-2/Garten/rose.stl").as_deref(), Some(file), "copied references work");
        let member = Path::new("/home/davis/3D/auftrag.zip/teile/deckel.stl");
        assert_eq!(s.resolve(&s.id_of(member).unwrap()).as_deref(), Some(member), "archive members too");
        assert_eq!(s.resolve("3D"), Some(PathBuf::from("/home/davis/3D")), "the library itself");
        for bad in ["3D/../geheim.txt", "3D//etc/passwd", "/etc/passwd", "unbekannt/a.stl", "3D/./a.stl"] {
            assert_eq!(s.resolve(bad), None, "{bad}");
        }
        assert_eq!(s.id_of(Path::new("/tmp/x.stl")), None);
    }

    #[test]
    fn local_urls_are_guessed() {
        for url in [
            "http://localhost:11434/v1",
            "http://127.0.0.1:1234",
            "http://192.168.1.20:8080/v1",
            "http://10.0.0.5",
            "http://[::1]:11434",
            "http://gpu-box.local:11434",
            "http://100.101.102.103:11434",
            "http://user:pw@172.16.4.4/v1",
        ] {
            assert!(guess_local(url), "{url}");
        }
        for url in ["https://api.openai.com/v1", "https://openrouter.ai/api/v1", "http://8.8.8.8", "http://172.32.0.1"] {
            assert!(!guess_local(url), "{url}");
        }
    }
}
