//! The person's choices on this phone, kept in the app's storage.

use crate::i18n::{self, Language};
use nsh_core::prelude::*;
use nsh_core::storage::Storage;
use serde::{Deserialize, Serialize};

const PREFS: &str = "prefs";

#[derive(Clone, Copy)]
pub struct Prefs {
    /// The terminal's font size, in points.
    pub font: Signal<f32>,
    /// Terminals shown are taken at the phone's grid (else they keep the
    /// desktop's, and typing does too).
    pub fit: Signal<bool>,
    /// The app's language (the phone's, unless chosen).
    pub language: Signal<Language>,
    /// The key bar's keys, in order ([`crate::keybar::KEYS`] ids).
    pub keys: Signal<Vec<String>>,
    /// Machines folded on the home page, by computer and machine.
    pub folded: Signal<Vec<String>>,
    /// Each space's color ([`crate::ui::MARKS`]), by computer and space.
    pub colors: Signal<std::collections::BTreeMap<String, usize>>,
    /// Agents by priority, else grouped by space.
    pub grouped: Signal<bool>,
    /// Each computer's main agent, by computer.
    pub leads: Signal<std::collections::BTreeMap<String, crate::lead::Lead>>,
    aliases: Signal<std::collections::BTreeMap<String, String>>,
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct Kept {
    font: f32,
    fit: bool,
    language: Language,
    keys: Vec<String>,
    folded: Vec<String>,
    colors: std::collections::BTreeMap<String, usize>,
    grouped: bool,
    leads: std::collections::BTreeMap<String, crate::lead::Lead>,
    aliases: std::collections::BTreeMap<String, String>,
}

impl Default for Kept {
    fn default() -> Self {
        Kept { font: 12.0, fit: true, language: Language::System, keys: crate::keybar::at_first(), folded: Vec::new(), colors: Default::default(), grouped: false, leads: Default::default(), aliases: Default::default() }
    }
}

impl Prefs {
    pub fn machine_name(self, computer: &str, machine: &str) -> String {
        let key = format!("{computer}\0{machine}");
        self.aliases.with(|a| a.get(&key).cloned()).unwrap_or_else(|| crate::link::host_label(machine).unwrap_or(t!("Machine")).into())
    }

    pub async fn rename_machine(self, live: &crate::live::Live, machine: &str) {
        let id = live.computer.id();
        let prompt = crate::ui::name_prompt(self.machine_name(&id, machine));
        if let Some(name) = live.words(prompt).await && !name.trim().is_empty() {
            self.aliases.update(|a| { a.insert(format!("{id}\0{machine}"), name.trim().into()); });
        }
    }

    /// The main agent chosen on `computer`.
    pub fn lead(self, computer: &str) -> Option<crate::lead::Lead> {
        self.leads.with(|l| l.get(computer).cloned())
    }

    /// Whether `computer` has a main agent, as kept (before the prefs are
    /// read: the app opens on it).
    pub fn kept_lead(computer: &str) -> bool {
        Storage::get_item(PREFS).and_then(|t| serde_json::from_str::<Kept>(&t).ok()).is_some_and(|k| k.leads.contains_key(computer))
    }

    /// Loads them, and keeps them as they change.
    pub fn provide() -> Prefs {
        let kept: Kept = Storage::get_item(PREFS).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        let language = i18n::chosen();
        language.set(kept.language);
        let prefs = Prefs { font: signal(kept.font), fit: signal(kept.fit), language, keys: signal(kept.keys), folded: signal(kept.folded), colors: signal(kept.colors), grouped: signal(kept.grouped), leads: signal(kept.leads), aliases: signal(kept.aliases) };
        effect(move || {
            let kept = Kept { font: prefs.font.get(), fit: prefs.fit.get(), language: prefs.language.get(), keys: prefs.keys.get(), folded: prefs.folded.get(), colors: prefs.colors.get(), grouped: prefs.grouped.get(), leads: prefs.leads.get(), aliases: prefs.aliases.get() };
            let _ = Storage::set_item(PREFS, serde_json::to_string(&kept).unwrap_or_default());
        });
        provide_context(prefs);
        prefs
    }
}
