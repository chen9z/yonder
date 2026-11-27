-- Version 5: bots the owner paused. Set and cleared only by `yonder bot
-- pause`, `resume`, `reset` and `rm`; never by a run's state, a restart or
-- what `status` sees.

CREATE TABLE bot_pauses (
    bot             TEXT PRIMARY KEY,
    paused_at       TEXT NOT NULL
);
