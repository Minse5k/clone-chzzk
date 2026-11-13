-- 방송 소유자: 녹화방송을 등록하거나 송출을 시작한 사용자. 소유자만 방송을 관리할 수 있다.

-- 지워야 할 녹화본 객체. 서버가 시작할 때 저장소에서 지우고 이 목록에서 뺀다.
-- 방송을 삭제할 때 저장소 객체를 바로 지우지 못한 경우에도 쓴다.
CREATE TABLE pending_object_deletions (
    storage_backend TEXT NOT NULL,    -- 'local' | 'oci'
    object_key      TEXT NOT NULL,
    PRIMARY KEY (storage_backend, object_key)
);

-- 로그인 도입 전(과제 1·2)의 방송은 소유자가 없어 아무도 관리할 수 없으므로 지운다.
-- 녹화본 객체는 DB 행을 지우기 전에 삭제 대기열로 옮겨 저장 공간을 돌려받는다.
INSERT OR IGNORE INTO pending_object_deletions (storage_backend, object_key)
    SELECT storage_backend, object_key FROM recordings;
DELETE FROM recordings;
DELETE FROM broadcasts;

-- 소유자를 필수 열로 두기 위해 표를 다시 만든다(SQLite는 기본값 없는 NOT NULL 열을 추가할 수 없다).
-- 위에서 비웠으므로 옮길 행이 없다.
DROP INDEX broadcasts_created_at;
DROP INDEX broadcasts_live;
CREATE TABLE broadcasts_new (
    id         TEXT PRIMARY KEY NOT NULL,              -- UUID v7
    title      TEXT NOT NULL,
    owner_id   TEXT NOT NULL REFERENCES users (id),
    created_at TEXT NOT NULL,                          -- RFC 3339, UTC
    ended_at   TEXT                                    -- RFC 3339, UTC. 없으면 라이브 중
);
DROP TABLE broadcasts;
ALTER TABLE broadcasts_new RENAME TO broadcasts;
CREATE INDEX broadcasts_created_at ON broadcasts (created_at DESC);
CREATE INDEX broadcasts_live ON broadcasts (created_at DESC) WHERE ended_at IS NULL;
CREATE INDEX broadcasts_owner ON broadcasts (owner_id);
