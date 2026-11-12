# clone-chzzk

방송을 송출·등록하고 시청하며 댓글을 나누는 서비스. 최종 목표는 데스크톱에서 송출한 라이브 방송을 웹에서 시청하는 것이며, 과제 단위로 하나씩 확장한다.

- 도메인 용어: [GLOSSARY.md](GLOSSARY.md)
- 설계 결정: [docs/adr/](docs/adr/) (라이브 송출 방식은 [ADR 0003](docs/adr/0003-live-ingest-websocket-to-hls.md), 로그인 방식은 [ADR 0004](docs/adr/0004-jwt-auth-with-revocation-list.md))

## 이번 단계(과제 3) 구현 범위

과제 2까지의 녹화방송·휴대폰 라이브 송출에 **사용자 인증·인가와 댓글**을 더했다. 권한은 화면에서 버튼을 숨기는 것으로 끝내지 않고, **모든 쓰기 요청을 서버가 검사**한다.

| 기능 | 구현 |
|---|---|
| 회원가입·로그인·로그아웃 | 아이디(3~20자, 영문 소문자·숫자·밑줄)와 비밀번호(8~72자)로 가입하면 바로 로그인된다. 비밀번호는 argon2id 해시로만 저장한다. 로그인하면 서버가 서명한 JWT를 받고, 웹은 이를 브라우저 저장소에 두어 새로 고쳐도 로그인이 유지된다. 로그아웃하면 서버가 그 토큰을 폐기 목록에 올려 만료 전이라도 다시 쓸 수 없다. |
| 인증 상태 관리 | 서버의 인증 추출기(`AuthUser`)가 `Authorization: Bearer` 토큰의 서명·만료·폐기 여부와 사용자 존재를 확인한다. 웹은 상단에 로그인한 아이디와 로그아웃 버튼을 보여 주고, 서버가 토큰을 거절(401)하면 로그아웃 상태로 바꾼다. |
| 방송 소유자 | 녹화방송을 등록하거나 라이브 송출을 시작한 사용자가 그 방송의 소유자가 된다. 목록과 상세에 방송한 사람이 보인다. |
| 방송 관리 권한 | 소유자만 제목 수정, 방송 삭제(녹화본·댓글 포함), 라이브 강제 종료(송출하던 기기가 아닌 곳에서도)를 할 수 있다. 서버는 비로그인 401, 다른 사용자 403으로 거절한다. 라이브 중인 방송은 삭제할 수 없다(409). |
| 송출 제어 권한 | 녹화방송 등록(401)과 라이브 송출 시작(시작 메시지의 토큰 확인)은 로그인한 사용자만 할 수 있다. 소유자가 다른 기기에서 라이브를 끝내면 송출 화면에 그 이유가 나온다. |
| 댓글 | 방송 상세 아래에서 로그인한 사용자가 댓글(1~300자)을 쓰고, 자기 댓글만 지울 수 있다. 비로그인 작성은 401, 다른 사람의 댓글 삭제는 403(방송 소유자여도)으로 거절한다. 댓글 수정 기능은 두지 않았다. |
| 댓글 실시간 반영 | 상세 화면이 방송별 댓글 WebSocket에 연결해, 다른 사람이 쓰거나 지운 댓글을 새로 고치지 않아도 바로 반영한다. 연결이 끊기면 3초 뒤 다시 연결하고 놓친 댓글을 채운다. |

### 공개 범위

보기는 로그인 없이 공개하고, 쓰기만 로그인과 권한을 요구한다.

