//! Ready-made tasks for the assistant, in the user's language (German unless set to English).

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{PromptMessage, Role};
use rmcp::{prompt, prompt_router};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::server::Meshgrove;

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct InboxArgs {
    /// Only the drops below this folder id.
    #[serde(default)]
    pub ordner: Option<String>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct PrintListArgs {
    /// What the list is for, e.g. "alles für den Gartenmarkt, unter 2 Stunden".
    pub wunsch: String,
    /// The printer it should fit on.
    #[serde(default)]
    pub drucker: Option<String>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct TagCleanupArgs {
    /// Only this library (alias).
    #[serde(default)]
    pub bibliothek: Option<String>,
}

impl Meshgrove {
    fn english(&self) -> bool {
        self.load_config().ok().and_then(|c| c.language).is_some_and(|l| l.starts_with("en"))
    }
}

fn user(text: String) -> Vec<PromptMessage> {
    vec![PromptMessage::new_text(Role::User, text)]
}

#[prompt_router(vis = "pub(crate)")]
impl Meshgrove {
    /// Sort the new files of the inbox: look at each drop, suggest fitting tags, take it out of the inbox once accepted.
    #[prompt(name = "eingang-einordnen")]
    async fn sort_inbox(&self, Parameters(args): Parameters<InboxArgs>) -> Vec<PromptMessage> {
        let only = args.ordner.map(|o| format!(" (folder {o})")).unwrap_or_default();
        user(if self.english() {
            format!(
                "Sort the new files in my Meshgrove inbox{only}.\n\
                 1. list_inbox, then list_tags for the vocabulary.\n\
                 2. For each drop: folder_summary (readme, common tags) and, where allowed, get_thumbnails.\n\
                 3. Suggest 2–6 tags per file with propose_tags (one call per drop, mark_inbox_done: true): \
                 tags in use first, then suggested ones; a new tag only when nothing fits. Point out printability \
                 problems and files too big for my printers.\n\
                 4. Sum up per drop what you suggested; it waits for my review in Meshgrove."
            )
        } else {
            format!(
                "Ordne die neuen Dateien in meinem Meshgrove-Eingang ein{only}.\n\
                 1. list_inbox, dann list_tags für die Wortliste.\n\
                 2. Je Gruppe: folder_summary (Readme, gemeinsame Tags) und, wo erlaubt, get_thumbnails.\n\
                 3. Schlage je Datei 2–6 Tags mit propose_tags vor (ein Aufruf je Gruppe, mark_inbox_done: true): \
                 vorhandene Tags zuerst, dann Vorschläge; neue Tags nur, wenn nichts passt. Weise auf \
                 Druckbarkeitsprobleme hin und auf Dateien, die auf keinen meiner Drucker passen.\n\
                 4. Fasse je Gruppe zusammen, was du vorgeschlagen hast; es wartet in Meshgrove auf meine Prüfung."
            )
        })
    }

    /// Put together a print list for a wish ("garden market, under 2 h") from the library.
    #[prompt(name = "druckliste-zusammenstellen")]
    async fn build_print_list(&self, Parameters(args): Parameters<PrintListArgs>) -> Vec<PromptMessage> {
        let printer = args.drucker.as_ref().map(|p| format!(" It must fit on {p}.")).unwrap_or_default();
        let drucker = args.drucker_de();
        user(if self.english() {
            format!(
                "Put together a print list from my Meshgrove library: {}.{printer}\n\
                 Use search_models (tags, fits_printer, max_size_mm, max_print_minutes for G-code, unprinted …) \
                 and get_model where needed. Then propose_list with kind \"print\" and sensible quantities, and \
                 tell me why you picked each file. It waits for my review in Meshgrove.",
                args.wunsch
            )
        } else {
            format!(
                "Stelle aus meiner Meshgrove-Bibliothek eine Druckliste zusammen: {}.{drucker}\n\
                 Nutze search_models (Tags, fits_printer, max_size_mm, max_print_minutes bei G-Code, ungedruckt …) \
                 und bei Bedarf get_model. Dann propose_list mit kind \"print\" und sinnvollen Mengen, und sag mir, \
                 warum du welche Datei gewählt hast. Die Liste wartet in Meshgrove auf meine Prüfung.",
                args.wunsch
            )
        })
    }

    /// Clean up the tags: spelling variants, synonyms, German/English pairs, rarely used tags.
    #[prompt(name = "tags-aufraeumen")]
    async fn clean_up_tags(&self, Parameters(args): Parameters<TagCleanupArgs>) -> Vec<PromptMessage> {
        let only = args.bibliothek.map(|b| format!(" ({b})")).unwrap_or_default();
        user(if self.english() {
            format!(
                "Clean up the tags of my Meshgrove library{only}. Call list_tags, find spelling variants, \
                 plurals, synonyms and German/English pairs, and suggest merges with propose_tag_merge (towards the \
                 tag in use most, or the suggested one). List rarely used tags you would drop, but do not suggest \
                 removing them without asking. Everything waits for my review in Meshgrove."
            )
        } else {
            format!(
                "Räume die Tags meiner Meshgrove-Bibliothek auf{only}. Rufe list_tags auf, finde \
                 Schreibvarianten, Mehrzahlformen, Synonyme und Deutsch/Englisch-Paare und schlage mit \
                 propose_tag_merge Zusammenlegungen vor (hin zum meistgenutzten oder zum vorgeschlagenen Tag). Nenne \
                 selten genutzte Tags, die du streichen würdest, aber schlag ihr Entfernen nicht ungefragt vor. Alles \
                 wartet in Meshgrove auf meine Prüfung."
            )
        })
    }
}

impl PrintListArgs {
    fn drucker_de(&self) -> String {
        self.drucker.as_ref().map(|p| format!(" Sie muss auf {p} passen.")).unwrap_or_default()
    }
}
