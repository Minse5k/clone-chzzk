# clone-chzzk

로그인 없이 방송을 송출·등록하고 시청하는 서비스. 최종 목표는 데스크톱에서 송출한 라이브 방송을 웹에서 시청하는 것이며, 과제 단위로 하나씩 확장한다.

- 도메인 용어: [GLOSSARY.md](GLOSSARY.md)
- 설계 결정: [docs/adr/](docs/adr/) (라이브 송출 방식은 [ADR 0003](docs/adr/0003-live-ingest-websocket-to-hls.md))

## 이번 단계(과제 2) 구현 범위

과제 1의 녹화방송 기능에 **휴대폰 라이브 송출과 시청**을 더했다. 로그인과 권한 검사는 없다.

| 기능 | 구현 |
|---|---|
| 라이브 송출 | 휴대폰 브라우저의 **라이브 송출** 화면에서 카메라·마이크 권한을 받아 미리보기를 띄우고, **방송 시작**·**방송 종료**로 송출한다. 전면·후면 카메라 전환, 경과 시간 표시, 송출 중 공유 URL 복사를 지원한다. |
| 송출 방식 | 브라우저 MediaRecorder가 1초 단위 녹화 조각을 WebSocket(`/api/live/ingest`)으로 보내면, 서버가 방송마다 ffmpeg 하나로 H.264/AAC 2초 단위 HLS를 만든다. 별도 앱 설치나 송출 키가 없다. |
| 라이브 시청 | 방송 목록의 **라이브 중** 섹션에서 방송을 고르거나 공유 URL로 들어와 HLS로 시청한다(지연 3~6초). Safari는 기본 HLS 재생을 쓰고, 나머지 브라우저는 hls.js를 쓴다. 중간에 들어와도 지금 시점부터 본다. |
| 방송 상태 | `라이브 중`·`종료`를 목록과 상세에 배지로 구분한다. 목록은 **라이브 중**과 **지난 방송**으로 나뉜다. 시청 화면은 3초마다 상태를 다시 확인해, 방송이 끝나면 종료로 바뀐다. |
| 종료 후 녹화본 | 송출이 끝나면 HLS 조각을 다시 인코딩 없이 mp4로 묶어 녹화본 저장소(local/OCI)에 저장한다. 과제 1과 같은 상세·플레이어로 다시 본다. |
| 끊김·예외 처리 | 송출 연결이 끊기거나 영상이 20초 동안 오지 않으면 방송을 종료한다. 녹화본 크기 상한이나 저장 공간 한도에 닿으면 이유를 알리고 종료한다. 동시 라이브 수가 차 있거나 서버에 ffmpeg가 없으면 시작을 거부한다. 서버가 송출 도중 재시작되면 남은 방송을 종료하고 남은 영상으로 녹화본을 만든다. |
| 송출 화면 안내 | 권한 거부, 카메라 없음, HTTPS가 아닌 주소, 미지원 브라우저, 서버의 거부·종료 이유를 각각 다른 문구로 안내한다. 송출 중에는 페이지 이탈 경고와 화면 꺼짐 방지(지원 브라우저)를 켠다. |
| 로그인 없음 | 송출·시청 모두 인증 없이 쓴다. 송출 연결의 수명이 곧 방송의 수명이어서, 송출자만 자기 방송을 끝낼 수 있다. |

**과제 1 기능(유지)**: 녹화방송 등록(제목 + mp4·webm 업로드), 목록(최신순), 상세·재생(Range 탐색), 공유 URL, 영속 저장(SQLite), 등록 검증(400·413·415·507), local/OCI 녹화본 저장소. 과제 1에서 등록한 방송은 `종료` 상태로 그대로 보이고 재생된다.

**이번 단계에서 하지 않는 것**: 로그인·권한 검사, 송출 키, 댓글, 도네이션, 시청자 수, 데스크톱 송출 앱, WebRTC 같은 1초 미만 저지연 송출, 여러 화질(ABR), 끊긴 송출을 같은 방송으로 이어 가기, 썸네일, 방송 수정·삭제, 페이지네이션·검색.

## 구성

Cargo workspace 모노레포.

| 경로 | 설명 |
|---|---|
| `apps/server` | axum API 서버. SQLite(sqlx)에 방송 정보를 저장한다. `src/live.rs`가 송출 WebSocket, ffmpeg HLS 변환, 라이브 재생 파일 서빙, 종료 후 녹화본 생성을 맡는다. |
| `apps/web` | Dioxus 0.7 웹 앱. 방송 목록·상세(녹화본/라이브 플레이어)·등록·라이브 송출 화면. `src/live.rs`가 카메라·마이크, MediaRecorder, WebSocket 송출을 맡는다. |
| `crates/shared` | 서버와 웹이 함께 쓰는 API 타입과 송출 메시지. |