| 기능 | 비로그인 | 로그인한 사용자 | 방송 소유자 | 댓글 작성자 |
|---|---|---|---|---|
| 방송 목록·상세 보기, 녹화본 재생, 라이브 시청 | ✅ | ✅ | ✅ | ✅ |
| 댓글 목록 보기, 댓글 실시간 연결 | ✅ | ✅ | ✅ | ✅ |
| 녹화방송 등록, 라이브 송출 시작 | ❌ 401 | ✅ (본인이 소유자가 됨) | ✅ | ✅ |
| 방송 제목 수정, 방송 삭제, 라이브 종료 | ❌ 401 | ❌ 403 | ✅ | ❌ 403 (소유자가 아니면) |
| 댓글 작성 | ❌ 401 | ✅ | ✅ | ✅ |
| 댓글 삭제 | ❌ 401 | ❌ 403 | ❌ 403 (작성자가 아니면) | ✅ |

서버의 판정 순서: 로그인하지 않았거나 토큰이 잘못·만료·폐기되었으면 먼저 **401**, 없는 방송·댓글이면 **404**, 소유자·작성자가 아니면 **403**, 상태가 맞지 않으면(라이브 중 삭제, 이미 끝난 방송 종료) **409**. 방송 권한 확인은 `apps/server/src/api.rs`의 `find_owned`, 댓글 삭제 권한은 `apps/server/src/comments.rs`의 `delete_comment`에 있다.

**과제 1·2 기능(유지)**: 녹화방송 등록(제목 + mp4·webm 업로드)·목록·상세·재생(Range 탐색)·공유 URL·등록 검증(400·413·415·507)·local/OCI 저장소, 휴대폰 카메라·마이크 라이브 송출(WebSocket → ffmpeg → HLS)·라이브 목록과 시청·방송 상태(`라이브 중`/`종료`)·종료 후 녹화본·끊김과 재시작 처리. 달라진 점은 **등록과 송출에 로그인이 필요**하다는 것뿐이다.

> 과제 3으로 올리면 소유자가 없는 이전 방송(과제 1·2에서 만든 방송)은 마이그레이션에서 녹화본과 함께 지워진다. 소유자가 없으면 아무도 관리할 수 없기 때문이다. 녹화본 파일은 서버가 시작할 때 저장소에서 지운다.

**이번 단계에서 하지 않는 것**: 소셜 로그인, 이메일 인증, 비밀번호 찾기·변경, 회원 탈퇴, 리프레시 토큰, 관리자 역할, 댓글 수정·답글·신고, 도네이션, 시청자 수, 데스크톱 송출 앱, 방송 소유권 이전, 페이지네이션·검색.

## 구성

Cargo workspace 모노레포.

| 경로 | 설명 |
|---|---|
| `apps/server` | axum API 서버. SQLite(sqlx)에 사용자·방송·댓글을 저장한다. `src/auth.rs`가 회원가입·로그인·로그아웃과 토큰 검증(`AuthUser`), `src/api.rs`가 방송 API와 소유자 권한 확인(`find_owned`), `src/comments.rs`가 댓글 API와 실시간 WebSocket, `src/live.rs`가 송출 WebSocket·ffmpeg HLS 변환·라이브 재생·종료 후 녹화본을 맡는다. |
| `apps/web` | Dioxus 0.7 웹 앱. 방송 목록·상세(녹화본/라이브 플레이어, 소유자 관리, 댓글)·등록·라이브 송출·로그인·회원가입 화면. `src/session.rs`가 로그인 상태, `src/comments.rs`가 댓글 영역과 실시간 연결, `src/live.rs`가 카메라·마이크와 송출을 맡는다. |
| `crates/shared` | 서버와 웹이 함께 쓰는 API 타입, 송출·댓글 WebSocket 메시지. |

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

`JWT_SECRET`을 정하지 않으면 서버가 시작할 때마다 임시 서명 키를 만든다(로그에 경고가 나온다). 이 경우 서버를 다시 켜면 모든 사용자가 로그아웃되므로 다시 로그인하면 된다. 로그인을 유지하려면 직접 만든 값을 지정한다.

```bash
JWT_SECRET="$(openssl rand -base64 48)" cargo run -p server
```

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

### 과제 3: 비로그인·방송 소유자·다른 사용자별 허용과 거절

