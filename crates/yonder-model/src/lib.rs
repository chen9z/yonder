//! The ledger's records as every Yonder program sees them: tasks and
//! reminders, and the words their states are stored and shown as. Times are
//! UTC, `YYYY-MM-DDTHH:MM:SSZ`, so they sort as text.

use serde::{Deserialize, Serialize};

pub mod envelope;

/// An enum stored as a snake_case word: `as_str` gives it, `parse` takes it back.
macro_rules! words {
    ($(#[$m:meta])* $name:ident { $($(#[$vm:meta])* $v:ident = $s:literal),+ $(,)? }) => {
        $(#[$m])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name { $($(#[$vm])* $v),+ }

        impl $name {
            pub const ALL: &[$name] = &[$($name::$v),+];

            pub fn as_str(self) -> &'static str {
                match self { $($name::$v => $s),+ }
            }

            pub fn parse(s: &str) -> Option<$name> {
                match s { $($s => Some($name::$v),)+ _ => None }
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.pad(self.as_str())
            }
        }
    };
}

words! {
    TaskState { Open = "open", Done = "done", Cancelled = "cancelled" }
}

words! {
    /// `fired`: a one-off reminder that went off, or a `check_once` one
    /// whose output changed. `expired`: a check past its `check_until`.
    ReminderState { Active = "active", Fired = "fired", Cancelled = "cancelled", Expired = "expired" }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Task {
    /// `T-0001`.
    pub id: String,
    pub title: String,
    pub state: TaskState,
    /// Where it came from, such as `hc:nova`.
    pub source: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Reminder {
    /// `R-0001`.
    pub id: String,
    pub content: String,
    pub state: ReminderState,
    pub next_fire_at: String,
    /// None: once. Else `5m`, `2h`, `1d`, `daily@09:00` or `weekly:1@09:00`.
    pub every: Option<String>,
    /// Not before this, even if `next_fire_at` has passed.
    pub snooze_until: Option<String>,
    /// A shell command; the reminder is about its output changing.
    pub check_cmd: Option<String>,
    /// SHA-256 hex of the command's trimmed output, never the output.
    pub check_hash: Option<String>,
    /// Ends after the first change.
    pub check_once: bool,
    /// The check stops after this; None: never.
    pub check_until: Option<String>,
    pub last_checked_at: Option<String>,
    /// Failures in a row; the third is told once.
    pub check_failures: u32,
    /// Why the last check failed: `exit:N`, `timeout` and so on.
    pub check_error: Option<String>,
    pub last_fired_at: Option<String>,
    pub task_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl Reminder {
    /// When it goes off: `next_fire_at`, or the end of its snooze if later.
    pub fn due_at(&self) -> &str {
        match &self.snooze_until {
            Some(s) if s.as_str() > self.next_fire_at.as_str() => s,
            _ => &self.next_fire_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_round_trip() {
        for s in TaskState::ALL {
            assert_eq!(TaskState::parse(s.as_str()), Some(*s));
            let json = serde_json::to_string(s).unwrap();
            assert_eq!(json, format!("\"{s}\""));
        }
        for s in ReminderState::ALL {
            assert_eq!(ReminderState::parse(s.as_str()), Some(*s));
        }
        assert_eq!(TaskState::parse("closed"), None);
    }

    #[test]
    fn snooze_moves_due_at() {
        let mut r = Reminder {
            id: "R-0001".into(),
            content: "check gpu-box".into(),
            state: ReminderState::Active,
            next_fire_at: "2026-01-02T09:00:00Z".into(),
            every: None,
            snooze_until: None,
            check_cmd: None,
            check_hash: None,
            check_once: false,
            check_until: None,
            last_checked_at: None,
            check_failures: 0,
            check_error: None,
            last_fired_at: None,
            task_id: None,
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
        };
        assert_eq!(r.due_at(), "2026-01-02T09:00:00Z");
        r.snooze_until = Some("2026-01-02T10:00:00Z".into());
        assert_eq!(r.due_at(), "2026-01-02T10:00:00Z");
        r.snooze_until = Some("2026-01-02T08:00:00Z".into());
        assert_eq!(r.due_at(), "2026-01-02T09:00:00Z");
    }
}
