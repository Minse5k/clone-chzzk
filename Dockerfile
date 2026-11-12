# syntax=docker/dockerfile:1
# clone-chzzk 서버 + 웹 이미지(라이브 송출을 HLS로 바꾸는 ffmpeg 포함). OCI A1(ARM) VM에서 직접 빌드한다(맥 Apple Silicon에서도 같은 아키텍처).

# dx가 내려받는 wasm-bindgen 실행 파일이 glibc 2.38 이상을 요구해 trixie를 쓴다.
FROM rust:1-trixie AS build
WORKDIR /src

RUN rustup target add wasm32-unknown-unknown
# Dioxus CLI는 웹 앱의 dioxus 버전(Cargo.lock)과 맞춘다.
ARG DIOXUS_CLI_VERSION=0.7.10
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    cargo install dioxus-cli --version ${DIOXUS_CLI_VERSION} --locked

COPY . .

# 웹은 서버와 같은 출처에서 서빙되므로 API 주소를 비워 상대 경로로 호출하게 한다.
ENV API_BASE_URL=""
# 이전 빌드의 웹 출력은 지워 해시가 다른 옛 파일이 쌓이지 않게 하고,
# 빌드 캐시(target)에 남은 결과물보다 소스가 새것으로 보이도록 워크스페이스 소스의 수정 시각을 갱신한다.
# 복사된 파일의 수정 시각만으로는 cargo가 변경을 알아채지 못해, 다시 배포해도 이전 웹이 남는 일이 있었다.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    find apps crates -type f -exec touch {} + \
    && rm -rf target/dx/web/release/web/public \
    && dx build -p web --release \
    && cargo build -p server --release --locked \
    && mkdir -p /out \
    && cp -r target/dx/web/release/web/public /out/web \
    && cp target/release/server /out/server

FROM debian:trixie-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates ffmpeg \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=build /out/server /app/server
COPY --from=build /out/web /app/web

ENV SERVER_ADDR=0.0.0.0:3000 \
    WEB_DIST_DIR=/app/web \
    DATABASE_PATH=/data/chzzk.db \
    STORAGE_DIR=/data/recordings \
    LIVE_DIR=/data/live \
    RUST_LOG=info,tower_http=info
VOLUME /data
EXPOSE 3000
CMD ["/app/server"]
