# clone-chzzk

방송을 송출·등록하고 시청하며 댓글과 후원을 나누는 서비스. 최종 목표는 데스크톱에서 송출한 라이브 방송을 웹에서 시청하는 것이며, 과제 단위로 하나씩 확장한다.

- 도메인 용어: [GLOSSARY.md](GLOSSARY.md)
- 설계 결정: [docs/adr/](docs/adr/) (라이브 송출 방식은 [ADR 0003](docs/adr/0003-live-ingest-websocket-to-hls.md), 로그인 방식은 [ADR 0004](docs/adr/0004-jwt-auth-with-revocation-list.md), 후원 결제 확정 방식은 [ADR 0005](docs/adr/0005-donation-confirmed-by-server-payment-approval.md), 여러 인스턴스의 공용 상태와 오토스케일 방식은 [ADR 0006](docs/adr/0006-multi-instance-shared-state.md))

## 이번 단계(과제 6) 구현 범위

과제 5까지의 서비스를 **산 도메인으로 OCI Always Free VM에 HTTPS로 배포**하고, 배포된 서버의 방송을 보는 **Windows·macOS 데스크톱 클라이언트**와 **다운로드 페이지**를 더했다.

- 서비스: **https://mins.work**
- 다운로드 페이지: **https://mins.work/download**
- 상태 확인: https://mins.work/healthz

