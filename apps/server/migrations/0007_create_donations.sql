-- 후원: 주문(결제 대기)과 확정된 후원을 한 표에 둔다. 후원 내역·합계는 confirmed만 센다.
--
-- 중복 확정 방지:
--   * 확정은 status = 'pending'인 행만 바꾸는 조건부 갱신으로 한 번만 일어난다.
--   * payment_key는 유일해서 같은 결제가 다른 주문에 확정될 수 없다.
CREATE TABLE donations (
    order_id       TEXT PRIMARY KEY NOT NULL,              -- 토스페이먼츠 주문번호
    broadcast_id   TEXT NOT NULL REFERENCES broadcasts (id),
    recipient_id   TEXT NOT NULL REFERENCES users (id),    -- 주문 시점의 방송 소유자
    donor_id       TEXT NOT NULL REFERENCES users (id),
    amount         INTEGER NOT NULL CHECK (amount > 0),    -- 원
    message        TEXT NOT NULL,
    status         TEXT NOT NULL CHECK (status IN ('pending', 'confirmed', 'failed', 'canceled')),
    payment_key    TEXT UNIQUE,                            -- 확정한 결제의 키
    failure_reason TEXT,
    created_at     TEXT NOT NULL,                          -- RFC 3339, UTC
    confirmed_at   TEXT                                    -- RFC 3339, UTC
);

CREATE INDEX donations_broadcast ON donations (broadcast_id, status, confirmed_at DESC);
