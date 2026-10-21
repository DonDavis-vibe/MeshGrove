//! Library search syntax: `drache "castle tower" ext:3mf tag:pirat`.
//! Plain words must all appear (case-insensitive) in the file name or its folder path.

use crate::formats::Kind;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Query {
    /// Lower-case words or quoted phrases.
    pub terms: Vec<String>,
    /// From `ext:`/`typ:`/`type:`; empty means any kind.
    pub kinds: Vec<Kind>,
    /// From `tag:`, lower-case.
    pub tags: Vec<String>,
}

impl Query {
    pub fn is_empty(&self) -> bool {
        self.terms.is_empty() && self.kinds.is_empty() && self.tags.is_empty()
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
}
