# syntax=docker/dockerfile:1
# clone-chzzk 서버 + 웹 이미지. OCI A1(ARM) VM에서 직접 빌드한다(맥 Apple Silicon에서도 같은 아키텍처).

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
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    dx build -p web --release \
    && cargo build -p server --release --locked \
    && mkdir -p /out \
    && cp -r target/dx/web/release/web/public /out/web \
    && cp target/release/server /out/server

FROM debian:trixie-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=build /out/server /app/server
COPY --from=build /out/web /app/web

ENV SERVER_ADDR=0.0.0.0:3000 \
    WEB_DIST_DIR=/app/web \
    DATABASE_PATH=/data/chzzk.db \
    STORAGE_DIR=/data/recordings \
    RUST_LOG=info,tower_http=info
VOLUME /data
EXPOSE 3000
CMD ["/app/server"]