| 요구 사항 | 구현 |
|---|---|
| 도메인 | `mins.work`, Cloudflare Registrar. 등록·DNS·TLS는 [도메인과 HTTPS](#도메인과-https)에 적었다. |
| OCI 배포 | 오사카 리전 A1.Flex 2 OCPU/12GB VM 1대에서 Docker Compose(PostgreSQL·Redis·app 1개·Caddy)로 띄운다. 절차는 [docs/deploy-oci.md](docs/deploy-oci.md), 사양과 무료 근거는 [OCI VM](#oci-vm)에 있다. |
| 기존 기능 유지 + 헬스 체크 | 과제 1~5의 서버·웹을 그대로 배포한다. `GET /healthz`가 인스턴스 상태를 JSON으로 답한다(내려가는 중이면 503). |
| 비밀 값·서비스 주소 | 모두 VM의 `.env`(저장소에 없음, `chmod 600`)로 넣는다: `SITE_ADDRESS`(도메인), `JWT_SECRET`, `POSTGRES_PASSWORD`, 토스 테스트 키, `CORS_ALLOWED_ORIGINS`. 예시는 [.env.example](.env.example). 데스크톱 앱의 서버 주소는 빌드 때 `CHZZK_SERVER_URL`(기본 `https://mins.work`)로 정하고, 실행 환경의 `CHZZK_SERVER_URL`이나 앱 위쪽 서버 칸으로 바꿀 수 있다. |
| 데스크톱 클라이언트 | [`apps/desktop`](apps/desktop/src/main.rs)(Dioxus 데스크톱, Windows는 WebView2·macOS는 WKWebView). 배포 서버의 방송 목록(전체·라이브·종료)을 받고, 고른 방송을 앱 안에서 재생한다. 라이브는 HLS(macOS는 웹뷰가 바로, Windows는 hls.js), 종료된 방송은 녹화본(Range 요청)이다. 재생 방식 판단은 [`api.rs`](apps/desktop/src/api.rs)(단위 테스트 6개). `--check` 옵션은 창 없이 목록과 재생 주소를 출력해 설치 확인과 CI에 쓴다. |
| Windows·macOS 패키지 | [`.github/workflows/desktop.yml`](.github/workflows/desktop.yml)이 `dx bundle`로 Windows x64(`.exe` 설치 프로그램, `.msi`)와 macOS arm64(`.dmg`, `.app.zip`)를 만든다. `desktop-v*` 태그를 올리면 GitHub Release에 고정된 파일 이름으로 올라간다. |
| 다운로드 페이지 | 웹의 [`/download`](apps/web/src/download.rs). OS별 파일 링크(`releases/latest/download/<파일>`), 지원 아키텍처, 설치·실행 절차, 서명하지 않은 앱 경고 넘기는 법. |

### 도메인과 HTTPS

| 항목 | 값 |
|---|---|
| 도메인 | `mins.work` |
| 등록 기관 | Cloudflare Registrar |
| 등록일 / 만료일 | 2026-11-20 / 2027-11-20 (1년) |
| 최초 등록 비용 | **US$10.20** (1년) |
| 갱신 비용 | **US$10.20 / 년** (Cloudflare는 레지스트리 원가에 팔아 첫해와 갱신 가격이 같다) |
| DNS | Cloudflare DNS. `A @ → VM 공인 IP`, **Proxy 끔(DNS only)** |
| TLS | VM의 Caddy가 Let's Encrypt에서 자동 발급·갱신(HTTP-01, 80번). HTTP는 HTTPS로 308 이동. |

- 프록시(주황 구름)를 끈 이유: 켜면 Cloudflare가 TLS를 끝내 Caddy가 직접 인증서를 받지 못하고, 무료 프록시의 업로드 크기 제한(100MB)에 녹화본 업로드(최대 2GB)가 걸린다.
- `deploy/Caddyfile`의 사이트 주소는 `{$SITE_ADDRESS::80}`이다. `.env`에 `SITE_ADDRESS=mins.work`를 두면 Caddy가 그 이름으로 인증서를 받는다.
- 설정 절차: [docs/deploy-oci.md > 도메인 연결](docs/deploy-oci.md#도메인-연결-과제-6)

### OCI VM

| 항목 | 값 |
|---|---|
| 리전 | Japan Central (Osaka, `ap-osaka-1`), 홈 리전 |
| Shape | `VM.Standard.A1.Flex` (Ampere, arm64) |
| OCPU / 메모리 | **2 OCPU / 12GB** |
| 부트 볼륨 | 50GB |
| 이미지 | Ubuntu 24.04 (aarch64) |
| 공인 IP | 임시(ephemeral) 공인 IP. 예약 IP는 쓰지 않는다. |

Always Free에 맞는 이유:

| 자원 | Always Free 한도(테넌시 합계) | 사용량 |
|---|---|---|
| A1 Compute | 4 OCPU, 24GB | 2 OCPU, 12GB |
| 블록 스토리지 | 200GB | 50GB (부트 볼륨) |
| 아웃바운드 전송 | 월 10TB | 시청 트래픽 |
| Object Storage | 20GB | 쓰지 않음(녹화본은 VM 디스크, 총량 15GiB에서 업로드를 막음) |

- 오토스케일러는 배포에서 띄우지 않는다(`compose.yaml`의 `autoscale` 프로필). app은 1개로 고정해 VM 자원 사용이 일정하다. 오토스케일러는 원래도 같은 VM 안의 컨테이너 수만 바꾸고 OCI 자원을 만들지 않는다.
- 2 OCPU에서 라이브 재인코딩은 방송당 OCPU 1개 안팎이라 동시 라이브를 2개(`MAX_LIVE_BROADCASTS` 기본값)로 둔다.
- 예산 알림(`free-tier-guard`, 월 US$1)을 걸어 두어 혹시 과금이 생기면 바로 메일이 온다.

### 데스크톱 클라이언트 직접 빌드

```bash
cargo install dioxus-cli --version 0.7.10 --locked
cargo test -p chzzk-desktop
cargo run -p chzzk-desktop                                  # 창으로 실행 (기본 서버 https://mins.work)
CHZZK_SERVER_URL=http://localhost:8088 cargo run -p chzzk-desktop   # 로컬 서버로 실행
cargo run -p chzzk-desktop -- --check https://mins.work     # 창 없이 목록·재생 주소 확인
dx bundle -p chzzk-desktop --desktop --release --package-types macos --package-types dmg   # macOS 패키지
dx bundle -p chzzk-desktop --desktop --release --package-types msi --package-types nsis    # Windows 패키지 (Windows에서)
```

### 설치·실행 확인 절차

다운로드 페이지(https://mins.work/download)에 같은 내용이 있다.

**Windows (x64)**

1. `chzzk-desktop-windows-x64-setup.exe`를 받아 실행한다. SmartScreen 경고가 뜨면 **추가 정보 → 실행**(서명하지 않은 앱).
2. 시작 메뉴에서 **ChzzkDesktop**을 연다. 왼쪽에 https://mins.work 의 방송 목록이 뜬다.
3. 방송을 누르면 오른쪽에서 재생된다(라이브는 음소거로 시작).
4. 창 없이 확인: PowerShell에서 `& "$env:LOCALAPPDATA\ChzzkDesktop\chzzk-desktop.exe" --check` → `서버 https://mins.work: 방송 N개`.

**macOS (Apple Silicon)**

1. `chzzk-desktop-macos-arm64.dmg`를 열고 **ChzzkDesktop**을 응용 프로그램 폴더로 끈다.
2. 서명·공증하지 않은 앱이라 `xattr -dr com.apple.quarantine /Applications/ChzzkDesktop.app` 후 연다(또는 시스템 설정 → 개인정보 보호 및 보안 → 그래도 열기).
3. 방송 목록이 뜨고, 방송을 누르면 재생된다.
4. 창 없이 확인: `/Applications/ChzzkDesktop.app/Contents/MacOS/chzzk-desktop --check`

### OS·아키텍처별 테스트 결과

TEST_RESULTS_PLACEHOLDER

## 과제 5 구현 범위(유지)

과제 4까지의 서비스를 **로컬에서 여러 인스턴스로 띄우고, CPU 사용률에 따라 인스턴스 수가 자동으로 늘고 줄게** 했다. 클라우드 배포 없이 Docker Compose 하나로 확인한다. 인스턴스가 늘거나 줄어도 로그인·영상·라이브·댓글·후원이 유지되도록 상태를 모두 인스턴스 밖 공용 저장소로 옮겼다.

| 요구 사항 | 구현 |
|---|---|
| 서버 컨테이너 이미지 | [`Dockerfile`](Dockerfile)(서버 + 빌드된 웹 + ffmpeg). 오토스케일러는 [`apps/autoscaler/Dockerfile`](apps/autoscaler/Dockerfile). |
| 여러 인스턴스 실행 | [`compose.yaml`](compose.yaml)의 `app` 서비스. `container_name`·호스트 포트가 없어 `--scale`로 여러 개를 띄운다. 인스턴스마다 코어 1개(`cpus: 1`)까지 쓴다. |
| 지표 기준 자동 조절 | 직접 만든 오토스케일러 [`apps/autoscaler`](apps/autoscaler/src/main.rs)가 10초마다 `docker stats`로 app 인스턴스들의 CPU 사용률을 재고 **HPA와 같은 식**(`ceil(현재 수 × 평균 CPU ÷ 목표)`, 허용 오차 10%)으로 원하는 수를 정해 `AUTOSCALER_MIN`(1)~`AUTOSCALER_MAX`(5) 안에서 맞춘다. 늘리기는 바로(`docker compose up --scale`), 줄이기는 60초 안정화 뒤 **라이브 송출을 받지 않는 인스턴스만** 골라 `docker stop`(정상 종료)한다. 판단 규칙은 [`policy.rs`](apps/autoscaler/src/policy.rs)(단위 테스트 10개). |
| 요청 분산 | [`deploy/Caddyfile`](deploy/Caddyfile): Caddy가 Docker DNS의 `app` A 레코드를 1초마다 다시 읽어(`dynamic a`) 떠 있는 인스턴스 모두에 `least_conn`으로 나눈다. 막 내려간 인스턴스로 연결이 실패하면 다른 인스턴스로 다시 보낸다(`lb_try_duration`). |
| 공용 상태 | **PostgreSQL**: 사용자·로그아웃 목록·방송·녹화본 정보·댓글·후원(SQLite에서 옮김). **Redis Pub/Sub**: 댓글·후원 실시간 알림, 방송 삭제·라이브 종료 신호를 모든 인스턴스에 전달([`bus.rs`](apps/server/src/bus.rs)). **공유 볼륨 `app-data`**: 녹화본과 라이브 HLS 조각(OCI Object Storage도 그대로 쓸 수 있음). **JWT**: 모든 인스턴스가 같은 `JWT_SECRET`을 쓴다. |
| 라이브 상태 유지 | 송출받는 인스턴스가 방송에 **하트비트**를 5초마다 남기고, 하트비트가 30초 넘게 끊긴 라이브만 다른 인스턴스가 종료·녹화본으로 정리한다(전에는 서버가 시작할 때 라이브를 모두 끝내 새 인스턴스가 뜨면 다른 인스턴스의 방송이 끝났다). 동시 라이브 수는 DB에서 서비스 전체로 센다. 인스턴스는 `/healthz`로 송출 수를 알리고, SIGTERM을 받으면 송출을 마무리해 녹화본까지 만든 뒤 내려간다. |
| 부하 발생 | [`deploy/load/k6.js`](deploy/load/k6.js)(k6). 방송 목록·상세·댓글 조회와 로그인(argon2 해시로 CPU를 씀)을 섞는다. `docker compose --profile load run --rm k6` |

```mermaid
flowchart LR
    U[브라우저 / k6] --> C[Caddy<br/>dynamic a + least_conn]
    C --> A1[app-1]
    C --> A2[app-2]
    C --> A3[app-N]
    A1 & A2 & A3 --> P[(PostgreSQL)]
    A1 & A2 & A3 <--> R[(Redis Pub/Sub)]
    A1 & A2 & A3 --> V[(공유 볼륨<br/>녹화본·HLS)]
    S[autoscaler] -- docker stats / healthz --> A1 & A2 & A3
    S -- compose up --scale / docker stop --> D[Docker]
```

**과제 1~4 기능(유지)**: 녹화방송, 휴대폰 라이브 송출·시청, 회원가입·로그인·로그아웃, 소유자 권한, 댓글과 실시간 반영, 토스페이먼츠 테스트 결제 후원. 기능은 그대로이고 저장 위치만 바뀌었다(SQLite → PostgreSQL). 과제 4 이전의 SQLite 데이터는 옮기지 않으므로 새 DB로 시작한다.

**이번 단계에서 하지 않는 것**: 클라우드 배포·OCI 재배포, Kubernetes, 요청 수 기반 스케일링, Postgres·Redis 자체의 다중화, 송출 연결을 다른 인스턴스로 옮기기(송출받던 인스턴스가 죽으면 그 방송은 종료로 정리된다), SQLite→Postgres 데이터 이전 도구.

## 과제 4 구현 범위(유지)

과제 3까지의 녹화방송·휴대폰 라이브 송출·인증·인가·댓글에 **방송자를 후원하는 도네이션**을 더했다. 결제는 **토스페이먼츠 테스트 모드**(결제위젯)로만 하며 실제 돈은 오가지 않는다. 화면의 성공 표시가 아니라 **서버가 결제 승인 API로 확인한 결과로만 후원을 확정**하고, 같은 승인 통지가 여러 번 와도 **한 번만 적립**한다.

| 기능 | 구현 |
|---|---|
| 후원 대상·금액 선택 | 라이브 중인 방송 상세의 **후원하기**에서 로그인한 시청자가 금액(1,000·5,000·10,000원 버튼 또는 1,000~1,000,000원 직접 입력)과 메시지(100자까지)를 고른다. 받는 사람은 그 방송의 소유자이며, 클라이언트가 아니라 **서버가 주문을 만들 때 정한다**. 비로그인 401, 자기 방송 400, 끝난 방송 409. |
| 테스트 결제 요청 | 서버가 `결제 대기` 주문(주문번호, 금액, 받는 사람)을 만들면, 웹이 토스페이먼츠 결제위젯으로 결제 수단을 보여 주고 결제를 요청한다. 결제 인증이 끝나면 성공 주소(`/donations/success`), 실패·취소면 실패 주소(`/donations/fail`)로 돌아온다. |
| 승인 확인 후 확정 | 성공 주소의 결제 키·주문번호·금액을 서버에 보내면, 서버는 ① 주문한 사용자인지 ② 요청 금액이 **저장된 주문 금액**과 같은지 ③ 방송이 아직 라이브인지 확인한 뒤 토스페이먼츠 **결제 승인 API**를 직접 부르고, 응답이 `DONE`이며 **주문번호·총액이 주문과 같을 때만** 확정한다. 금액이 다르면 승인 API를 부르지 않고 실패, 방송이 끝났으면 승인하지 않고 취소(돈이 나가지 않음). |
| 실패·취소 | 결제창을 닫거나 결제가 실패하면 실패 화면이 서버에 알려 주문을 `취소`·`실패`로 끝낸다. 확정된 후원은 바뀌지 않는다. |
| 승인 통지(웹훅)와 중복 방지 | 토스페이먼츠 결제 상태 웹훅을 받되 **본문은 믿지 않고** 결제 조회 API로 다시 확인해 같은 확정 경로를 탄다(사용자가 성공 화면으로 돌아오지 못한 경우에도 확정). 확정은 `status = 'pending'`인 주문만 바꾸는 **조건부 갱신**이고 `payment_key`는 **유일**해서, 같은 승인 확인 재요청·같은 웹훅 반복·승인 확인과 웹훅이 둘 다 와도 한 건만 적립된다. |
| 후원 내역·합계 | 방송 상세에 그 방송의 확정 후원 합계·건수와 내역(후원자, 금액, 메시지, 시각)을 최신순으로 보여 준다. 로그인 없이 볼 수 있고 끝난 방송에서도 보인다. |
| 실시간 후원 알림 | 후원이 새로 확정되면 방송 알림 WebSocket(댓글과 같은 연결)으로 한 번만 알려, 시청 화면의 합계·내역이 바로 바뀌고 알림이 잠시 강조된다. |

```mermaid
sequenceDiagram
    participant V as 시청자(웹)
    participant S as 서버
    participant T as 토스페이먼츠(테스트)
    V->>S: 후원 주문(금액, 메시지)
    S-->>V: 주문번호·금액·받는 사람(방송 소유자) — 결제 대기
    V->>T: 결제위젯으로 결제 요청
    T-->>V: 성공 주소(paymentKey, orderId, amount)
    V->>S: 승인 확인 요청
    S->>S: 주문 금액·주문자·방송 상태 대조
    S->>T: 결제 승인 API
    T-->>S: status DONE, orderId, totalAmount
    S->>S: 대조 후 조건부 갱신으로 확정(한 번만)
    S-->>V: 확정된 후원
    S-)V: 실시간 알림(다른 시청자 화면 포함)
    T-)S: 웹훅(결제 상태 변경) — 결제 조회 API로 다시 확인, 이미 확정이면 무시
```

후원 확정 코드는 `apps/server/src/donations.rs`의 `confirm`·`finalize`·`verify`·`webhook`, 중복 방지 저장 구조는 `apps/server/migrations/0001_initial_schema.sql`의 `donations`와 `apps/server/src/db.rs`의 `confirm_donation`에 있다.

**과제 1~3 기능(유지)**: 녹화방송 등록·목록·상세·재생, 휴대폰 라이브 송출·시청·방송 상태·종료 후 녹화본, 회원가입·로그인·로그아웃(JWT), 소유자 권한(제목 수정·삭제·라이브 종료), 댓글과 실시간 반영. 달라진 점은 **확정된 후원이 있는 방송은 후원 내역을 지키기 위해 삭제할 수 없다**(409)는 것뿐이다.

**이번 단계에서 하지 않는 것**: 실제 결제(라이브 키), 정산·출금, 환불·결제 취소 API, 가상계좌·계좌이체 입금 대기, 후원 랭킹·목표, 후원 메시지 TTS·방송 화면 오버레이, 시청자 수, 데스크톱 송출 앱.

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
| 후원 내역·합계 보기, 실시간 후원 알림 | ✅ | ✅ | ✅ | ✅ |
| 후원(주문·결제 승인 확인·실패 통지) | ❌ 401 | ✅ (라이브 중인 남의 방송, 자기 주문만) | ❌ 400 (자기 방송) | ✅ |

서버의 판정 순서: 로그인하지 않았거나 토큰이 잘못·만료·폐기되었으면 먼저 **401**, 없는 방송·댓글이면 **404**, 소유자·작성자가 아니면 **403**, 상태가 맞지 않으면(라이브 중 삭제, 이미 끝난 방송 종료) **409**. 방송 권한 확인은 `apps/server/src/api.rs`의 `find_owned`, 댓글 삭제 권한은 `apps/server/src/comments.rs`의 `delete_comment`, 후원 주문의 주인 확인은 `apps/server/src/donations.rs`의 `find_own_order`에 있다. 토스페이먼츠 웹훅은 로그인 없이 받지만 본문 대신 결제 조회 결과만 믿는다.

**과제 1·2 기능(유지)**: 녹화방송 등록(제목 + mp4·webm 업로드)·목록·상세·재생(Range 탐색)·공유 URL·등록 검증(400·413·415·507)·local/OCI 저장소, 휴대폰 카메라·마이크 라이브 송출(WebSocket → ffmpeg → HLS)·라이브 목록과 시청·방송 상태(`라이브 중`/`종료`)·종료 후 녹화본·끊김과 재시작 처리. 달라진 점은 **등록과 송출에 로그인이 필요**하다는 것뿐이다.

> 과제 3으로 올리면 소유자가 없는 이전 방송(과제 1·2에서 만든 방송)은 마이그레이션에서 녹화본과 함께 지워진다. 소유자가 없으면 아무도 관리할 수 없기 때문이다. 녹화본 파일은 서버가 시작할 때 저장소에서 지운다.

**이번 단계에서 하지 않는 것**: 소셜 로그인, 이메일 인증, 비밀번호 찾기·변경, 회원 탈퇴, 리프레시 토큰, 관리자 역할, 댓글 수정·답글·신고, 도네이션, 시청자 수, 데스크톱 송출 앱, 방송 소유권 이전, 페이지네이션·검색.

## 구성

Cargo workspace 모노레포.

| 경로 | 설명 |
|---|---|
| `apps/server` | axum API 서버. PostgreSQL(sqlx)에 사용자·방송·댓글·후원을 저장한다. `src/auth.rs`가 회원가입·로그인·로그아웃과 토큰 검증(`AuthUser`), `src/api.rs`가 방송 API와 소유자 권한 확인(`find_owned`), `src/comments.rs`가 댓글 API와 방송 알림 WebSocket, `src/donations.rs`가 후원 주문·승인 확인·실패 통지·웹훅·내역, `src/payments.rs`가 토스페이먼츠 결제 승인·조회 API, `src/live.rs`가 송출 WebSocket·ffmpeg HLS 변환·라이브 재생·종료 후 녹화본·하트비트, `src/bus.rs`가 Redis Pub/Sub로 인스턴스 사이의 알림 전달을 맡는다. |
| `apps/web` | Dioxus 0.7 웹 앱. 방송 목록·상세(녹화본/라이브 플레이어, 소유자 관리, 댓글)·등록·라이브 송출·로그인·회원가입 화면. `src/session.rs`가 로그인 상태, `src/comments.rs`가 댓글 영역과 실시간 연결, `src/donations.rs`가 후원 폼·토스페이먼츠 결제위젯·결제 결과 화면·후원 내역, `src/live.rs`가 카메라·마이크와 송출을 맡는다. |
| `apps/desktop` | Dioxus 데스크톱 시청 클라이언트(과제 6, Windows·macOS). `src/api.rs`가 서버 주소·방송 목록·재생 방식 판단, `src/main.rs`가 화면과 HLS 플레이어, `Dioxus.toml`이 패키지 설정. |
| `apps/autoscaler` | 직접 만든 오토스케일러(과제 5, 배포에서는 띄우지 않음). docker CLI로 app 인스턴스의 CPU 사용률을 재고 `src/policy.rs`의 규칙으로 인스턴스 수를 바꾼다. |
| `crates/shared` | 서버·웹·데스크톱이 함께 쓰는 API 타입, 송출·댓글 WebSocket 메시지. |
| `.github/workflows/desktop.yml` | 데스크톱 클라이언트 Windows·macOS 패키지 빌드와 GitHub Release. |
| `compose.yaml`, `deploy/` | 배포·로컬 구성(PostgreSQL, Redis, app, Caddy; `autoscale` 프로필의 오토스케일러, `load` 프로필의 k6 부하). `deploy/Caddyfile`이 분산, `deploy/load/k6.js`가 부하 스크립트. |

## 준비물

- Rust (stable) 와 `wasm32-unknown-unknown` 타깃: `rustup target add wasm32-unknown-unknown`
- Dioxus CLI 0.7: `cargo install dioxus-cli --version ^0.7 --locked`
- Docker (PostgreSQL·Redis와 과제 5의 오토스케일 구성용)
- ffmpeg (라이브 송출용): macOS `brew install ffmpeg`, Ubuntu `sudo apt-get install ffmpeg`
  - 없어도 서버는 시작하며 녹화방송 기능은 그대로 동작한다. 송출만 거부된다.

## 로컬 실행

PostgreSQL 하나와 터미널 두 개를 쓴다. 아래 개발용 DB 비밀번호(`postgres`)는 로컬에서만 쓰는 값이다.

```bash
# 개발·테스트용 PostgreSQL (한 번만)
docker run -d --name chzzk-dev-pg -e POSTGRES_PASSWORD=postgres -p 5432:5432 postgres:17
```

```bash
# 터미널 1: API 서버 (http://localhost:3000)
export DATABASE_URL=postgres://postgres:postgres@localhost:5432/postgres
cargo run -p server
```

아래의 `cargo run -p server` 예시는 모두 터미널 1에서 이렇게 `DATABASE_URL`을 내보낸 상태를 가정한다.

```bash
# 터미널 2: 웹 (http://localhost:8080)
dx serve -p web
```

브라우저에서 http://localhost:8080 을 연다. DB는 PostgreSQL에, 녹화본과 라이브 HLS 조각은 기본적으로 `data/` 아래에 저장된다. 이 상태에서 같은 PC의 브라우저는 웹캠으로 송출할 수 있다(localhost는 브라우저가 보안 출처로 취급한다).

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

### 후원 결제용 토스페이먼츠 테스트 키

후원을 쓰려면 토스페이먼츠 **테스트 키**(`test_`로 시작)를 서버 환경변수로 준다. 키가 없으면 서버는 그대로 시작하고 후원만 "결제 서비스가 설정되지 않아…"(503)로 막힌다. 라이브 키(`live_`)는 쓰지 않는다.

- 가입 없이: 토스페이먼츠 문서의 [결제위젯 연동 가이드](https://docs.tosspayments.com/guides/v2/payment-widget/integration)와 [샘플 프로젝트](https://github.com/tosspayments/tosspayments-sample)에 공개된 **결제위젯 문서용 테스트 키**(클라이언트 키 `test_gck_docs_…`, 시크릿 키 `test_gsk_docs_…`)를 쓴다.
- 가입한 경우: [개발자센터 > API 키](https://developers.tosspayments.com/my/api-keys)의 **결제위젯 연동 키** 테스트 키를 쓴다.

```bash
TOSS_CLIENT_KEY=<테스트 클라이언트 키> TOSS_SECRET_KEY=<테스트 시크릿 키> JWT_SECRET="$(openssl rand -base64 48)" cargo run -p server
```

## 로컬 오토스케일 실행 (과제 5)

Docker만 있으면 된다. 이 구성이 서버를 여러 인스턴스로 띄우고 CPU 사용률에 따라 늘리고 줄인다.

```bash
cp .env.example .env
# .env에서 아래 두 값을 채운다(필수, 커밋 금지). 나머지는 비워 두면 기본값이다.
#   POSTGRES_PASSWORD=$(openssl rand -hex 24) 결과
#   JWT_SECRET=$(openssl rand -base64 48) 결과
# 오토스케일러는 autoscale 프로필에 있어 프로필을 켜야 뜬다(배포에서는 띄우지 않는다).
docker compose --profile autoscale up -d --build
```

- 첫 빌드는 웹(Dioxus)·서버를 컴파일하느라 10분 이상 걸린다.
- 브라우저에서 http://localhost 를 연다. 80번 포트를 다른 프로그램이 쓰고 있으면 `.env`에 `HTTP_PORT=8088`을 두고 http://localhost:8088 을 연다.
- 처음에는 `app` 인스턴스가 `AUTOSCALER_MIN`(기본 1)개 뜬다.

인스턴스 수와 스케일러의 판단은 다음으로 본다.

```bash
docker compose ps app
docker compose logs -f autoscaler
```

부하를 걸면(기본 40 가상 사용자, 3분) 인스턴스가 늘고, 부하가 끝나고 안정화 시간(기본 60초)이 지나면 줄어든다.

```bash
docker compose --profile load run --rm k6
# 세기·시간 조절: LOAD_VUS=80 LOAD_DURATION=5m docker compose --profile load run --rm k6
```

정리: `docker compose down`(데이터 유지) 또는 `docker compose down -v`(DB·녹화본 볼륨까지 삭제).

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

### 과제 5: 스케일 아웃·인과 상태 유지

1. [로컬 오토스케일 실행](#로컬-오토스케일-실행-과제-5)대로 띄운다. `docker compose ps app`에 `chzzk-app-1` 하나만 보인다.
2. 브라우저(http://localhost)에서 가입·로그인하고, 녹화방송을 하나 등록하고 댓글을 하나 쓴다.
3. 다른 터미널에서 `docker compose logs -f autoscaler`를 열어 두고 부하를 건다: `docker compose --profile load run --rm k6`.
4. **스케일 아웃**: 10~20초 안에 로그에 `스케일 아웃: 1 → 2`, `2 → 5`가 나오고, `docker compose ps app`에 인스턴스가 늘어 있다.
5. **스케일 중 상태 유지**: 그동안 브라우저를 새로고침한다. 로그인이 풀리지 않고 방송 목록·댓글이 그대로다. 요청이 여러 인스턴스로 나뉘어도 같은 DB·Redis·공유 볼륨을 보기 때문이다. 두 브라우저에서 같은 방송을 열고 댓글을 쓰면 다른 인스턴스에 붙은 쪽에도 바로 나타난다.
6. **스케일 인**: k6가 끝나고 60초(안정화 시간)쯤 지나면 `스케일 인: chzzk-app-5 정상 종료 …`가 이어지고 1개로 돌아온다. 로그인·목록·댓글은 그대로다.
7. (선택) 라이브 송출 중에는 그 방송을 받는 인스턴스를 줄이지 않는다. 휴대폰이나 같은 PC 브라우저로 송출을 켜 둔 채 3~6을 하면, 송출을 받는 인스턴스는 남고 나머지만 줄어든다(`줄일 수 있는 인스턴스가 N개뿐입니다` 로그). 다른 인스턴스로 연 시청 화면에서도 라이브가 재생된다.

#### 확인한 기록 (2026-11-18, MacBook Docker Desktop 10코어, 기본 설정)

k6 40 가상 사용자 3분. 5초마다 센 app 인스턴스 수와 30초마다 확인한 상태(로그인한 토큰으로 `/api/auth/me`, 방송 목록, 부하 전에 쓴 댓글):

```text
09:53:53 app=1            ← k6 시작
09:54:03 app=2
09:54:13 app=5
로그인 상태 200 (scaletester), 방송 목록 200 (1개), 댓글 200 (['스케일 전에 쓴 댓글'])
…  (09:54:13 ~ 09:57:59 app=5 유지, 30초마다 같은 확인 결과)
09:58:04 app=1            ← k6 종료(약 09:56:55) 뒤 부하가 사라진 것을 본 지 60초(안정화) 뒤
로그인 상태 200 (scaletester), 방송 목록 200 (1개), 댓글 200 (['스케일 전에 쓴 댓글'])
```

오토스케일러 로그(`docker compose logs autoscaler`, 시각은 UTC):

```text
[autoscaler 00:53:46Z] 인스턴스 1개 CPU [6%] 평균 6.4% (목표 50%) → 원하는 수 1, 맞출 수 1
[autoscaler 00:53:58Z] 인스턴스 1개 CPU [100%] 평균 99.8% (목표 50%) → 원하는 수 2, 맞출 수 2
[autoscaler 00:53:58Z] 스케일 아웃: 1 → 2
[autoscaler 00:54:11Z] 인스턴스 2개 CPU [103% 100%] 평균 101.2% (목표 50%) → 원하는 수 5, 맞출 수 5
[autoscaler 00:54:11Z] 스케일 아웃: 2 → 5
[autoscaler 00:54:24Z] 인스턴스 5개 CPU [102% 98% 100% 98% 100%] 평균 99.3% (목표 50%) → 원하는 수 5, 맞출 수 5
…
[autoscaler 00:57:12Z] 인스턴스 5개 CPU [0% 0% 0% 0% 0%] 평균 0.0% (목표 50%) → 원하는 수 1, 맞출 수 5
…  (안정화 시간 동안 5 유지)
[autoscaler 00:57:59Z] 인스턴스 5개 CPU [0% 0% 0% 0% 0%] 평균 0.0% (목표 50%) → 원하는 수 1, 맞출 수 1
[autoscaler 00:58:00Z] 스케일 인: chzzk-app-5 정상 종료 (5개 → 4개)
[autoscaler 00:58:00Z] 스케일 인: chzzk-app-4 정상 종료 (4개 → 3개)
[autoscaler 00:58:00Z] 스케일 인: chzzk-app-3 정상 종료 (3개 → 2개)
[autoscaler 00:58:00Z] 스케일 인: chzzk-app-2 정상 종료 (2개 → 1개)
[autoscaler 00:58:11Z] 인스턴스 1개 CPU [0%] 평균 0.0% (목표 50%) → 원하는 수 1, 맞출 수 1
```

k6 결과: 요청 78,547건, `http_req_failed 0.00%`, 검사(목록·상세·댓글·로그인·내 정보 200) 78,546건 모두 통과. 스케일 아웃·인 중에도 실패한 요청이 없었다. 다시 90초 부하를 걸었을 때도 `1 → 3 → 5`로 늘었다가 `5 → 1`로 줄었고 요청 32,757건이 모두 성공했다.

### 과제 4: 도네이션 (테스트 결제 성공·실패·취소, 승인 통지 중복)

**준비**

1. 위 [후원 결제용 토스페이먼츠 테스트 키](#후원-결제용-토스페이먼츠-테스트-키)를 넣고 서버를 띄운다. 서버 로그에 `TOSS_CLIENT_KEY·TOSS_SECRET_KEY가 없어` 경고가 없어야 한다.
2. 사용자 둘을 준비한다. **A**(방송자)는 [과제 2](#과제-2-휴대폰-송출--다른-화면에서-시청-유지) 절차로 라이브 송출을 시작하고, **B**(시청자)는 다른 브라우저(또는 시크릿 창)에서 로그인해 그 방송의 상세를 연다.
3. 아래 API 확인에 쓸 값을 준비한다(과제 3의 `check` 함수와 같은 터미널).

```bash
API=http://localhost:3000
JSON='content-type: application/json'
TOKEN_B=$(curl -s -X POST $API/api/auth/login -H "$JSON" -d '{"username":"user_b","password":"password-b1"}' | jq -r .token)
LIVE=<A가 송출 중인 방송 ID(상세 주소의 마지막 부분)>
```

**1. 테스트 결제 성공**

1. B의 상세 화면 **후원하기**에서 금액(예: **5,000원**)과 메시지를 고르고 **다음: 결제 수단 고르기**를 누른다. 받는 사람에 A의 아이디가 보인다.
2. 결제위젯 위에 "테스트 환경이에요. 실제로 결제되지 않아요." 안내가 보인다. 결제 수단을 고르고 **5,000원 결제하기** → 결제창 안내대로 인증을 마친다(테스트 모드라 실제 출금은 없다).
3. 성공 화면에 "결제를 확인하고 있습니다…"가 잠깐 보인 뒤 **"후원이 완료되었습니다."**와 받는 사람·금액이 나온다. 이 문구는 서버가 결제 승인 API로 확인한 뒤에만 나온다.
4. A의 화면(그리고 같은 방송을 보던 다른 화면)에 새로 고치지 않아도 "🎉 user_b 님이 5,000원 후원했습니다!"와 함께 **후원 1건 · 합계 5,000원**이 보인다.
5. 서버 로그에 `후원 주문: user_b → user_a 5000원`, `후원 확정: user_b → user_a 5000원`이 찍힌다.
6. 성공 화면 주소의 `paymentKey=` 값을 복사해 둔다(아래 5에서 쓴다).

**2. 테스트 결제 취소**

1. B가 다시 후원하기 → 결제 수단 고르기 → 결제하기로 결제창을 연 뒤, 결제창의 **닫기(X)**를 누른다.
2. 후원 영역에 **"결제를 취소했습니다. 후원은 이루어지지 않았습니다."**가 나오고, 후원 건수·합계는 그대로다. (휴대폰처럼 결제창이 페이지를 옮기는 환경에서는 실패 화면 `/donations/fail`로 이동해 같은 문구가 나온다.)
3. 서버 로그에 `후원 주문 canceled: 사용자가 결제를 취소했습니다.`가 찍힌다.

**3. 테스트 결제 실패**

- 결제창에서 카드 인증이 실패하면 실패 화면에 "결제에 실패했습니다: <토스페이먼츠 안내>"가 나오고 후원은 생기지 않는다.
- 서버 검증 실패(브라우저에서 금액을 조작한 경우): 주문을 5,000원으로 만들고 승인 확인을 100원으로 보낸다. 서버는 **결제 승인 API를 부르지 않고** 주문을 실패로 끝낸다.

```bash
ORDER=$(curl -s -X POST $API/api/broadcasts/$LIVE/donations -H "Authorization: Bearer $TOKEN_B" -H "$JSON" -d '{"amount":5000,"message":"금액 조작 확인"}' | jq -r .order_id)
curl -s -X POST $API/api/donations/confirm -H "Authorization: Bearer $TOKEN_B" -H "$JSON" -d "{\"payment_key\":\"tgen_fake\",\"order_id\":\"$ORDER\",\"amount\":100}"; echo
curl -s $API/api/broadcasts/$LIVE/donations | jq '{count, total_amount}'
```

기대 출력: `{"message":"결제 금액이 주문 금액과 달라 결제를 승인하지 않았습니다."}`, 그리고 건수·합계는 1의 결과(1건, 5,000원) 그대로.

**4. 결제 중에 방송이 끝난 경우**

1. B가 결제창을 연 상태에서 A가 **방송 종료**(또는 상세의 **라이브 종료**)를 누른다.
2. B가 결제를 마치면 성공 화면에 **"방송이 끝나 결제를 승인하지 않았습니다. 돈은 나가지 않습니다."**가 나온다. 서버가 승인 API를 부르지 않아 결제 인증은 만료된다.

**5. 승인 통지 중복**

같은 결제에 대한 통지가 여러 번 와도 후원은 한 번만 적립되는지 확인한다.

1. **같은 승인 확인 재요청**: 1에서 열린 성공 화면을 여러 번 새로 고친다 → 매번 같은 "후원이 완료되었습니다."가 나오고 A 화면의 건수·합계는 그대로다(서버는 결제 승인 API도 다시 부르지 않는다).
2. **같은 웹훅 반복**: 1에서 복사한 결제 키로 토스페이먼츠와 같은 형식의 웹훅을 세 번 보낸다. 서버는 본문을 믿지 않고 결제 조회 API로 확인한 뒤, 이미 확정된 후원이라 다시 적립하지 않는다.

```bash
PAYMENT_KEY=<성공 화면 주소의 paymentKey 값>
for i in 1 2 3; do
  curl -s -X POST $API/api/payments/toss/webhook -H "$JSON" \
    -d "{\"eventType\":\"PAYMENT_STATUS_CHANGED\",\"data\":{\"paymentKey\":\"$PAYMENT_KEY\",\"status\":\"DONE\"}}"; echo
done
curl -s $API/api/broadcasts/$LIVE/donations | jq '{count, total_amount}'
```

기대 출력: `{"result":"already_processed"}` 세 줄, 그리고 건수·합계 그대로.

3. **실제 토스페이먼츠 웹훅**(선택): 웹훅은 공개 HTTPS 주소로만 온다. [휴대폰 연결 방법 1](#방법-1-로컬-pc--임시-https-터널-계정-불필요)의 터널 주소로 `https://<터널 주소>/api/payments/toss/webhook`을 [개발자센터 > 웹훅](https://developers.tosspayments.com/my/webhook)에 `PAYMENT_STATUS_CHANGED`로 등록하고 결제한다. 웹훅 기록의 **다시 시도**로 같은 통지를 다시 보내도 건수·합계는 그대로다. (문서용 공개 테스트 키는 내 개발자센터에 웹훅을 등록할 수 없으므로, 이 단계는 가입해 받은 테스트 키로 한다.)
4. **자동 테스트**: `cargo test -p server --test donations`로 웹훅만으로 확정, 같은 웹훅 반복, 승인 확인↔웹훅 순서 바꿈, 동시 승인 확인을 가짜 결제 서버로 확인한다.

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
| `DATABASE_URL` | — (필수) | PostgreSQL 연결 주소. 예: `postgres://postgres:postgres@localhost:5432/postgres` |
| `DATABASE_MAX_CONNECTIONS` | `10` | 인스턴스 하나가 여는 DB 연결 수의 상한 |
| `REDIS_URL` | — | 인스턴스 사이에 댓글·후원 알림과 송출 종료 신호를 전하는 Redis. 없으면 인스턴스 1개로 보고 프로세스 안에서만 전한다 |
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
| `MAX_LIVE_BROADCASTS` | `2` | 서비스 전체(모든 인스턴스 합)에서 동시에 진행할 수 있는 라이브 방송 수(송출마다 ffmpeg가 CPU로 인코딩) |
| `INSTANCE_ID` | 호스트 이름 | 서버 인스턴스 이름. 컨테이너에서는 컨테이너 ID가 된다 |
| `LIVE_HEARTBEAT_INTERVAL_SECS` | `5` | 송출 중인 인스턴스가 하트비트를 갱신하고, 끊긴 라이브를 찾는 주기 |
| `LIVE_HEARTBEAT_TIMEOUT_SECS` | `30` | 하트비트가 이만큼 끊긴 라이브 방송은 송출받던 인스턴스가 죽은 것으로 보고 종료 |
| `LIVE_IDLE_TIMEOUT_SECS` | `20` | 송출 중 영상이 이 시간 동안 오지 않으면 방송 종료 |
| `JWT_SECRET` | (없음, 임시 키) | 로그인 토큰(JWT) 서명 키. 실제 값은 실행 환경에만 둔다. 예: `openssl rand -base64 48`로 만든 값. 비우면 시작할 때마다 임시 키를 만든다(재시작하면 모두 로그아웃) |
| `JWT_TTL_SECS` | `86400` (24시간) | 로그인 토큰 유효 시간 |
| `TOSS_CLIENT_KEY` | (없음) | 토스페이먼츠 결제위젯 테스트 클라이언트 키(`test_gck_…`). 웹에 내려 준다. 비우면 후원만 503 |
| `TOSS_SECRET_KEY` | (없음) | 토스페이먼츠 테스트 시크릿 키(`test_gsk_…`). 서버만 쓴다. 실제 값은 실행 환경에만 둔다 |
| `TOSS_API_BASE` | `https://api.tosspayments.com` | 결제 승인·조회 API 주소. 테스트에서 가짜 결제 서버를 가리킬 때만 바꾼다 |
| `CORS_ALLOWED_ORIGINS` | `http://localhost:8080` | 허용할 웹 출처(쉼표 구분, `*`는 전체) |
| `WEB_DIST_DIR` | (없음) | 빌드된 웹 정적 파일 디렉터리. 지정하면 서버가 웹 화면도 서빙한다(배포·터널용) |
| `SITE_ADDRESS` | (없음, HTTP :80) | (Docker Compose의 Caddy) HTTPS 주소. 예: `158-101-1-2.sslip.io` |
| `POSTGRES_PASSWORD` | — (compose 필수) | (Docker Compose) PostgreSQL 비밀번호. compose가 `DATABASE_URL`을 만들어 app에 넣는다 |
| `HTTP_PORT` / `HTTPS_PORT` | `80` / `443` | (Docker Compose) Caddy가 호스트에 여는 포트 |
| `APP_CPUS` | `1` | (Docker Compose) app 인스턴스 하나가 쓸 수 있는 CPU 코어 수. 스케일러의 CPU 100%가 이 값 기준이다 |
| `AUTOSCALER_MIN` / `AUTOSCALER_MAX` | `1` / `5` | (오토스케일러) app 인스턴스 수의 최소·최대 |
| `AUTOSCALER_TARGET_CPU` | `50` | (오토스케일러) 인스턴스 하나의 목표 CPU 사용률(%, 코어 1개 = 100) |
| `AUTOSCALER_INTERVAL_SECS` | `10` | (오토스케일러) CPU를 재고 판단하는 주기 |
| `AUTOSCALER_SCALE_DOWN_STABILIZATION_SECS` | `60` | (오토스케일러) 줄이기 전에 기다리는 시간. 이 시간 동안의 원하는 수 중 가장 큰 값까지만 줄인다 |
| `LOAD_VUS` / `LOAD_DURATION` | `40` / `3m` | (k6 부하) 가상 사용자 수와 시간 |
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
| `POST /api/broadcasts/{id}/donations` | 로그인 | 후원 주문 `{"amount","message"}`. 라이브 중인 남의 방송만(끝난 방송 409, 자기 방송 400, 금액 1,000~1,000,000원·메시지 100자 위반 400, 키 없음 503). 201 `{"order_id","amount","order_name","recipient","client_key","customer_key"}` |
| `GET /api/broadcasts/{id}/donations` | 공개 | 후원 내역과 합계 `{"total_amount","count","donations":[...]}`(확정 후원만, 최신순 최근 100건) |
| `POST /api/donations/confirm` | 주문자 | 결제 승인 확인 `{"payment_key","order_id","amount"}`. 금액 불일치 400(승인 API 호출 안 함), 방송 종료 409(승인 안 함), 승인 거절 400, 이미 확정된 같은 결제면 같은 후원을 그대로 돌려줌. 200 |
| `POST /api/donations/fail` | 주문자 | 결제 실패·취소 통지 `{"order_id","code","message"}`. 결제 대기 주문만 끝낸다(확정된 후원 409). 204 |
| `POST /api/payments/toss/webhook` | 공개(토스페이먼츠) | 결제 상태 웹훅. 결제 조회 API로 다시 확인해 확정·실패·취소. `{"result":"confirmed"\|"already_processed"\|"closed"\|"ignored"}` 200, 결제 서비스 오류는 502 |

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

### 댓글(방송 알림) WebSocket 메시지

읽기 전용이다(`crates/shared`의 `CommentEvent`). 연결 전에 쓰인 댓글은 보내지 않으므로, 연결한 뒤 댓글 목록 API로 채운다. 방송이 삭제되면 서버가 연결을 닫는다.

- `{"type":"created","comment":{"id","broadcast_id","author":{"id","username"},"body","created_at"}}`
- `{"type":"deleted","comment_id":"..."}`
- `{"type":"donated","donation":{"order_id","broadcast_id","donor","recipient","amount","message","confirmed_at"}}` — 후원이 새로 확정될 때 한 번

## 배포

OCI(오사카 리전) Always Free 범위 안에서 A1 VM 1대 + 비공개 Object Storage 버킷으로 배포한다. 콘솔에서 자원을 만드는 순서, sslip.io HTTPS 설정, 과금 확인 포인트는 [docs/deploy-oci.md](docs/deploy-oci.md)에 있다.

VM에서도 [로컬 오토스케일 실행](#로컬-오토스케일-실행-과제-5)과 같은 `compose.yaml`(PostgreSQL, Redis, 앱 인스턴스들, Caddy, 오토스케일러)을 띄운다. 앱 인스턴스 하나가 API, 송출 WebSocket, 빌드된 웹 화면을 같은 주소로 서빙한다. 과제 5는 클라우드 배포를 요구하지 않아 OCI에는 다시 배포하지 않았다.

```bash
cp .env.example .env   # POSTGRES_PASSWORD, JWT_SECRET, STORAGE_BACKEND=oci, OCI_* 값, SITE_ADDRESS, TOSS_* 테스트 키, AUTOSCALER_MAX=2를 채운다(커밋 금지)
docker compose up -d --build
```

## 테스트

```bash
# 테스트용 PostgreSQL과 Redis (로컬 실행의 chzzk-dev-pg를 그대로 써도 된다)
docker run -d --name chzzk-test-pg -e POSTGRES_PASSWORD=postgres -p 5432:5432 postgres:17
docker run -d --name chzzk-test-redis -p 6379:6379 redis:7
cargo test -p server -p autoscaler
```

테스트는 `TEST_DATABASE_URL`(기본 `postgres://postgres:postgres@localhost:5432/postgres`)의 PostgreSQL 서버에 테스트마다 새 데이터베이스를 만들고 끝나면 지운다. 여러 인스턴스 테스트는 `TEST_REDIS_URL`(기본 `redis://localhost:6379`)을 쓴다.

서버 통합 테스트 105개와 단위 테스트 1개, 오토스케일러 단위 테스트 10개. 테스트마다 새 DB, 임시 저장소, 임시 라이브 디렉터리를 쓰며, 테스트용 서명 키(`test-only-jwt-secret`)와 결제 키(`test_sk_fake_…`)는 실제 비밀 값이 아니다. 송출 테스트는 ffmpeg가 필요하다.

- `tests/cluster.rs`(9개, 과제 5): 같은 DB·Redis·공유 디렉터리를 쓰는 **서버 인스턴스 두 개(A·B)**를 실제 포트에 띄운다. A에서 받은 토큰을 B가 받아들이고 A의 로그아웃이 B에도 적용, A로 쓴 댓글·삭제를 B의 WebSocket이 받음, 방송 삭제 시 B의 댓글 연결도 닫힘, A가 송출받는 라이브를 B에서 보고(HLS) 종료하면 송출이 끝나고 녹화본이 남음, 새 인스턴스가 떠도 다른 인스턴스의 라이브는 유지, 하트비트가 끊긴 라이브는 다른 인스턴스가 종료·녹화본 정리, 송출 중에는 하트비트가 갱신되어 정리되지 않음, 동시 라이브 수를 A·B 합으로 셈, `/healthz`와 정상 종료(송출을 이유와 함께 끝내고 녹화본까지 만든 뒤 503)
- `apps/autoscaler`(10개): 목표 대비 늘리기·줄이기, 허용 오차, 최소·최대, 인스턴스 0개, 줄이기 안정화 시간, 송출 중·응답 없는·내려가는 인스턴스는 줄이지 않음, 나중에 생긴 인스턴스부터 줄임
- `tests/donations.rs`(30개): 토스페이먼츠 대신 같은 응답 형식의 **가짜 결제 서버**를 실제 포트에 띄운다(승인 요청 수, 시크릿 키 Basic 인증까지 확인). 주문 거절(401·400·409·404·503)과 받는 사람, 승인 성공, 같은 승인 확인 재요청·동시 요청이 한 건, 금액 조작은 승인 API를 부르지 않고 실패, 승인 거절·응답 금액 불일치·다른 주문의 결제 키 거절, 남의 주문 403, 실패·취소 통지, 확정된 후원은 실패로 바뀌지 않음, 결제 중 방송 종료는 승인 안 함, 웹훅만으로 확정·같은 웹훅 반복·승인 확인↔웹훅 순서 무관 한 건, 꾸민 웹훅 본문 무시, 실패·만료·취소 웹훅, 모르는 결제·다른 이벤트 무시, 결제 서비스 장애 시 502, 내역·합계(확정만, 최신순), 끝난 방송의 내역, 실시간 알림 한 번, 후원받은 방송 삭제 409, 다른 인스턴스에서 확정된 후원 알림
- `tests/auth.rs`(9개): 회원가입·로그인·로그아웃, 잘못·만료·폐기된 토큰 401, 임시 서명 키
- `tests/api.rs`(30개): 과제 1·2 등록·목록·상세·재생·거부·재시작·웹 서빙·상태, 소유자 권한 표(401·403·허용), 삭제·종료 409
- `tests/comments.rs`(11개): 댓글 작성·목록·삭제 권한, 댓글 WebSocket 알림
- `tests/live.rs`(17개): 실제 포트에서 ffmpeg webm을 WebSocket으로 송출 → HLS → 종료 → 녹화본, 시작 거부, 소유자 원격 종료, 하트비트가 끊긴 방송의 시작 시 정리

웹 앱은 컴파일로 확인한다.

```bash
cargo clippy -p web --target wasm32-unknown-unknown
dx build -p web --release
```
