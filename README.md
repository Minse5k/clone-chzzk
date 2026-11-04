# clone-chzzk

로그인 없이 방송을 등록하고 시청하는 서비스. 최종 목표는 데스크톱에서 송출한 라이브 방송을 웹에서 시청하는 것이며, 과제 단위로 하나씩 확장한다.

- 도메인 용어: [GLOSSARY.md](GLOSSARY.md)
- 설계 결정: [docs/adr/](docs/adr/)

## 이번 단계(과제 1) 구현 범위

로그인 없이 **녹화된 방송을 등록하고 시청**한다.

| 기능 | 구현 |
|---|---|
| 방송 등록 | 제목과 녹화본(mp4·webm 파일 업로드)을 등록한다. 파일은 메모리에 쌓지 않고 저장소로 스트리밍된다. |
| 방송 목록 | 등록된 방송을 최신 등록순으로 보여 준다. 비어 있으면 안내 문구를 보여 준다. |
| 방송 상세·재생 | 제목·등록 시각과 `<video>` 플레이어. Range 요청을 지원해 원하는 지점으로 건너뛸 수 있다. |
| 공유 URL | 방송마다 고유 주소(`/broadcasts/{id}`)가 있고 상세 화면에서 복사할 수 있다. 이 주소로 바로 들어와도 열린다. |
| 영속 저장 | 방송 정보와 녹화본 위치(저장소 종류·객체 키·크기)를 SQLite에 저장한다. 서버를 재시작해도 남는다. |
| 녹화본 저장소 | 로컬 디렉터리(기본) 또는 OCI Object Storage 비공개 버킷 |
| 등록 검증 | 제목(1~100자), 형식(mp4·webm), 파일 크기 상한, 저장 총량 한도. 거부된 등록은 아무것도 남기지 않는다. |
| 로그인 없음 | 모든 화면과 API를 인증 없이 쓴다. |

**이번 단계에서 하지 않는 것**: 인증·인가, 라이브 송출(데스크톱 송출 앱), 댓글, 도네이션, 방송 수정·삭제, 외부 URL로 녹화본 연결, 트랜스코딩·썸네일, 목록 페이지네이션·검색, 도메인·HTTPS.

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

8080 포트를 이미 다른 프로그램이 쓰고 있다면 웹 포트를 바꾸고, 서버가 그 출처를 허용하게 한다.

```bash
CORS_ALLOWED_ORIGINS=http://localhost:8090 cargo run -p server
```

```bash
dx serve -p web --port 8090
```

## 직접 확인 절차

위 로컬 실행 상태에서 확인한다. 녹화본으로 쓸 mp4 또는 webm 파일 하나를 준비한다.

1. http://localhost:8080 을 연다 → **방송 목록**에 "등록된 방송이 없습니다."가 보인다.
2. 오른쪽 위 **방송 등록** → 제목을 입력하고 녹화본 파일을 고른 뒤 **등록**을 누른다.
   - 버튼이 "업로드 중…"으로 바뀌었다가, 끝나면 방금 등록한 방송의 **상세 화면**으로 이동한다.
3. 상세 화면에서 영상이 재생되는지, 재생 바를 움직여 원하는 지점으로 건너뛸 수 있는지 확인한다.
4. **공유 URL**의 **복사**를 누른 뒤, 새 탭(또는 시크릿 창)에 붙여넣어 같은 방송이 열리는지 확인한다.
5. 상단 **clone-chzzk** 또는 **← 목록으로**를 눌러 목록에 방금 등록한 방송이 제목·등록 시각과 함께 보이는지 확인한다. 하나 더 등록하면 새 방송이 맨 앞에 온다.
6. **재시작 후 유지**: 터미널 1의 서버를 `Ctrl+C`로 끄고 `cargo run -p server`로 다시 켠다. 목록을 새로고침해 방송이 그대로 있는지, 상세 화면에서 다시 재생되는지 확인한다.
7. **거부 확인**(선택): 제목을 비우거나, mp4·webm이 아닌 파일을 고르면 등록 화면에 이유가 표시되고 목록에 아무것도 추가되지 않는다. 없는 주소(예: `/broadcasts/없는-방송`)로 가면 "찾을 수 없음" 화면이 나온다.

## 환경변수

예시는 [.env.example](.env.example)에 있다(실제 비밀 값 없음). 로컬 실행은 기본값만으로 동작한다. 값을 바꾸려면 셸에서 지정하거나 `.env`를 만들어 불러온다. `.env`는 git에서 제외되어 있다.

```bash
cp .env.example .env
set -a; source .env; set +a
cargo run -p server
```

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

서버 HTTP API 통합 테스트(17개). 테스트마다 임시 SQLite 파일과 임시 저장소를 쓰며, 등록·목록·상세·재생(Range), 거부(400/413/415/507)와 정리, 재시작 후 유지, 웹 정적 파일 서빙을 확인한다.

웹 앱은 컴파일로 확인한다.

```bash
cargo clippy -p web --target wasm32-unknown-unknown
dx build -p web --release
```
