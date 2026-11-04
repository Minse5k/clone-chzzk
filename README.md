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
| `STORAGE_BACKEND` | `local` | 녹화본 저장소. `local` 또는 `oci` |
| `STORAGE_DIR` | `data/recordings` | local 저장소 디렉터리 |
| `OCI_S3_ENDPOINT` | — | (oci) S3 호환 엔드포인트 `https://<namespace>.compat.objectstorage.<region>.oraclecloud.com` |
| `OCI_REGION` | — | (oci) 리전. 예: `ap-osaka-1` |
| `OCI_BUCKET` | — | (oci) 비공개 버킷 이름 |
| `OCI_ACCESS_KEY_ID` | — | (oci) Customer Secret Key의 Access Key |
| `OCI_SECRET_ACCESS_KEY` | — | (oci) Customer Secret Key의 Secret Key |
| `MAX_UPLOAD_BYTES` | `2147483648` (2GiB) | 녹화본 하나의 최대 크기. 넘으면 413 |
| `STORAGE_QUOTA_BYTES` | `16106127360` (15GiB) | 저장된 녹화본 크기 합의 한도. 넘으면 507 |
| `CORS_ALLOWED_ORIGINS` | `http://localhost:8080` | 허용할 웹 출처(쉼표 구분, `*`는 전체) |
| `WEB_DIST_DIR` | (없음) | 빌드된 웹 정적 파일 디렉터리. 지정하면 서버가 웹 화면도 서빙한다(배포용) |
| `API_BASE_URL` | `http://localhost:3000` | (웹 빌드 시) 웹이 호출할 서버 주소 |

## 녹화본 저장소

기본값 `STORAGE_BACKEND=local`은 서버 디렉터리(`STORAGE_DIR`)에 녹화본을 저장하며 비밀 값이 필요 없다.

`STORAGE_BACKEND=oci`로 두면 OCI Object Storage의 **비공개 버킷**에 S3 호환 API로 저장한다. 업로드는 8MB 단위 멀티파트로 스트리밍되고, 재생은 서버가 Range 요청을 버킷으로 그대로 넘긴다(버킷을 공개할 필요 없음). 설계 근거는 [ADR 0002](docs/adr/0002-recordings-proxied-through-server.md).

1. OCI 콘솔에서 버킷을 만든다(공개 범위: Private).
2. 프로필 > 사용자 설정 > **Customer Secret Keys**에서 키를 발급한다(Secret Key는 발급 직후에만 보인다).
3. Object Storage 화면의 **Namespace**로 엔드포인트를 만든다: `https://<namespace>.compat.objectstorage.<region>.oraclecloud.com`
4. 위 표의 `OCI_*` 환경변수를 실행 환경에만 지정하고 서버를 시작한다.

설정이 빠졌거나 버킷에 접근할 수 없으면 서버는 시작하지 않고 원인을 출력한다. 각 녹화본이 어느 저장소에 있는지는 DB에 기록되므로, 저장소를 바꿔도 기존 녹화본의 위치는 남는다(다른 저장소에 있는 녹화본은 원래 저장소로 되돌리기 전까지 재생되지 않는다).

> ⚠️ OCI Object Storage의 Always Free 한도는 20GB다. 한도를 넘으면 PAYG 계정에서는 과금되므로 `STORAGE_QUOTA_BYTES`(기본 15GiB)를 그보다 작게 유지한다.

## API

| 메서드·경로 | 설명 |
|---|---|
| `GET /api/broadcasts` | 방송 목록(최신 등록순) |
| `POST /api/broadcasts` | 방송 등록. `multipart/form-data`로 `title`(텍스트)과 `recording`(mp4/webm 파일)을 보낸다. 성공 시 201 |
| `GET /api/broadcasts/{id}` | 방송 상세. 없으면 404 |
| `GET /api/broadcasts/{id}/recording` | 녹화본 재생. `Range` 요청을 지원한다(206) |
| `GET /api/upload-policy` | 녹화본 하나의 최대 크기와 남은 저장 공간 |

오류 응답 본문은 `{ "message": "..." }` 형태의 한국어 메시지다.

| 상태 | 방송 등록이 거부되는 경우 |
|---|---|
| 400 | 제목 누락·공백뿐·100자 초과(앞뒤 공백 제거 후), 녹화본 누락 |
| 413 | 녹화본이 `MAX_UPLOAD_BYTES`보다 큼 |
| 415 | mp4·webm이 아닌 파일 |
| 507 | 등록하면 녹화본 크기 합이 `STORAGE_QUOTA_BYTES`를 넘음 |

거부된 등록은 목록에 나타나지 않고 저장소에 파일을 남기지 않는다.

## 배포

OCI(오사카 리전) Always Free 범위 안에서 A1 VM 1대 + 비공개 Object Storage 버킷으로 배포한다. 콘솔에서 자원을 만드는 순서와 과금 확인 포인트는 [docs/deploy-oci.md](docs/deploy-oci.md)에 있다.

VM에서는 Docker Compose로 앱과 Caddy 리버스 프록시를 띄운다. 앱 컨테이너 하나가 API와 빌드된 웹 화면을 같은 주소로 서빙한다.

```bash
cp .env.example .env   # STORAGE_BACKEND=oci 와 OCI_* 값을 채운다(커밋 금지)
docker compose up -d --build
```

## 테스트

```bash
cargo test -p server
```

서버 HTTP API 통합 테스트. 테스트마다 임시 SQLite 파일과 임시 저장소를 쓴다.