## 준비물

- Rust (stable) 와 `wasm32-unknown-unknown` 타깃: `rustup target add wasm32-unknown-unknown`
- Dioxus CLI 0.7: `cargo install dioxus-cli --version ^0.7 --locked`
- ffmpeg (라이브 송출용): macOS `brew install ffmpeg`, Ubuntu `sudo apt-get install ffmpeg`
  - 없어도 서버는 시작하며 녹화방송 기능은 그대로 동작한다. 송출만 거부된다.

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

브라우저에서 http://localhost:8080 을 연다. 데이터는 기본적으로 `data/` 아래(SQLite 파일, 녹화본, 라이브 HLS 조각)에 저장된다. 이 상태에서 같은 PC의 브라우저는 웹캠으로 송출할 수 있다(localhost는 브라우저가 보안 출처로 취급한다).

8080 포트를 이미 다른 프로그램이 쓰고 있다면 웹 포트를 바꾸고, 서버가 그 출처를 허용하게 한다.

```bash
CORS_ALLOWED_ORIGINS=http://localhost:8090 cargo run -p server
```

```bash
dx serve -p web --port 8090
```

## 휴대폰 연결 (HTTPS 주소)

휴대폰 브라우저는 **HTTPS 주소(또는 localhost)에서만 카메라·마이크를 허용**한다. `http://192.168.x.x:8080`처럼 같은 Wi-Fi의 HTTP 주소로 열면 송출 화면에 "HTTPS 주소에서만 쓸 수 있습니다" 안내가 나오고 카메라가 켜지지 않는다. 시청은 HTTP에서도 된다. 아래 둘 중 하나로 HTTPS 주소를 만든다.

### 방법 1: 로컬 PC + 임시 HTTPS 터널 (계정 불필요)

서버가 빌드된 웹 화면까지 같은 주소로 서빙하게 띄운 뒤, [cloudflared](https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/downloads/) 임시 터널로 HTTPS 주소를 받는다. 무료이며 가입이 필요 없다.

```bash
# 1) cloudflared 설치 (macOS). 다른 OS는 위 링크의 설치 방법을 따른다.
brew install cloudflared
```

```bash
# 2) 웹을 같은 출처용으로 빌드한다(API 주소를 비우면 웹을 연 주소로 API를 부른다).
API_BASE_URL= dx build -p web --release
```

```bash
# 3) 서버가 빌드된 웹도 함께 서빙하게 띄운다.
WEB_DIST_DIR=target/dx/web/release/web/public cargo run -p server
```

```bash
# 4) 다른 터미널에서 터널을 연다. 출력의 https://<임의 이름>.trycloudflare.com 주소를 쓴다.
cloudflared tunnel --url http://localhost:3000
```

휴대폰과 PC 모두 그 `https://….trycloudflare.com` 주소로 연다. 터널을 끄면 주소가 사라지고, 다시 열면 주소가 바뀐다. Docker로 띄웠다면(`docker compose up -d --build`) 4)의 주소를 `http://localhost:80`으로 바꾼다.

### 방법 2: OCI 배포 + sslip.io 주소

도메인 없이 `https://<공인IP를 대시로>.sslip.io`(예: `158-101-1-2.sslip.io`) 주소에 Caddy가 Let's Encrypt 인증서를 자동으로 붙인다. 순서는 [docs/deploy-oci.md](docs/deploy-oci.md)에 있다.

## 카메라·마이크 권한 설정

송출 화면에서 **카메라·마이크 켜기**를 누르면 브라우저가 권한을 묻는다. 둘 다 **허용**한다.

| 브라우저 | 처음 허용 | 거부했던 권한을 다시 허용 |
|---|---|---|
| 안드로이드 Chrome | 팝업에서 **이번 방문 동안 허용** 또는 **앱 사용 중에 허용** | 주소창 왼쪽 아이콘 → **권한**(또는 사이트 설정) → 카메라·마이크 **허용** 후 새로고침. 안드로이드 설정 → 앱 → Chrome → 권한에서 카메라·마이크가 꺼져 있으면 켠다. |
| iOS Safari | 팝업에서 **허용** | 주소창의 **aA**(또는 페이지 메뉴) → **웹 사이트 설정** → 카메라·마이크 **허용** 후 새로고침. 설정 앱 → Safari → 카메라·마이크가 **거부**면 **확인** 또는 **허용**으로 바꾼다. |
| PC Chrome·Edge | 주소창 아래 팝업에서 **허용** | 주소창 왼쪽 아이콘 → 카메라·마이크 **허용** 후 새로고침 |

