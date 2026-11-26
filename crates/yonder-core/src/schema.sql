-- The ledger, version 1. Times are UTC, `YYYY-MM-DDTHH:MM:SSZ`, so they
-- compare as text. Later versions add to it in store.rs's MIGRATIONS.

CREATE TABLE IF NOT EXISTS tasks (
    id              TEXT PRIMARY KEY,          -- T-0001
    title           TEXT NOT NULL,
    state           TEXT NOT NULL CHECK (state IN ('open', 'done', 'cancelled')),
    source          TEXT,                      -- such as hc:<addr>
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS reminders (
    id              TEXT PRIMARY KEY,          -- R-0001
    content         TEXT NOT NULL,
    state           TEXT NOT NULL CHECK (state IN ('active', 'fired', 'cancelled')),
    next_fire_at    TEXT NOT NULL,
    every_spec      TEXT,                      -- NULL: once; else 5m | 2h | 1d | daily@09:00 | weekly:1@09:00
    snooze_until    TEXT,
    check_cmd       TEXT,
    check_hash      TEXT,                      -- SHA-256 hex of the trimmed stdout, never the output
    task_id         TEXT REFERENCES tasks(id),
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL,

    CHECK (check_cmd IS NULL OR (every_spec IS NOT NULL AND check_hash IS NOT NULL))
);

CREATE INDEX IF NOT EXISTS idx_tasks_state ON tasks(state);
CREATE INDEX IF NOT EXISTS idx_rem_active  ON reminders(state, next_fire_at);
