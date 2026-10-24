//! Picking from candidates while typing, shared by the tag field and the chat's @ references:
//! what matches the typed text, in which order, and the highlight that Up/Down move.

use egui::{Key, Ui};

#[derive(Debug, Clone, PartialEq)]
pub struct Candidate<T> {
    pub label: String,
    /// Shown after the label (what kind it is, how many files …).
    pub detail: String,
    /// Something the library has, as opposed to a suggestion: listed first among equals.
    pub existing: bool,
    pub value: T,
}

/// The candidates whose label contains `needle` (any case): those starting with it first, then
/// existing ones before suggestions, otherwise in the given order; at most `max`.
pub fn rank<T>(needle: &str, mut items: Vec<Candidate<T>>, max: usize) -> Vec<Candidate<T>> {
    let needle = needle.trim().to_lowercase();
    if !needle.is_empty() {
        items.retain(|c| c.label.to_lowercase().contains(&needle));
        items.sort_by_key(|c| (!c.label.to_lowercase().starts_with(&needle), !c.existing));
    }
    items.truncate(max);
    items
}

/// The highlighted candidate of `count`, moved by Up/Down while `active` (the keys are taken
/// from the text field then). It starts at none and resets when the typed text changes.
pub fn highlight(ui: &mut Ui, id: egui::Id, needle: &str, count: usize, active: bool) -> Option<usize> {
    let (seen, mut picked): (String, Option<usize>) = ui.data(|d| d.get_temp(id)).unwrap_or_default();
    if seen != needle {
        picked = None;
    }
    if active && count > 0 {
        let down = ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::ArrowDown));
        let up = ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::ArrowUp));
        let last = count - 1;
        if down {
            picked = Some(picked.map_or(0, |i| (i + 1).min(last)));
        } else if up {
            picked = picked.and_then(|i| i.checked_sub(1));
        }
    }
    picked = picked.filter(|&i| i < count);
    ui.data_mut(|d| d.insert_temp(id, (needle.to_owned(), picked)));
    picked
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(label: &str, existing: bool) -> Candidate<()> {
        Candidate { label: label.into(), detail: String::new(), existing, value: () }
    }

    #[test]
    fn matches_at_the_start_and_existing_ones_come_first() {
        let items = vec![c("Gartenzaun", false), c("Vorgarten", true), c("Garten", true), c("Werkstatt", true)];
        let labels: Vec<String> = rank("gart", items.clone(), 8).into_iter().map(|c| c.label).collect();
        assert_eq!(labels, ["Garten", "Gartenzaun", "Vorgarten"]);
        assert_eq!(rank("", items, 2).len(), 2, "nothing typed: the first ones");
    }
}