다른 앱(화상회의 등)이 카메라를 쓰고 있으면 "다른 앱이 카메라나 마이크를 쓰고 있습니다" 안내가 나온다. 그 앱을 닫고 다시 시도한다. 송출 중에 다른 앱으로 넘어가거나 화면을 끄면 휴대폰 브라우저가 카메라를 멈춘다. 20초 안에 돌아오지 않으면 방송이 종료된다.

## 직접 확인 절차

### 과제 2: 휴대폰 송출 → 다른 화면에서 시청

위 [휴대폰 연결](#휴대폰-연결-https-주소)의 HTTPS 주소를 휴대폰과 PC(또는 다른 기기)에서 연다.

1. **휴대폰**: 오른쪽 위 **라이브 송출** → **카메라·마이크 켜기** → 권한을 허용하면 미리보기가 나온다. **카메라 전환**으로 전면·후면을 바꿀 수 있다.
2. **휴대폰**: 제목을 입력하고 **방송 시작**을 누른다.
   - **송출 시작 확인**: 미리보기 왼쪽 위 배지가 "연결 중…"을 거쳐 빨간 `LIVE 00:01`로 바뀌고 시간이 올라간다. 아래에 "송출 중입니다"와 공유 URL이 나온다.
3. **PC**: 방송 목록을 연다(이미 열려 있으면 새로고침). **라이브 중** 섹션에 방금 방송이 `라이브 중` 배지와 함께 보인다.
4. **PC**: 방송을 누르면 상세 화면에서 휴대폰 카메라 영상이 3~6초 늦게 재생된다. 처음 몇 초는 "방송을 준비하고 있습니다"가 보일 수 있다. 자동 재생을 위해 소리가 꺼진 채 시작하므로, 플레이어의 음소거를 풀어 마이크 소리를 확인한다. 휴대폰 송출 화면의 공유 URL을 다른 기기에 붙여넣어도 같은 방송이 열린다.
5. **휴대폰**: **방송 종료**를 누른다.
   - **송출 종료 확인**: 배지가 "종료 중…"을 거쳐 사라지고 "방송이 종료되었습니다"와 **방송 보기** 버튼이 나온다.
6. **PC**: 상세 화면의 배지가 몇 초 안에 `종료`로 바뀐다. 잠시 뒤 녹화본 플레이어로 바뀌어 처음부터 다시 볼 수 있다. 목록에서는 방송이 **지난 방송**으로 옮겨 가 있다.
7. **끊김 확인**(선택): 송출 중에 휴대폰 브라우저 탭을 닫으면, PC의 상세 화면이 몇 초 안에 `종료`로 바뀐다.

서버 로그로도 확인할 수 있다: 송출을 시작하면 `송출 시작: <제목>`, 끝나면 `송출 종료`, 녹화본이 저장되면 `녹화본 저장: <크기>`가 찍힌다.

### 과제 1: 녹화방송 등록·시청 (유지)

로컬 실행 상태에서 확인한다. 녹화본으로 쓸 mp4 또는 webm 파일 하나를 준비한다.

1. http://localhost:8080 을 연다 → **방송 목록**에 "지금 라이브 중인 방송이 없습니다."와 "등록된 방송이 없습니다."가 보인다.
2. 오른쪽 위 **방송 등록** → 제목을 입력하고 녹화본 파일을 고른 뒤 **등록**을 누른다.
   - 버튼이 "업로드 중…"으로 바뀌었다가, 끝나면 방금 등록한 방송의 **상세 화면**으로 이동한다.
3. 상세 화면에서 영상이 재생되는지, 재생 바를 움직여 원하는 지점으로 건너뛸 수 있는지 확인한다.
4. **공유 URL**의 **복사**를 누른 뒤, 새 탭(또는 시크릿 창)에 붙여넣어 같은 방송이 열리는지 확인한다.
5. 상단 **clone-chzzk** 또는 **← 목록으로**를 눌러 **지난 방송**에 방금 등록한 방송이 `종료` 배지, 제목, 등록 시각과 함께 보이는지 확인한다. 하나 더 등록하면 새 방송이 맨 앞에 온다.
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
| `MAX_UPLOAD_BYTES` | `2147483648` (2GiB) | 녹화본 하나의 최대 크기. 등록은 넘으면 413, 라이브는 받은 영상이 넘으면 종료 |
| `STORAGE_QUOTA_BYTES` | `16106127360` (15GiB) | 저장된 녹화본 크기 합의 한도. 등록은 넘으면 507, 라이브는 닿으면 종료 |
| `LIVE_DIR` | `data/live` | 라이브 HLS 조각을 만드는 임시 디렉터리. 녹화본을 만든 뒤 지운다 |
| `FFMPEG_PATH` | `ffmpeg` | 송출을 HLS로 바꾸는 ffmpeg 실행 파일 |
| `MAX_LIVE_BROADCASTS` | `2` | 동시에 진행할 수 있는 라이브 방송 수(송출마다 ffmpeg가 CPU로 인코딩) |
| `LIVE_IDLE_TIMEOUT_SECS` | `20` | 송출 중 영상이 이 시간 동안 오지 않으면 방송 종료 |
| `CORS_ALLOWED_ORIGINS` | `http://localhost:8080` | 허용할 웹 출처(쉼표 구분, `*`는 전체) |
| `WEB_DIST_DIR` | (없음) | 빌드된 웹 정적 파일 디렉터리. 지정하면 서버가 웹 화면도 서빙한다(배포·터널용) |
| `SITE_ADDRESS` | (없음, HTTP :80) | (Docker Compose의 Caddy) HTTPS 주소. 예: `158-101-1-2.sslip.io` |
| `API_BASE_URL` | `http://localhost:3000` | (웹 빌드 시) 웹이 호출할 서버 주소. 비우면 웹을 연 주소와 같은 출처로 부른다 |

## 녹화본 저장소

기본값 `STORAGE_BACKEND=local`은 서버 디렉터리(`STORAGE_DIR`)에 녹화본을 저장하며 비밀 값이 필요 없다.

`STORAGE_BACKEND=oci`로 두면 OCI Object Storage의 **비공개 버킷**에 S3 호환 API로 저장한다. 업로드는 8MB 단위 멀티파트로 스트리밍되고, 재생은 서버가 Range 요청을 버킷으로 그대로 넘긴다(버킷을 공개할 필요 없음). 라이브 방송의 녹화본도 같은 저장소에 들어간다. 설계 근거는 [ADR 0002](docs/adr/0002-recordings-proxied-through-server.md).

1. OCI 콘솔에서 버킷을 만든다(공개 범위: Private).
2. 프로필 > 사용자 설정 > **Customer Secret Keys**에서 키를 발급한다(Secret Key는 발급 직후에만 보인다).
3. Object Storage 화면의 **Namespace**로 엔드포인트를 만든다: `https://<namespace>.compat.objectstorage.<region>.oraclecloud.com`
4. 위 표의 `OCI_*` 환경변수를 실행 환경에만 지정하고 서버를 시작한다.

설정이 빠졌거나 버킷에 접근할 수 없으면 서버는 시작하지 않고 원인을 출력한다. 각 녹화본이 어느 저장소에 있는지는 DB에 기록되므로, 저장소를 바꿔도 기존 녹화본의 위치는 남는다(다른 저장소에 있는 녹화본은 원래 저장소로 되돌리기 전까지 재생되지 않는다).

> ⚠️ OCI Object Storage의 Always Free 한도는 20GB다. 한도를 넘으면 PAYG 계정에서는 과금되므로 `STORAGE_QUOTA_BYTES`(기본 15GiB)를 그보다 작게 유지한다.

## API

| 메서드·경로 | 설명 |
|---|---|
| `GET /api/broadcasts` | 방송 목록(최신순). `?status=live` 또는 `?status=ended`로 상태를 거른다(그 밖의 값은 400) |
| `POST /api/broadcasts` | 방송 등록. `multipart/form-data`로 `title`(텍스트)과 `recording`(mp4/webm 파일)을 보낸다. 성공 시 201 |
| `GET /api/broadcasts/{id}` | 방송 상세. `status`(`live`/`ended`), `ended_at`, `recording`, `live_playlist`(라이브 재생 목록 있음)를 담는다. 없으면 404 |
| `GET /api/broadcasts/{id}/recording` | 녹화본 재생. `Range` 요청을 지원한다(206) |
| `GET /api/broadcasts/{id}/live/index.m3u8` | 라이브 재생 목록(HLS). 같은 경로의 `segNNNNN.ts` 조각도 서빙한다. 그 밖의 이름은 404 |
| `GET /api/live/ingest` (WebSocket) | 라이브 송출. 아래 메시지 순서를 따른다 |
| `GET /api/upload-policy` | 녹화본 하나의 최대 크기와 남은 저장 공간 |

오류 응답 본문은 `{ "message": "..." }` 형태의 한국어 메시지다.

| 상태 | 방송 등록이 거부되는 경우 |
|---|---|
| 400 | 제목 누락·공백뿐·100자 초과(앞뒤 공백 제거 후), 녹화본 누락 |
| 413 | 녹화본이 `MAX_UPLOAD_BYTES`보다 큼 |
| 415 | mp4·webm이 아닌 파일 |
| 507 | 등록하면 녹화본 크기 합이 `STORAGE_QUOTA_BYTES`를 넘음 |

거부된 등록은 목록에 나타나지 않고 저장소에 파일을 남기지 않는다.

### 송출 WebSocket 메시지

텍스트 메시지는 JSON이다(`crates/shared`의 `IngestClientMessage`, `IngestServerMessage`).

1. 송출자 → 서버: `{"type":"start","title":"방송 제목","mime_type":"video/webm;codecs=vp8,opus"}`
2. 서버 → 송출자: `{"type":"started","broadcast":{...}}` (라이브 방송 생성) 또는 `{"type":"error","message":"..."}` (거부, 방송 없음)
3. 송출자 → 서버: 바이너리 메시지로 MediaRecorder 녹화 조각(webm 또는 mp4)을 계속 보낸다.
4. 송출자 → 서버: `{"type":"end"}`. 연결을 끊어도 종료된다.
5. 서버 → 송출자: `{"type":"ended","reason":null}`. 서버가 먼저 끝냈으면 `reason`에 이유가 있다.

| 송출이 거부·종료되는 경우 | 시점 |
|---|---|
| 제목 누락·100자 초과 | 시작 거부 |
| 서버에 ffmpeg 없음 | 시작 거부 |
| 저장 공간이 남지 않음 | 시작 거부 |
| 동시 라이브 수(`MAX_LIVE_BROADCASTS`) 초과 | 시작 거부 |
| 받은 영상이 녹화본 상한 또는 남은 저장 공간을 넘음 | 송출 중 종료 |
| 영상이 `LIVE_IDLE_TIMEOUT_SECS` 동안 오지 않음 | 송출 중 종료 |
| 연결 끊김 | 송출 중 종료 |

## 배포

OCI(오사카 리전) Always Free 범위 안에서 A1 VM 1대 + 비공개 Object Storage 버킷으로 배포한다. 콘솔에서 자원을 만드는 순서, sslip.io HTTPS 설정, 과금 확인 포인트는 [docs/deploy-oci.md](docs/deploy-oci.md)에 있다.

VM에서는 Docker Compose로 앱(ffmpeg 포함)과 Caddy 리버스 프록시를 띄운다. 앱 컨테이너 하나가 API, 송출 WebSocket, 빌드된 웹 화면을 같은 주소로 서빙한다.

```bash
cp .env.example .env   # STORAGE_BACKEND=oci, OCI_* 값, SITE_ADDRESS를 채운다(커밋 금지)
docker compose up -d --build
```

## 테스트

```bash
cargo test -p server
```

서버 통합 테스트 36개와 단위 테스트 1개. 테스트마다 임시 SQLite 파일, 임시 저장소, 임시 라이브 디렉터리를 쓴다. 송출 테스트는 ffmpeg가 필요하다.

- `tests/api.rs`(22개): 등록·목록·상세·재생(Range), 거부(400/413/415/507)와 정리, 재시작 후 유지, 웹 정적 파일 서빙, 방송 상태와 상태 질의, 과제 1 DB의 마이그레이션
- `tests/live.rs`(14개): 실제 포트에 서버를 띄우고, ffmpeg로 만든 webm을 WebSocket으로 송출한다. 라이브 목록 → HLS 서빙 → 종료 → 녹화본, 연결 끊김, 시작 거부(제목·동시 수·저장 공간·ffmpeg 없음), 크기 상한·무응답 종료, 저장 공간이 찬 경우, 재시작 복구, 라이브 파일 경로 검사

웹 앱은 컴파일로 확인한다.

```bash
cargo clippy -p web --target wasm32-unknown-unknown
dx build -p web --release
```
