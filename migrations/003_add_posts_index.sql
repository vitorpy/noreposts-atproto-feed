-- Add compound index for better query performance on feed generation
-- This helps with queries that filter by author_did and order by created_at
CREATE INDEX IF NOT EXISTS idx_posts_author_created ON posts(author_did, created_at DESC);
