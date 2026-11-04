# clone-chzzk

로그인 없이 방송을 등록하고 시청하는 서비스. 최종 목표는 데스크톱에서 송출한 라이브 방송을 웹에서 시청하는 것이며, 과제 단위로 하나씩 확장한다.

- 도메인 용어: [GLOSSARY.md](GLOSSARY.md)
- 설계 결정: [docs/adr/](docs/adr/)

## 구성

Cargo workspace 모노레포.

| 경로 | 설명 |
|---|---|
| `apps/server` | axum API 서버. SQLite(sqlx)에 방송 정보를 저장한다. |
| `apps/web` | Dioxus 0.7 웹 앱. 방송 목록·상세·등록 화면. |
| `crates/shared` | 서버와 웹이 함께 쓰는 API 타입. |

## 준비물

- Rust (stable) 와 `wasm32-unknown-unknown` 타깃: `rustup target add wasm32-unknown-unknown`
- Dioxus CLI 0.7: `cargo install dioxus-cli --version ^0.7 --locked`

## 로컬 실행

터미널 두 개를 쓴다. 기본값만으로 실행되며 비밀 값은 필요 없다.

```bash
# 터미널 1: API 서버 (http://localhost:3000)
cargo run -p server
```

```bash
# 터미널 2: 웹 (http://localhost:8080)
dx serve -p web
```

브라우저에서 http://localhost:8080 을 연다. 데이터는 기본적으로 `data/` 아래(SQLite 파일, 녹화본)에 저장된다.

## 환경변수

예시는 [.env.example](.env.example)에 있다. 서버는 `.env` 파일을 자동으로 읽지 않으므로 셸에서 지정한다.

| 이름 | 기본값 | 설명 |
|---|---|---|
| `SERVER_ADDR` | `0.0.0.0:3000` | 서버 바인딩 주소 |
| `DATABASE_PATH` | `data/chzzk.db` | SQLite 파일 경로 |
| `STORAGE_DIR` | `data/recordings` | local 저장소 디렉터리 |
| `CORS_ALLOWED_ORIGINS` | `http://localhost:8080` | 허용할 웹 출처(쉼표 구분, `*`는 전체) |
| `API_BASE_URL` | `http://localhost:3000` | (웹 빌드 시) 웹이 호출할 서버 주소 |

## API

| 메서드·경로 | 설명 |
|---|---|
| `GET /api/broadcasts` | 방송 목록(최신 등록순) |
| `POST /api/broadcasts` | 방송 등록. `multipart/form-data`로 `title`(텍스트)과 `recording`(mp4/webm 파일)을 보낸다. 성공 시 201 |
| `GET /api/broadcasts/{id}` | 방송 상세. 없으면 404 |
| `GET /api/broadcasts/{id}/recording` | 녹화본 재생. `Range` 요청을 지원한다(206) |

오류 응답 본문은 `{ "message": "..." }` 형태의 한국어 메시지다.

## 테스트

```bash
cargo test -p server
```

서버 HTTP API 통합 테스트. 테스트마다 임시 SQLite 파일과 임시 저장소를 쓴다.
