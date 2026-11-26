-- Version 2: what yonderd needs. Reminders gain the check's bookkeeping
-- and the `expired` state (a new CHECK, so the table is copied over);
-- deliveries are what goes to the lead agent, each trigger written once
-- before it is sent; meta holds the daemon's last tick.

CREATE TABLE reminders_v2 (
    id              TEXT PRIMARY KEY,          -- R-0001
    content         TEXT NOT NULL,
    state           TEXT NOT NULL CHECK (state IN ('active', 'fired', 'cancelled', 'expired')),
    next_fire_at    TEXT NOT NULL,
    every_spec      TEXT,                      -- NULL: once; else 5m | 2h | 1d | daily@09:00 | weekly:1@09:00
    snooze_until    TEXT,
    last_fired_at   TEXT,
    check_cmd       TEXT,
    check_hash      TEXT,                      -- SHA-256 hex of the trimmed stdout, never the output
    check_once      INTEGER NOT NULL DEFAULT 0 CHECK (check_once IN (0, 1)),
    check_until     TEXT,                      -- NULL: forever
    last_checked_at TEXT,
    check_failures  INTEGER NOT NULL DEFAULT 0,
    check_error     TEXT,                      -- exit:N | signal:N | timeout | output_too_large | spawn_failed: ...
    check_alerted   INTEGER NOT NULL DEFAULT 0 CHECK (check_alerted IN (0, 1)),
    task_id         TEXT REFERENCES tasks(id),
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL,

    CHECK (check_cmd IS NULL OR (every_spec IS NOT NULL AND check_hash IS NOT NULL))
);

-- Version 1 checks had no limit: they keep none.
INSERT INTO reminders_v2 (id, content, state, next_fire_at, every_spec, snooze_until, check_cmd, check_hash, task_id, created_at, updated_at)
    SELECT id, content, state, next_fire_at, every_spec, snooze_until, check_cmd, check_hash, task_id, created_at, updated_at FROM reminders;
DROP TABLE reminders;
ALTER TABLE reminders_v2 RENAME TO reminders;
CREATE INDEX idx_rem_active ON reminders(state, next_fire_at);

CREATE TABLE deliveries (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    reminder_id     TEXT NOT NULL REFERENCES reminders(id),
    trigger_kind    TEXT NOT NULL CHECK (trigger_kind IN ('time', 'check_changed', 'check_failing', 'check_expired')),
    due_at          TEXT NOT NULL,             -- the time that went off
    body            TEXT NOT NULL,             -- the envelope, as sent
    state           TEXT NOT NULL DEFAULT 'pending' CHECK (state IN ('pending', 'sending', 'sent', 'lost')),
    attempts        INTEGER NOT NULL DEFAULT 0,
    last_error      TEXT,
    sent_to         TEXT,
    created_at      TEXT NOT NULL,
    sent_at         TEXT,

    UNIQUE (reminder_id, trigger_kind, due_at)
);
CREATE INDEX idx_deliveries_state ON deliveries(state, id);

CREATE TABLE meta (
    key             TEXT PRIMARY KEY,
    value           TEXT NOT NULL
);