로컬 실행 상태(웹 http://localhost:8080, 서버 http://localhost:3000)에서 확인한다. 사용자 두 명(**A**: 방송 소유자, **B**: 다른 사용자)을 쓴다. 한 브라우저에서 로그아웃·로그인을 바꿔 가며 해도 되고, 일반 창(A)과 시크릿 창(B)을 나란히 띄우면 편하다.

**1. 회원가입·로그인·로그아웃**

1. 오른쪽 위 **회원가입** → 아이디 `user_a`, 비밀번호 `password-a1`로 가입한다 → 상단에 `user_a`와 **로그아웃**이 보인다.
2. 새로 고쳐도 로그인이 유지되는지 확인한다.
3. **로그아웃** → 상단이 **로그인**·**회원가입**으로 바뀐다. **로그인**에서 비밀번호를 틀리게 넣으면 "아이디 또는 비밀번호가 올바르지 않습니다."가 나온다. 맞게 넣으면 다시 로그인된다.
4. 시크릿 창에서 `user_b` / `password-b1`로 가입한다. 같은 아이디(`USER_A`처럼 대소문자만 다른 것 포함)로 가입하면 "이미 쓰고 있는 아이디입니다."가 나온다.

**2. 방송 소유와 관리 (화면)**

| 단계 | 비로그인 | A (소유자) | B (다른 사용자) |
|---|---|---|---|
| **방송 등록**·**라이브 송출** 화면 | "로그인이 필요합니다" 안내와 로그인 링크만 보인다 | 등록·송출할 수 있다 | 등록·송출할 수 있다(자기 방송이 된다) |
| A가 등록한 방송의 상세 | **방송 관리** 영역이 없다 | **방송 관리**에 **제목 수정**·**방송 삭제**(라이브 중이면 **라이브 종료**)가 보인다 | **방송 관리** 영역이 없다 |

1. A로 **방송 등록**에서 녹화본을 올린다 → 상세 화면 제목 아래에 `user_a`가 보이고, **방송 관리**가 나온다.
2. A가 **제목 수정** → 새 제목 → **저장**하면 제목이 바뀐다.
3. B(시크릿 창)로 같은 공유 URL을 연다 → 영상은 보이지만 **방송 관리**가 없다.
4. A가 **방송 삭제** → 확인하면 목록으로 이동하고 방송이 사라진다.

**3. 화면 대신 API로 직접 요청해도 서버가 거절하는지** (`curl`, `jq` 사용. macOS: `brew install jq`)

버튼이 숨겨져 있어도 서버가 권한을 검사하는지 확인한다. 새 터미널(bash 또는 zsh)에서 순서대로 붙여 넣는다. `clip.mp4`는 가지고 있는 아무 mp4 파일 경로로 바꾼다.

```bash
API=http://localhost:3000
# check <이름> <토큰(비로그인은 '')> <curl 인자...>: 응답 상태 코드만 출력한다.
check() { name=$1; token=$2; shift 2; if [ -n "$token" ]; then set -- -H "Authorization: Bearer $token" "$@"; fi; curl -s -o /dev/null -w "$name: %{http_code}\n" "$@"; }
TOKEN_A=$(curl -s -X POST $API/api/auth/login -H 'content-type: application/json' -d '{"username":"user_a","password":"password-a1"}' | jq -r .token)
TOKEN_B=$(curl -s -X POST $API/api/auth/login -H 'content-type: application/json' -d '{"username":"user_b","password":"password-b1"}' | jq -r .token)
```

```bash
check '비로그인 방송 등록 (401)' '' -X POST $API/api/broadcasts -F title=비로그인 -F recording=@clip.mp4
ID=$(curl -s -X POST $API/api/broadcasts -H "Authorization: Bearer $TOKEN_A" -F title=권한확인 -F recording=@clip.mp4 | jq -r .id)
JSON='content-type: application/json'
check '제목 수정 - 비로그인 (401)' '' -X PATCH $API/api/broadcasts/$ID -H "$JSON" -d '{"title":"바꾼 제목"}'
check '제목 수정 - B (403)' "$TOKEN_B" -X PATCH $API/api/broadcasts/$ID -H "$JSON" -d '{"title":"바꾼 제목"}'
check '제목 수정 - A (200)' "$TOKEN_A" -X PATCH $API/api/broadcasts/$ID -H "$JSON" -d '{"title":"바꾼 제목"}'
```

```bash
check '댓글 작성 - 비로그인 (401)' '' -X POST $API/api/broadcasts/$ID/comments -H "$JSON" -d '{"body":"안녕"}'
CID=$(curl -s -X POST $API/api/broadcasts/$ID/comments -H "Authorization: Bearer $TOKEN_B" -H "$JSON" -d '{"body":"B의 댓글"}' | jq -r .id)
check '댓글 삭제 - 비로그인 (401)' '' -X DELETE $API/api/broadcasts/$ID/comments/$CID
check '댓글 삭제 - A, 방송 소유자지만 작성자 아님 (403)' "$TOKEN_A" -X DELETE $API/api/broadcasts/$ID/comments/$CID
check '댓글 삭제 - B, 작성자 (204)' "$TOKEN_B" -X DELETE $API/api/broadcasts/$ID/comments/$CID
```

```bash
check '방송 삭제 - 비로그인 (401)' '' -X DELETE $API/api/broadcasts/$ID
check '방송 삭제 - B (403)' "$TOKEN_B" -X DELETE $API/api/broadcasts/$ID
check '방송 삭제 - A (204)' "$TOKEN_A" -X DELETE $API/api/broadcasts/$ID
check '로그아웃 - B (204)' "$TOKEN_B" -X POST $API/api/auth/logout
check '로그아웃한 토큰으로 내 정보 (401)' "$TOKEN_B" $API/api/auth/me
```

각 줄의 괄호 안 숫자와 실제 상태 코드가 같으면 된다.

**4. 라이브 송출 제어**

1. A로 **라이브 송출**을 시작한다(같은 PC 웹캠 또는 [휴대폰 연결](#휴대폰-연결-https-주소)).
2. B로 그 방송의 상세를 열면 **방송 관리**가 없다. 위 3의 첫 블록을 다시 실행해 토큰을 새로 받은 뒤 `check '라이브 종료 - B (403)' "$TOKEN_B" -X POST $API/api/broadcasts/<방송 ID>/end`를 보내면 403이고 송출은 계속된다.
3. A가 다른 창(또는 다른 기기)에서 그 방송의 상세를 열어 **라이브 종료**를 누른다 → 송출 화면에 "소유자가 다른 기기에서 방송을 종료했습니다."가 나오고, 상세 화면은 `종료`로 바뀐 뒤 녹화본으로 바뀐다.
4. 라이브 중에 **방송 삭제**는 보이지 않으며, API로 삭제를 보내면 409("라이브 중인 방송은 먼저 종료해야 삭제할 수 있습니다.")다.

**5. 댓글 (화면, 실시간)**

1. 비로그인 창에서 방송 상세를 열면 댓글 목록은 보이지만 입력창 대신 "댓글을 쓰려면 로그인이 필요합니다"가 나온다.
2. A 창과 B 창에서 같은 방송 상세를 연다. B가 댓글을 쓰면 **A 창에 새로 고치지 않아도** 바로 나타난다.
3. B의 댓글에는 B 창에서만 **삭제**가 보인다. B가 지우면 A 창에서도 바로 사라진다.
4. 비었거나 300자를 넘는 댓글은 이유와 함께 거절된다.

### 과제 2: 휴대폰 송출 → 다른 화면에서 시청 (유지)

위 [휴대폰 연결](#휴대폰-연결-https-주소)의 HTTPS 주소를 휴대폰과 PC(또는 다른 기기)에서 연다. 시청하는 PC는 로그인하지 않아도 된다.

0. **휴대폰**: 오른쪽 위 **회원가입**(또는 **로그인**)으로 로그인한다. 로그인하지 않으면 송출 화면에 로그인 안내만 나온다.
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

1. http://localhost:8080 을 연다 → **방송 목록**에 "지금 라이브 중인 방송이 없습니다."와 "등록된 방송이 없습니다."가 보인다. 오른쪽 위 **회원가입**으로 가입한다(등록에는 로그인이 필요하다).
2. 오른쪽 위 **방송 등록** → 제목을 입력하고 녹화본 파일을 고른 뒤 **등록**을 누른다.
   - 버튼이 "업로드 중…"으로 바뀌었다가, 끝나면 방금 등록한 방송의 **상세 화면**으로 이동한다.
3. 상세 화면에서 영상이 재생되는지, 재생 바를 움직여 원하는 지점으로 건너뛸 수 있는지 확인한다.
4. **공유 URL**의 **복사**를 누른 뒤, 새 탭(또는 시크릿 창)에 붙여넣어 같은 방송이 열리는지 확인한다.
5. 상단 **clone-chzzk** 또는 **← 목록으로**를 눌러 **지난 방송**에 방금 등록한 방송이 `종료` 배지, 제목, 등록 시각과 함께 보이는지 확인한다. 하나 더 등록하면 새 방송이 맨 앞에 온다.
6. **재시작 후 유지**: 터미널 1의 서버를 `Ctrl+C`로 끄고 `cargo run -p server`로 다시 켠다. 목록을 새로고침해 방송이 그대로 있는지, 상세 화면에서 다시 재생되는지 확인한다(`JWT_SECRET`을 정하지 않았다면 로그인은 풀린다).
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
| `JWT_SECRET` | (없음, 임시 키) | 로그인 토큰(JWT) 서명 키. 실제 값은 실행 환경에만 둔다. 예: `openssl rand -base64 48`로 만든 값. 비우면 시작할 때마다 임시 키를 만든다(재시작하면 모두 로그아웃) |
| `JWT_TTL_SECS` | `86400` (24시간) | 로그인 토큰 유효 시간 |
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

인증 열: 공개 = 로그인 없이, 로그인 = `Authorization: Bearer <토큰>` 필요, 소유자·작성자 = 로그인한 사용자가 그 방송의 소유자·댓글의 작성자여야 한다.

| 메서드·경로 | 인증 | 설명 |
|---|---|---|
| `POST /api/auth/signup` | 공개 | 회원가입. `{"username","password"}` → 201 `{"token","user"}`. 중복 아이디 409, 규칙 위반 400 |
| `POST /api/auth/login` | 공개 | 로그인. 200 `{"token","user"}`. 아이디·비밀번호가 틀리면 401(어느 쪽인지 밝히지 않음) |
| `POST /api/auth/logout` | 로그인 | 요청에 쓴 토큰을 폐기한다. 204 |
| `GET /api/auth/me` | 로그인 | 내 정보 `{"id","username"}` |
| `GET /api/broadcasts` | 공개 | 방송 목록(최신순). `?status=live` 또는 `?status=ended`로 거른다(그 밖의 값은 400). 방송마다 `owner`(소유자)를 담는다 |
| `POST /api/broadcasts` | 로그인 | 방송 등록. `multipart/form-data`로 `title`과 `recording`(mp4/webm)을 보낸다. 등록한 사용자가 소유자. 201 |
| `GET /api/broadcasts/{id}` | 공개 | 방송 상세. `owner`, `status`(`live`/`ended`), `ended_at`, `recording`, `live_playlist`를 담는다. 없으면 404 |
| `PATCH /api/broadcasts/{id}` | 소유자 | 제목 수정 `{"title"}`. 제목 규칙은 등록과 같다(400). 200 |
| `DELETE /api/broadcasts/{id}` | 소유자 | 방송 삭제(녹화본·댓글 포함). 라이브 중이면 409. 204 |
| `POST /api/broadcasts/{id}/end` | 소유자 | 라이브 종료. 송출 연결에 종료를 알린다. 이미 종료된 방송이면 409. 200 |
| `GET /api/broadcasts/{id}/recording` | 공개 | 녹화본 재생. `Range` 요청을 지원한다(206) |
| `GET /api/broadcasts/{id}/live/index.m3u8` | 공개 | 라이브 재생 목록(HLS). 같은 경로의 `segNNNNN.ts` 조각도 서빙한다. 그 밖의 이름은 404 |
| `GET /api/broadcasts/{id}/comments` | 공개 | 댓글 목록(오래된 순, 최근 500개까지). 없는 방송 404 |
| `POST /api/broadcasts/{id}/comments` | 로그인 | 댓글 작성 `{"body"}`(앞뒤 공백 제거 후 1~300자, 위반 400). 201 |
| `DELETE /api/broadcasts/{id}/comments/{comment_id}` | 작성자 | 댓글 삭제. 다른 사용자(방송 소유자 포함) 403. 204 |
| `GET /api/broadcasts/{id}/comments/ws` (WebSocket) | 공개 | 댓글 실시간 알림(읽기 전용). 아래 메시지 참고 |
| `GET /api/live/ingest` (WebSocket) | 로그인(시작 메시지의 토큰) | 라이브 송출. 아래 메시지 순서를 따른다 |
| `GET /api/upload-policy` | 공개 | 녹화본 하나의 최대 크기와 남은 저장 공간 |

오류 응답 본문은 `{ "message": "..." }` 형태의 한국어 메시지다. 권한 오류의 판정 순서는 401 → 404 → 403 → 409다([공개 범위](#공개-범위) 참고).

| 상태 | 방송 등록이 거부되는 경우 |
|---|---|
| 401 | 로그인하지 않았거나 토큰이 잘못·만료·폐기됨 |
| 400 | 제목 누락·공백뿐·100자 초과(앞뒤 공백 제거 후), 녹화본 누락 |
| 413 | 녹화본이 `MAX_UPLOAD_BYTES`보다 큼 |
| 415 | mp4·webm이 아닌 파일 |
| 507 | 등록하면 녹화본 크기 합이 `STORAGE_QUOTA_BYTES`를 넘음 |

거부된 등록은 목록에 나타나지 않고 저장소에 파일을 남기지 않는다.

### 송출 WebSocket 메시지

텍스트 메시지는 JSON이다(`crates/shared`의 `IngestClientMessage`, `IngestServerMessage`). 브라우저 WebSocket은 헤더를 붙일 수 없어, 로그인 토큰은 시작 메시지에 싣는다(주소에 넣지 않아 접속 로그에 남지 않는다).

1. 송출자 → 서버: `{"type":"start","token":"<로그인 토큰>","title":"방송 제목","mime_type":"video/webm;codecs=vp8,opus"}`
2. 서버 → 송출자: `{"type":"started","broadcast":{...}}` (라이브 방송 생성, 송출자가 소유자) 또는 `{"type":"error","message":"..."}` (거부, 방송 없음)
3. 송출자 → 서버: 바이너리 메시지로 MediaRecorder 녹화 조각(webm 또는 mp4)을 계속 보낸다.
4. 송출자 → 서버: `{"type":"end"}`. 연결을 끊어도 종료된다.
5. 서버 → 송출자: `{"type":"ended","reason":null}`. 서버가 먼저 끝냈으면 `reason`에 이유가 있다.

| 송출이 거부·종료되는 경우 | 시점 |
|---|---|
| 토큰 없음("로그인이 필요합니다.") 또는 잘못·만료·폐기된 토큰 | 시작 거부 |
| 제목 누락·100자 초과 | 시작 거부 |
| 서버에 ffmpeg 없음 | 시작 거부 |
| 저장 공간이 남지 않음 | 시작 거부 |
| 동시 라이브 수(`MAX_LIVE_BROADCASTS`) 초과 | 시작 거부 |
| 소유자가 라이브 종료 API를 부름("소유자가 다른 기기에서 방송을 종료했습니다.") | 송출 중 종료 |
| 받은 영상이 녹화본 상한 또는 남은 저장 공간을 넘음 | 송출 중 종료 |
| 영상이 `LIVE_IDLE_TIMEOUT_SECS` 동안 오지 않음 | 송출 중 종료 |
| 연결 끊김 | 송출 중 종료 |

### 댓글 WebSocket 메시지

읽기 전용이다(`crates/shared`의 `CommentEvent`). 연결 전에 쓰인 댓글은 보내지 않으므로, 연결한 뒤 댓글 목록 API로 채운다. 방송이 삭제되면 서버가 연결을 닫는다.

- `{"type":"created","comment":{"id","broadcast_id","author":{"id","username"},"body","created_at"}}`
- `{"type":"deleted","comment_id":"..."}`

## 배포

OCI(오사카 리전) Always Free 범위 안에서 A1 VM 1대 + 비공개 Object Storage 버킷으로 배포한다. 콘솔에서 자원을 만드는 순서, sslip.io HTTPS 설정, 과금 확인 포인트는 [docs/deploy-oci.md](docs/deploy-oci.md)에 있다.

VM에서는 Docker Compose로 앱(ffmpeg 포함)과 Caddy 리버스 프록시를 띄운다. 앱 컨테이너 하나가 API, 송출 WebSocket, 빌드된 웹 화면을 같은 주소로 서빙한다.

```bash
cp .env.example .env   # STORAGE_BACKEND=oci, OCI_* 값, SITE_ADDRESS, JWT_SECRET을 채운다(커밋 금지)
docker compose up -d --build
```

## 테스트

```bash
cargo test -p server
```

서버 통합 테스트 68개와 단위 테스트 1개. 테스트마다 임시 SQLite 파일, 임시 저장소, 임시 라이브 디렉터리를 쓰며, 테스트용 서명 키(`test-only-jwt-secret`)는 실제 비밀 값이 아니다. 송출 테스트는 ffmpeg가 필요하다.

- `tests/auth.rs`(9개): 회원가입(소문자 저장, 중복·규칙 위반 거절), 로그인(틀린 비밀번호와 없는 아이디가 같은 메시지), 로그아웃한 토큰 401, 만료·다른 키·없는 사용자·형식 오류 토큰 401, 임시 서명 키
- `tests/api.rs`(31개): 과제 1·2 등록·목록·상세·재생(Range)·거부·재시작·웹 서빙·상태, 비로그인 등록 401, 소유자 기록, 권한 표(비로그인 401 · 다른 사용자 403 · 소유자 허용)로 제목 수정·삭제·라이브 종료, 삭제 시 녹화본·저장 공간 반환, 라이브 중 삭제와 종료된 방송 종료 409, 과제 2 DB의 마이그레이션(소유자 없는 방송과 녹화본 정리)
- `tests/comments.rs`(11개): 작성·목록(오래된 순, 작성자), 비로그인 작성 401, 내용 규칙 400, 없는 방송 404, 작성자만 삭제(방송 소유자도 403), 다른 방송 경로로 삭제 불가, 방송 삭제 시 댓글 삭제, 댓글 WebSocket의 생성·삭제 알림, 다른 방송 알림 없음, 없는 방송 404, 방송 삭제 시 연결 닫힘
- `tests/live.rs`(17개): 실제 포트에 서버를 띄우고 ffmpeg로 만든 webm을 WebSocket으로 송출한다. 라이브 목록 → HLS 서빙 → 종료 → 녹화본, 연결 끊김, 시작 거부(로그인·제목·동시 수·저장 공간·ffmpeg 없음), 송출한 사용자가 소유자, 소유자가 다른 기기에서 종료(다른 사용자는 403), 크기 상한·무응답 종료, 저장 공간이 찬 경우, 재시작 복구, 라이브 파일 경로 검사

웹 앱은 컴파일로 확인한다.

```bash
cargo clippy -p web --target wasm32-unknown-unknown
dx build -p web --release
```
