CREATE TABLE broadcasts (
    id         TEXT PRIMARY KEY NOT NULL, -- UUID v7
    title      TEXT NOT NULL,
    created_at TEXT NOT NULL              -- RFC 3339, UTC
);

CREATE INDEX broadcasts_created_at ON broadcasts (created_at DESC);
