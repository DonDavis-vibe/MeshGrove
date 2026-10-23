//! Library search syntax: `drache "castle tower" ext:3mf tag:pirat list:markt is:unprinted passt:"Kobra S1"`.
//! Plain words must all appear (case-insensitive) in the file name, its folder path or its note.

use crate::formats::Kind;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Query {
    /// Lower-case words or quoted phrases.
    pub terms: Vec<String>,
    /// From `ext:`/`typ:`/`type:`; empty means any kind.
    pub kinds: Vec<Kind>,
    /// From `tag:`, lower-case.
    pub tags: Vec<String>,
    /// From `list:`/`liste:`, lower-case list names.
    pub lists: Vec<String>,
    /// From `is:`.
    pub marks: Vec<Mark>,
    /// From `fits:`/`passt:`, lower-case printer names: fits on all of them.
    pub fits: Vec<String>,
}

/// `is:` filters on the user's own marks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    Favorite,
    /// Printed at least once.
    Printed,
    /// Never printed successfully.
    Unprinted,
    /// The newest print failed.
    Failed,
    /// Fits on none of the user's printers.
    TooBig,
    /// Something may make printing fail: mesh holes, suspicious size, too big for every printer.
    Problem,
    /// Waiting in the inbox.
    New,
    /// Models and G-code without any tag.
    Untagged,
}

impl Mark {
    fn parse(value: &str) -> Option<Mark> {
        Some(match value {
            "fav" | "favorite" | "favourite" | "favorit" | "favoriten" => Mark::Favorite,
            "printed" | "gedruckt" => Mark::Printed,
            "unprinted" | "ungedruckt" => Mark::Unprinted,
            "failed" | "fehlgeschlagen" => Mark::Failed,
            "toobig" | "too-big" | "zugross" | "zugroß" | "zu-gross" | "zu-groß" => Mark::TooBig,
            "problem" | "problems" | "probleme" | "fehler" => Mark::Problem,
            "new" | "neu" | "inbox" | "eingang" => Mark::New,
            "untagged" | "ohne-tags" | "ohnetags" => Mark::Untagged,
            _ => return None,
        })
    }
}

impl Query {
    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
            && self.kinds.is_empty()
            && self.tags.is_empty()
            && self.lists.is_empty()
            && self.marks.is_empty()
            && self.fits.is_empty()
    }
}

pub fn parse(input: &str) -> Query {
    let mut query = Query::default();
    for token in tokens(input) {
        let lower = token.to_lowercase();
        if let Some((key, value)) = lower.split_once(':')
            && !value.is_empty()
        {
            match key {
                "ext" | "typ" | "type" => {
                    // Unknown types stay plain words, so they match nothing instead of everything.
                    let kind = Kind::from_file_name(&format!("x.{value}"));
                    if kind != Kind::Other {
                        if !query.kinds.contains(&kind) {
                            query.kinds.push(kind);
                        }
                        continue;
                    }
                }
                "tag" => {
                    query.tags.push(value.to_owned());
                    continue;
                }
                "list" | "liste" => {
                    query.lists.push(value.to_owned());
                    continue;
                }
                "fits" | "passt" => {
                    query.fits.push(value.to_owned());
                    continue;
                }
                "is" => {
                    if let Some(mark) = Mark::parse(value) {
                        if !query.marks.contains(&mark) {
                            query.marks.push(mark);
                        }
                        continue;
                    }
                }
                _ => {}
            }
        }
        query.terms.push(lower);
    }
    query
}

/// Splits on whitespace, keeping "quoted phrases" together.
fn tokens(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for c in input.chars() {
        match c {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.trim().is_empty() {
        out.push(current);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_phrases_and_filters() {
        let q = parse(r#"Drache  "Castle Tower" ext:3MF typ:stl tag:Pirat ext:xyz"#);
        assert_eq!(q.terms, ["drache", "castle tower", "ext:xyz"]);
        assert_eq!(q.kinds, [Kind::ThreeMf, Kind::Stl]);
        assert_eq!(q.tags, ["pirat"]);
        assert!(parse("   ").is_empty());
        assert_eq!(parse("größe").terms, ["größe"]);
    }

    #[test]
    fn lists_and_marks() {
        let q = parse(r#"liste:"Weihnachts Markt" list:Geschenke is:FAV is:ungedruckt is:fav is:kaputt"#);
        assert_eq!(q.lists, ["weihnachts markt", "geschenke"]);
        assert_eq!(q.marks, [Mark::Favorite, Mark::Unprinted]);
        assert_eq!(q.terms, ["is:kaputt"], "unknown marks stay words");
        assert!(!parse("is:printed").is_empty());
    }

    #[test]
    fn printer_filters() {
        let q = parse(r#"passt:"Kobra S1" fits:a1 is:zugroß is:TooBig is:neu is:ohne-tags"#);
        assert_eq!(q.fits, ["kobra s1", "a1"]);
        assert_eq!(q.marks, [Mark::TooBig, Mark::New, Mark::Untagged]);
        assert!(!parse("is:zugross").is_empty());
    }
}
