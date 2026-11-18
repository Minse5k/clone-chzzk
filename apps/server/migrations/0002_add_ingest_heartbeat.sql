-- 과제 5: 라이브 방송을 송출받는 인스턴스와 그 인스턴스가 마지막으로 살아 있음을 알린 시각.
-- 송출 중인 인스턴스가 주기적으로 하트비트를 갱신하고, 하트비트가 오래 끊긴 라이브 방송은
-- 다른 인스턴스가 종료로 정리한다(송출받던 인스턴스가 죽은 경우). 송출 없이 생긴 방송은 비어 있다.
ALTER TABLE broadcasts ADD COLUMN ingest_instance TEXT;
ALTER TABLE broadcasts ADD COLUMN ingest_heartbeat_at TIMESTAMPTZ;

CREATE INDEX broadcasts_ingest_heartbeat ON broadcasts (ingest_heartbeat_at) WHERE ended_at IS NULL;
