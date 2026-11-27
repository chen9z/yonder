-- Version 6: handoffs, what the owner (or a conversation elsewhere) hands
-- a bot to merge into its MEMORY.md. Delivered at most once, as
-- deliveries are: waiting -> sending -> delivered | lost, and sending ->
-- waiting when hc refused before typing. Never deleted.

CREATE TABLE handoffs (
    id              TEXT PRIMARY KEY,          -- H-0001
    bot             TEXT NOT NULL,
    body            TEXT NOT NULL,
    via             TEXT NOT NULL CHECK (via IN ('command', 'paste')),
    state           TEXT NOT NULL CHECK (state IN ('waiting', 'sending', 'delivered', 'lost')),
    created_at      TEXT NOT NULL,
    delivered_at    TEXT,
    address         TEXT,                      -- the terminal it was sent to
    sender          INTEGER,                   -- the pid sending it, while sending
    reason          TEXT                       -- why it waits or was lost
);
CREATE INDEX idx_handoffs_waiting ON handoffs(state, bot);
