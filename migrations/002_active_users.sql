CREATE TABLE IF NOT EXISTS active_users (
    did TEXT PRIMARY KEY,
    last_feed_request TEXT NOT NULL,
    last_follow_sync TEXT
);

CREATE INDEX IF NOT EXISTS idx_active_users_last_request ON active_users(last_feed_request DESC);
