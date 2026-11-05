-- 녹화본: 종료된 방송에 남는 영상. 방송 하나에 0~1개.
CREATE TABLE recordings (
    broadcast_id    TEXT PRIMARY KEY NOT NULL REFERENCES broadcasts (id),
    storage_backend TEXT NOT NULL,    -- 'local' | 'oci'
    object_key      TEXT NOT NULL,    -- 저장소 안에서의 위치
    content_type    TEXT NOT NULL,    -- 'video/mp4' | 'video/webm'
    size_bytes      INTEGER NOT NULL
);
