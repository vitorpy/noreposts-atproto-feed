CREATE TABLE IF NOT EXISTS posts (
    uri TEXT PRIMARY KEY,
    cid TEXT NOT NULL,
    author_did TEXT NOT NULL,
    text TEXT NOT NULL,
    created_at TEXT NOT NULL,
    indexed_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_posts_author_indexed ON posts(author_did, indexed_at DESC);
CREATE INDEX IF NOT EXISTS idx_posts_indexed ON posts(indexed_at DESC);

CREATE TABLE IF NOT EXISTS follows (
    uri TEXT PRIMARY KEY,
    follower_did TEXT NOT NULL,
    target_did TEXT NOT NULL,
    created_at TEXT NOT NULL,
    indexed_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_follows_follower ON follows(follower_did);
CREATE INDEX IF NOT EXISTS idx_follows_target ON follows(target_did);
CREATE UNIQUE INDEX IF NOT EXISTS idx_follows_unique ON follows(follower_did, target_did);
