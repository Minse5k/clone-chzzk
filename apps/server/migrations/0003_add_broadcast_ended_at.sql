-- 방송 상태: 종료 시각이 없으면 라이브 중, 있으면 종료.
-- 상태를 따로 저장하지 않아 상태와 종료 시각이 어긋나지 않게 한다.
ALTER TABLE broadcasts ADD COLUMN ended_at TEXT; -- RFC 3339, UTC

-- 지금까지의 방송은 모두 방송 등록으로 생긴, 이미 종료된 방송이다.
UPDATE broadcasts SET ended_at = created_at;

CREATE INDEX broadcasts_live ON broadcasts (created_at DESC) WHERE ended_at IS NULL;
