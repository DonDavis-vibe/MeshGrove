//! The tags an AI should use: those in the library first, then Meshgrove's suggestions in the
//! user's language; anything else is a new tag, which the review shows as such.

use crate::config::{Config, SuggestedTag};

/// Longest tag name accepted from an AI.
pub const MAX_TAG_CHARS: usize = 40;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TagStatus {
    /// Already used in the library.
    Existing,
    /// One of Meshgrove's suggestions, not used yet.
    Suggested,
    /// Made up by the AI.
    New,
}

/// The tags known to a library and its settings.
pub struct Vocabulary {
    /// Tags in use, as spelled in the library.
    pub existing: Vec<String>,
    /// Suggested tags with their group ("theme", "property", "state").
    pub suggested: Vec<(SuggestedTag, &'static str)>,
    /// "de" or "en": the language suggestions are given in.
    pub german: bool,
}

impl Vocabulary {
    pub fn new(config: &Config, existing: Vec<String>) -> Vocabulary {
        let mut suggested = Vec::new();
        for (list, group) in [(config.themes(), "theme"), (config.properties(), "property"), (config.states(), "state")] {
            suggested.extend(list.into_iter().map(|t| (t, group)));
        }
        let german = config.language.as_deref().is_none_or(|l| l.starts_with("de"));
        Vocabulary { existing, suggested, german }
    }

    /// The group of a tag: theme, property, state, or own.
    pub fn group_of(&self, tag: &str) -> &'static str {
        self.suggested
            .iter()
            .find(|(t, _)| t.de.eq_ignore_ascii_case(tag) || t.en.eq_ignore_ascii_case(tag))
            .map_or("own", |(_, g)| g)
    }

    /// The spelling to use for `tag`: an existing tag of the same name (any case), else a
    /// suggestion (in German or English, given in the user's language), else the tag as written.
    pub fn normalize(&self, tag: &str) -> (String, TagStatus) {
        let tag = tag.trim();
        if let Some(e) = self.existing.iter().find(|e| e.eq_ignore_ascii_case(tag)) {
            return (e.clone(), TagStatus::Existing);
        }
        if let Some((s, _)) =
            self.suggested.iter().find(|(s, _)| s.de.eq_ignore_ascii_case(tag) || s.en.eq_ignore_ascii_case(tag))
        {
            // A translation of a tag already in use is that tag.
            for name in [&s.de, &s.en] {
                if let Some(e) = self.existing.iter().find(|e| e.eq_ignore_ascii_case(name)) {
                    return (e.clone(), TagStatus::Existing);
                }
            }
            let name = if self.german { &s.de } else { &s.en };
            return (name.clone(), TagStatus::Suggested);
        }
        (tag.to_owned(), TagStatus::New)
    }
}

/// Whether an AI's tag is acceptable at all: short, one line, no control characters.
pub fn valid_tag(tag: &str) -> bool {
    let tag = tag.trim();
    !tag.is_empty() && tag.chars().count() <= MAX_TAG_CHARS && !tag.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn existing_tags_win_then_suggestions_in_the_users_language() {
        let mut config = Config { language: Some("de".into()), ..Default::default() };
        let v = Vocabulary::new(&config, vec!["Werkstatt".into(), "Pirat".into()]);
        assert_eq!(v.normalize("werkstatt"), ("Werkstatt".into(), TagStatus::Existing));
        assert_eq!(v.normalize("Workshop"), ("Werkstatt".into(), TagStatus::Existing), "translation of one in use");
        assert_eq!(v.normalize("garden"), ("Garten".into(), TagStatus::Suggested));
        assert_eq!(v.normalize(" Drachen "), ("Drachen".into(), TagStatus::New));
        assert_eq!(v.group_of("Garten"), "theme");
        assert_eq!(v.group_of("PLA"), "property");
        assert_eq!(v.group_of("Pirat"), "own");
        config.language = Some("en".into());
        let v = Vocabulary::new(&config, Vec::new());
        assert_eq!(v.normalize("Garten"), ("Garden".into(), TagStatus::Suggested));
        assert!(valid_tag("Print-in-Place") && !valid_tag("") && !valid_tag("a\nb") && !valid_tag(&"x".repeat(41)));
    }
}
