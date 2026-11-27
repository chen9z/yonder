-- Version 3: what `yonder bot sync` last pushed to a bot's AGENTS.md, by
-- where it went (`<machine>:<cwd>`), so a file changed there since is
-- told from one Yonder wrote. Only the hash is kept, never the text.

CREATE TABLE bot_pushes (
    bot             TEXT NOT NULL,
    target          TEXT NOT NULL,             -- local:/tmp/example/ops | gpu-box:~/bots/trainer
    agents_sha256   TEXT NOT NULL,             -- SHA-256 hex of the AGENTS.md written
    pushed_at       TEXT NOT NULL,

    PRIMARY KEY (bot, target)
);
