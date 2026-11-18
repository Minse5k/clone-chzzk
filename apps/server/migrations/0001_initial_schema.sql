-- 과제 5: SQLite에서 PostgreSQL로 옮기며 과제 4까지의 최종 스키마를 다시 만든다.
-- 여러 서버 인스턴스가 이 DB 하나를 함께 쓴다.

-- 사용자: 아이디와 비밀번호(argon2id 해시)로 로그인한다.
CREATE TABLE users (
    id            UUID PRIMARY KEY,               -- UUID v7
    username      TEXT NOT NULL UNIQUE,           -- 소문자로 바꿔 저장해 대소문자 구분 없이 유일하다
    password_hash TEXT NOT NULL,                  -- PHC 문자열(argon2id)
    created_at    TIMESTAMPTZ NOT NULL
);

-- 로그아웃한 토큰. 만료 시각이 지나면 토큰 자체가 무효이므로 지워도 된다.
CREATE TABLE revoked_tokens (
    jti        TEXT PRIMARY KEY,
    expires_at TIMESTAMPTZ NOT NULL
);

-- 방송. 종료 시각이 없으면 라이브 중, 있으면 종료.
-- 상태를 따로 저장하지 않아 상태와 종료 시각이 어긋나지 않게 한다.
CREATE TABLE broadcasts (
    id         UUID PRIMARY KEY,                  -- UUID v7
    title      TEXT NOT NULL,
    owner_id   UUID NOT NULL REFERENCES users (id),
    created_at TIMESTAMPTZ NOT NULL,
    ended_at   TIMESTAMPTZ
);

CREATE INDEX broadcasts_created_at ON broadcasts (created_at DESC);
CREATE INDEX broadcasts_live ON broadcasts (created_at DESC) WHERE ended_at IS NULL;
CREATE INDEX broadcasts_owner ON broadcasts (owner_id);

-- 녹화본: 종료된 방송에 남는 영상. 방송 하나에 0~1개.
CREATE TABLE recordings (
    broadcast_id    UUID PRIMARY KEY REFERENCES broadcasts (id),
    storage_backend TEXT NOT NULL,                -- 'local' | 'oci'
    object_key      TEXT NOT NULL,                -- 저장소 안에서의 위치
    content_type    TEXT NOT NULL,                -- 'video/mp4' | 'video/webm'
    size_bytes      BIGINT NOT NULL CHECK (size_bytes >= 0)
);

-- 지워야 할 녹화본 객체. 서버가 시작할 때 저장소에서 지우고 이 목록에서 뺀다.
CREATE TABLE pending_object_deletions (
    storage_backend TEXT NOT NULL,
    object_key      TEXT NOT NULL,
    PRIMARY KEY (storage_backend, object_key)
);

-- 댓글: 로그인한 사용자가 방송에 남기는 글. 방송이 지워지면 함께 지워진다.
CREATE TABLE comments (
    id           UUID PRIMARY KEY,                -- UUID v7
    broadcast_id UUID NOT NULL REFERENCES broadcasts (id) ON DELETE CASCADE,
    author_id    UUID NOT NULL REFERENCES users (id),
    body         TEXT NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL
);

CREATE INDEX comments_broadcast ON comments (broadcast_id, created_at);

-- 후원: 주문(결제 대기)과 확정된 후원을 한 표에 둔다. 후원 내역·합계는 confirmed만 센다.
--
-- 중복 확정 방지:
--   * 확정은 status = 'pending'인 행만 바꾸는 조건부 갱신으로 한 번만 일어난다.
--   * payment_key는 유일해서 같은 결제가 다른 주문에 확정될 수 없다.
CREATE TABLE donations (
    order_id       TEXT PRIMARY KEY,                       -- 토스페이먼츠 주문번호
    broadcast_id   UUID NOT NULL REFERENCES broadcasts (id),
    recipient_id   UUID NOT NULL REFERENCES users (id),    -- 주문 시점의 방송 소유자
    donor_id       UUID NOT NULL REFERENCES users (id),
    amount         BIGINT NOT NULL CHECK (amount > 0),     -- 원
    message        TEXT NOT NULL,
    status         TEXT NOT NULL CHECK (status IN ('pending', 'confirmed', 'failed', 'canceled')),
    payment_key    TEXT UNIQUE,                            -- 확정한 결제의 키
    failure_reason TEXT,
    created_at     TIMESTAMPTZ NOT NULL,
    confirmed_at   TIMESTAMPTZ
);

CREATE INDEX donations_broadcast ON donations (broadcast_id, status, confirmed_at DESC);
