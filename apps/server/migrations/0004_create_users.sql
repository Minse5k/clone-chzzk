-- 사용자: 아이디와 비밀번호(argon2id 해시)로 로그인한다.
CREATE TABLE users (
    id            TEXT PRIMARY KEY NOT NULL, -- UUID v7
    username      TEXT NOT NULL UNIQUE,      -- 소문자로 바꿔 저장해 대소문자 구분 없이 유일하다
    password_hash TEXT NOT NULL,             -- PHC 문자열(argon2id)
    created_at    TEXT NOT NULL              -- RFC 3339, UTC
);

-- 로그아웃한 토큰. 만료 시각이 지나면 토큰 자체가 무효이므로 지워도 된다.
CREATE TABLE revoked_tokens (
    jti        TEXT PRIMARY KEY NOT NULL,
    expires_at TEXT NOT NULL                 -- RFC 3339, UTC
);
