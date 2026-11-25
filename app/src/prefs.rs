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
    /// Light or dark (the phone's, unless chosen).
    pub look: Signal<crate::theme::Look>,
    /// Which notes come over the page.
    pub notify: Signal<crate::notes::Notify>,
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
    look: crate::theme::Look,
    notify: crate::notes::Notify,
    keys: Vec<String>,
    folded: Vec<String>,
    colors: std::collections::BTreeMap<String, usize>,
    grouped: bool,
    leads: std::collections::BTreeMap<String, crate::lead::Lead>,
    aliases: std::collections::BTreeMap<String, String>,
}

impl Default for Kept {
    fn default() -> Self {
        Kept { font: 12.0, fit: true, language: Language::System, look: Default::default(), notify: Default::default(), keys: crate::keybar::at_first(), folded: Vec::new(), colors: Default::default(), grouped: false, leads: Default::default(), aliases: Default::default() }
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

    /// A computer saved again under another id (`to`, the same computer):
    /// its main agent, machines' names, spaces' colors and folds go with
    /// it; forgotten or another one (None), they go.
    pub fn carry(self, from: &str, to: Option<&str>) {
        let mut kept = Kept { folded: self.folded.get_untracked(), colors: self.colors.get_untracked(), leads: self.leads.get_untracked(), aliases: self.aliases.get_untracked(), ..Kept::default() };
        rekey(&mut kept, from, to);
        // At once: what reads one of them sees the others moved too.
        batch(|| {
            self.folded.set(kept.folded);
            self.colors.set(kept.colors);
            self.leads.set(kept.leads);
            self.aliases.set(kept.aliases);
        });
    }

    /// `was` saved again as `now`: its choices go with it if it is the
    /// same computer (another address), else they go.
    pub fn saved_again(self, was: &crate::link::Computer, now: &crate::link::Computer) {
        let (from, to) = (was.id(), now.id());
        if from != to {
            self.carry(&from, crate::link::same(was, now).then_some(to.as_str()));
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
        let prefs = Prefs { font: signal(kept.font), fit: signal(kept.fit), language, look: signal(kept.look), notify: signal(kept.notify), keys: signal(kept.keys), folded: signal(kept.folded), colors: signal(kept.colors), grouped: signal(kept.grouped), leads: signal(kept.leads), aliases: signal(kept.aliases) };
        effect(move || {
            let kept = Kept { font: prefs.font.get(), fit: prefs.fit.get(), language: prefs.language.get(), look: prefs.look.get(), notify: prefs.notify.get(), keys: prefs.keys.get(), folded: prefs.folded.get(), colors: prefs.colors.get(), grouped: prefs.grouped.get(), leads: prefs.leads.get(), aliases: prefs.aliases.get() };
            let _ = Storage::set_item(PREFS, serde_json::to_string(&kept).unwrap_or_default());
        });
        effect(move || prefs.look.get().apply());
        provide_context(prefs);
        prefs
    }
}

/// `from`'s entries as `to`'s (not over those it has), or gone. Keys:
/// `leads` by computer; `aliases`, `colors` by computer, NUL, machine or
/// space; `folded` a JSON `[computer, machine]`.
fn rekey(kept: &mut Kept, from: &str, to: Option<&str>) {
    if to == Some(from) {
        return;
    }
    // A key of `from`'s: what it becomes (None: gone); not one, None.
    fn moved<V>(map: &mut std::collections::BTreeMap<String, V>, key: impl Fn(&str) -> Option<Option<String>>) {
        let mine: Vec<String> = map.keys().filter(|k| key(k).is_some()).cloned().collect();
        for k in mine {
            if let (Some(v), Some(Some(new))) = (map.remove(&k), key(&k)) {
                map.entry(new).or_insert(v);
            }
        }
    }
    let under = |k: &str| k.strip_prefix(from)?.strip_prefix('\0').map(|rest| to.map(|to| format!("{to}\0{rest}")));
    moved(&mut kept.aliases, under);
    moved(&mut kept.colors, under);
    moved(&mut kept.leads, |k| (k == from).then(|| to.map(str::to_owned)));
    let folds = std::mem::take(&mut kept.folded);
    for f in folds {
        let new = match serde_json::from_str::<(String, Option<String>)>(&f) {
            Ok((computer, machine)) if computer == from => to.map(|to| serde_json::to_string(&(to, machine)).unwrap_or_default()),
            _ => Some(f),
        };
        if let Some(new) = new.filter(|n| !kept.folded.contains(n)) {
            kept.folded.push(new);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_computer_s_choices_go_with_it_or_go() {
        let lead = crate::lead::Lead { address: "t1".into(), space: "s1".into(), space_name: "nova".into(), app: Some("pi".into()) };
        let mut kept = Kept::default();
        kept.leads.insert("a".into(), lead.clone());
        kept.leads.insert("b".into(), lead.clone());
        kept.aliases.insert("a\0vps".into(), "Cloud".into());
        kept.aliases.insert("ab\0vps".into(), "Other".into());
        kept.colors.insert("a\0s1".into(), 3);
        kept.folded = vec![serde_json::to_string(&("a", Some("vps"))).unwrap(), serde_json::to_string(&("b", None::<String>)).unwrap()];
        // Saved again as `c`: all of `a`'s go with it, no other's.
        let mut moved = Kept { leads: kept.leads.clone(), aliases: kept.aliases.clone(), colors: kept.colors.clone(), folded: kept.folded.clone(), ..Kept::default() };
        rekey(&mut moved, "a", Some("c"));
        assert_eq!(moved.leads.keys().collect::<Vec<_>>(), ["b", "c"]);
        assert_eq!(moved.aliases.get("c\0vps").map(String::as_str), Some("Cloud"));
        assert_eq!(moved.aliases.get("ab\0vps").map(String::as_str), Some("Other"));
        assert_eq!(moved.colors.get("c\0s1"), Some(&3));
        assert_eq!(moved.folded, [serde_json::to_string(&("c", Some("vps"))).unwrap(), serde_json::to_string(&("b", None::<String>)).unwrap()]);
        // Onto one that has its own: its own stay.
        let mut onto = Kept { leads: kept.leads.clone(), ..Kept::default() };
        onto.leads.get_mut("b").unwrap().address = "t9".into();
        rekey(&mut onto, "a", Some("b"));
        assert_eq!((onto.leads.len(), onto.leads["b"].address.as_str()), (1, "t9"));
        // Forgotten: `a`'s go, the rest stay.
        rekey(&mut kept, "a", None);
        assert_eq!(kept.leads.keys().collect::<Vec<_>>(), ["b"]);
        assert_eq!(kept.aliases.keys().collect::<Vec<_>>(), ["ab\0vps"]);
        assert!(kept.colors.is_empty());
        assert_eq!(kept.folded.len(), 1);
    }
}
