# OCI 배포 안내서

clone-chzzk를 Oracle Cloud(OCI)에 **Always Free 한도 안에서** 배포하는 순서다. 콘솔에서 직접 자원을 만들고, VM에서 Docker Compose로 앱을 띄운다.

- 리전: **Japan Central (Osaka, `ap-osaka-1`)** — 홈 리전. Always Free 자원은 홈 리전에서만 무료다.
- 구성: A1 VM 1대(앱 + ffmpeg + Caddy) + 비공개 Object Storage 버킷(녹화본)
- 접속: `https://<공인IP를 대시로>.sslip.io`. 휴대폰 브라우저는 HTTPS에서만 카메라·마이크를 허용하므로, 도메인 없이 sslip.io 주소에 Caddy가 Let's Encrypt 인증서를 자동으로 붙인다(무료).

> ⚠️ **과금 위험 — 먼저 읽기**
>
> PAYG(종량제) 계정은 Always Free 한도를 넘는 순간 바로 과금된다. 아래 단계마다 있는 **✅ 무료 확인** 항목을 지키고, 하나라도 다르면 만들기 전에 멈춘다.
>
> | 자원 | Always Free 한도(테넌시 합계) | 이 안내서의 사용량 |
> |---|---|---|
> | Ampere A1 Compute | 4 OCPU, 24GB 메모리 | 2 OCPU, 12GB |
> | 블록 스토리지(부트 볼륨 포함) | 200GB | 50GB |
> | Object Storage(Standard·IA·Archive 합계) | 20GB, API 요청 월 5만 건 | 앱이 15GiB에서 업로드를 막음 |
> | 아웃바운드 데이터 전송 | 월 10TB | 시청 트래픽 |
>
> 라이브 송출은 방송마다 ffmpeg가 A1 CPU로 다시 인코딩한다(720p 기준 OCPU 1개 안팎). 2 OCPU VM에서는 `MAX_LIVE_BROADCASTS=2`(기본값)를 넘기지 않는다. CPU를 더 쓴다고 과금되지는 않지만, 넘치면 모든 방송이 끊긴다.
>
> 다른 VM·볼륨·버킷이 이미 있다면 그 사용량도 합산된다.

## 0. 예산 알림 만들기 (권장, 무료)

과금이 시작되면 바로 알 수 있도록 먼저 만든다.

1. 콘솔 메뉴 → **Billing & Cost Management → Budgets → Create Budget**
2. 대상: 루트 컴파트먼트, 월 예산 **1 USD**, 알림 규칙: 실제 사용액이 예산의 **1%** 초과 시 이메일

## 1. 네트워크(VCN) 만들기

1. 메뉴 → **Networking → Virtual Cloud Networks → Start VCN Wizard → Create VCN with Internet Connectivity**
2. 이름 `chzzk-vcn`, 나머지 기본값으로 생성
3. 생성된 VCN → **Public Subnet** → 보안 목록(Default Security List) → **Add Ingress Rules**
   - Source CIDR `0.0.0.0/0`, IP Protocol TCP, Destination Port `80`
   - Source CIDR `0.0.0.0/0`, IP Protocol TCP, Destination Port `443` (나중에 HTTPS용)
   - SSH(22)는 마법사가 이미 열어 둔다.

✅ 무료 확인: VCN, 서브넷, 인터넷 게이트웨이, 보안 목록은 과금되지 않는다. **NAT Gateway·Load Balancer는 이 안내서에서 쓰지 않는다**(Load Balancer는 무료 1개가 있지만 필요 없다).

## 2. VM 만들기

1. 메뉴 → **Compute → Instances → Create instance**
2. 이름 `chzzk`
3. **Image**: Canonical Ubuntu 24.04 (aarch64 빌드가 자동 선택됨)
4. **Shape → Change shape → Ampere → `VM.Standard.A1.Flex`**
   - OCPU **2**, Memory **12 GB**
5. **Networking**: 위에서 만든 `chzzk-vcn`의 Public Subnet, **Automatically assign public IPv4 address** 체크
6. **SSH keys**: 내 공개 키 붙여넣기(또는 키 쌍 생성 후 개인 키 다운로드)
7. **Boot volume**: Specify a custom boot volume size → **50 GB**, 성능(VPU)은 기본값 그대로

