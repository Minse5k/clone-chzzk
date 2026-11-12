-- 댓글: 로그인한 사용자가 방송에 남기는 글. 방송이 지워지면 함께 지워진다.
CREATE TABLE comments (
    id           TEXT PRIMARY KEY NOT NULL,                                  -- UUID v7
    broadcast_id TEXT NOT NULL REFERENCES broadcasts (id) ON DELETE CASCADE,
    author_id    TEXT NOT NULL REFERENCES users (id),
    body         TEXT NOT NULL,
    created_at   TEXT NOT NULL                                               -- RFC 3339, UTC
);

CREATE INDEX comments_broadcast ON comments (broadcast_id, created_at);
