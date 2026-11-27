-- Version 4: each bot's current run (one row a bot, replaced by the next
-- start once it is final) and the session it resumes, kept apart so a run
-- that goes does not take it along. Runs only move by
-- `UPDATE ... WHERE state = <from>`: see `Store::move_run`.

CREATE TABLE bot_runs (
    bot             TEXT PRIMARY KEY,
    address         TEXT NOT NULL,             -- t7 | gpu-box/t7
    state           TEXT NOT NULL CHECK (state IN ('starting', 'ready', 'failed', 'gone')),
    session         TEXT,                      -- the session it was started to resume
    started_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL,
    reason          TEXT                       -- start_timeout | needs_attention: ... | exited: ...
);

CREATE TABLE bot_sessions (
    bot             TEXT PRIMARY KEY,
    session         TEXT NOT NULL,
    updated_at      TEXT NOT NULL
);