✅ 무료 확인 (만들기 직전 요약 화면에서)
- Shape 옆에 **Always Free-eligible** 표시가 있다. `VM.Standard.E*`·`Optimized` 같은 다른 Shape는 과금된다.
- OCPU 2 / 12GB. 다른 A1 인스턴스와 합쳐 4 OCPU / 24GB를 넘지 않는다.
- 부트 볼륨 50GB. 다른 볼륨과 합쳐 200GB를 넘지 않는다. VPU를 기본값보다 올리지 않는다.
- 공인 IP는 **임시(ephemeral)**. 예약 IP(Reserved Public IP)는 만들지 않는다.

`Out of capacity` 오류가 나면 잠시 뒤 다시 시도하거나 다른 Availability Domain/Fault Domain을 고른다.

## 3. 녹화본 버킷 만들기

1. 메뉴 → **Storage → Object Storage & Archive Storage → Buckets → Create Bucket**
2. 이름 `chzzk-recordings`
3. Default Storage Tier **Standard**
4. **Enable Auto-Tiering 끔**, **Enable Object Versioning 끔**, **Emit Object Events 끔**
5. 생성 후 버킷의 Visibility가 **Private**인지 확인(기본값)
6. 같은 화면에서 **Namespace** 값을 메모해 둔다(테넌시 상세 화면에도 있다).

✅ 무료 확인
- Auto-Tiering을 켜면 객체가 Infrequent Access로 옮겨져 **조회 수수료**가 생길 수 있다 → 끈다.
- Versioning을 켜면 지운 녹화본의 이전 버전이 계속 용량을 차지한다 → 끈다.
- 앱은 녹화본 합계가 `STORAGE_QUOTA_BYTES`(기본 15GiB)를 넘으면 등록을 거부한다. 20GB보다 크게 올리지 않는다.
- 재생 중 탐색(Range 요청)과 업로드 파트(8MB당 1건)가 API 요청으로 집계된다. 월 5만 건은 소규모 사용에는 충분하지만, 많이 시청하면 콘솔의 Object Storage 지표로 확인한다.

## 4. Customer Secret Key 발급

앱은 OCI Object Storage의 **S3 호환 API**로 접근하며, 이때 Customer Secret Key를 쓴다.

1. 콘솔 오른쪽 위 프로필 → **My profile → Customer secret keys → Generate secret key**
2. 이름 `chzzk-server`
3. 표시되는 **Secret Key를 바로 복사**한다(다시 볼 수 없다).
4. 목록에 나타난 **Access Key** 값을 복사한다.

엔드포인트는 다음 형식이다.

```
https://<namespace>.compat.objectstorage.ap-osaka-1.oraclecloud.com
```

✅ 무료 확인: 키 발급은 과금되지 않는다. 키는 사용자당 2개까지다.

> 키는 VM의 `.env` 파일에만 넣는다. 저장소(git), 이슈, 채팅에 붙여넣지 않는다.

## 5. VM 준비

```bash
ssh ubuntu@<공인 IP>
```

Docker와 git을 설치한다.

```bash
sudo apt-get update
sudo apt-get install -y docker.io docker-compose-v2 docker-buildx git
sudo usermod -aG docker ubuntu
exit
```

다시 접속한 뒤, OCI Ubuntu 이미지에 기본으로 들어 있는 방화벽(iptables)에서 80·443을 연다. **보안 목록만 열면 접속되지 않는다.**

```bash
sudo iptables -I INPUT 6 -m state --state NEW -p tcp --dport 80 -j ACCEPT
sudo iptables -I INPUT 6 -m state --state NEW -p tcp --dport 443 -j ACCEPT
sudo netfilter-persistent save
```

## 6. 앱 설정과 실행

```bash
git clone https://github.com/Minse5k/clone-chzzk.git
cd clone-chzzk
cp .env.example .env
chmod 600 .env
nano .env
```

`.env`에서 다음 값을 채운다(나머지는 기본값 그대로 둔다).

```dotenv
STORAGE_BACKEND=oci
OCI_S3_ENDPOINT=https://<namespace>.compat.objectstorage.ap-osaka-1.oraclecloud.com
OCI_REGION=ap-osaka-1
OCI_BUCKET=chzzk-recordings
OCI_ACCESS_KEY_ID=<Access Key>
OCI_SECRET_ACCESS_KEY=<Secret Key>
# 휴대폰 송출용 HTTPS 주소. 공인 IP의 점(.)을 대시(-)로 바꾼다. 예: 공인 IP 158.101.1.2 → 158-101-1-2.sslip.io
SITE_ADDRESS=<공인IP를 대시로>.sslip.io
```

> sslip.io는 `158-101-1-2.sslip.io` 같은 이름을 그 IP로 돌려주는 무료 DNS다. 가입이나 설정이 없다. Caddy는 이 이름으로 Let's Encrypt 인증서를 받으므로 80·443 포트가 열려 있어야 한다(1·5단계).

> `DATABASE_PATH`, `STORAGE_DIR`, `SERVER_ADDR`, `WEB_DIST_DIR`는 `compose.yaml`이 컨테이너 경로로 고정하므로 `.env`의 값은 쓰이지 않는다. `API_BASE_URL`, `CORS_ALLOWED_ORIGINS`도 같은 출처로 서빙되는 배포에서는 쓰이지 않는다.

빌드하고 실행한다. 첫 빌드는 Dioxus CLI와 의존성을 컴파일하느라 20분 이상 걸릴 수 있다.

```bash
docker compose up -d --build
docker compose logs -f app
```

로그에 `서버 시작: http://0.0.0.0:3000`이 나오면 준비된 것이다. `ffmpeg(...)를 실행할 수 없어` 경고가 없어야 라이브 송출을 받는다(이미지에 ffmpeg가 들어 있다). OCI 설정이 틀리면 원인을 출력하고 컨테이너가 재시작을 반복하므로, 로그를 보고 `.env`를 고친 뒤 `docker compose up -d`로 다시 띄운다.

## 7. 직접 확인

브라우저에서 `https://<공인IP를 대시로>.sslip.io`를 연다. 주소창에 자물쇠가 보이면 인증서가 붙은 것이다. 처음 접속할 때 인증서 발급으로 몇 초 걸릴 수 있다(`docker compose logs caddy`에서 `certificate obtained successfully` 확인).

녹화방송(과제 1)

1. **방송 등록**에서 제목과 mp4/webm 파일을 올린다 → 상세 화면으로 이동하고 재생된다.
2. 재생 바를 움직여 원하는 지점으로 건너뛸 수 있다.
3. 상세 화면의 **공유 URL 복사**를 눌러 다른 기기·시크릿 창에서 연다 → 같은 방송이 열린다.
4. OCI 콘솔의 버킷 → Objects에 `recordings/<방송 ID>.mp4`가 생겼는지 확인한다.
5. `docker compose restart app` 후 목록과 재생이 그대로인지 확인한다.

휴대폰 라이브 송출(과제 2)

1. 휴대폰 브라우저(안드로이드 Chrome, iOS Safari)로 같은 `https://…sslip.io` 주소를 열고 **라이브 송출**을 누른다.
2. **카메라·마이크 켜기** → 권한을 허용하면 미리보기가 나온다. 제목을 넣고 **방송 시작**.
3. PC 브라우저에서 방송 목록을 열면 **라이브 중**에 방송이 보인다. 눌러서 3~6초 지연으로 시청한다.
4. 휴대폰에서 **방송 종료** → PC 화면이 `종료`로 바뀌고, 잠시 뒤 녹화본으로 다시 볼 수 있다.
5. OCI 버킷에 그 방송의 `recordings/<방송 ID>.mp4`가 생겼는지 확인한다.

## 다시 배포하기

```bash
cd ~/clone-chzzk
git pull
docker compose up -d --build
```

DB(SQLite)는 `app-data` 볼륨에 있어 재배포해도 유지된다. 백업은 다음처럼 한다.

```bash
docker compose cp app:/data/chzzk.db ./chzzk-backup.db
```

## 자기 도메인 쓰기 (선택)

sslip.io 대신 자기 도메인을 쓰려면 도메인의 A 레코드를 VM 공인 IP로 맞춘 뒤, `.env`의 `SITE_ADDRESS`를 바꾸고 다시 띄운다. Caddy가 새 인증서를 자동으로 발급한다.

```dotenv
SITE_ADDRESS=example.com
```

```bash
docker compose up -d
```

## 정리(과금 중단)

더 쓰지 않을 때는 다음 순서로 지운다: VM(Terminate, 부트 볼륨 영구 삭제 체크) → 버킷의 객체 → 버킷 → Customer Secret Key → VCN.
