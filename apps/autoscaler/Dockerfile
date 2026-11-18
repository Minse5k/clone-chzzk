# syntax=docker/dockerfile:1
# 직접 만든 오토스케일러 이미지. docker CLI(compose 플러그인 포함) 이미지에 정적 링크한 실행 파일을 넣는다.
# 호스트의 Docker 소켓과 프로젝트 디렉터리(compose.yaml, .env)를 마운트해서 쓴다(compose.yaml 참고).
FROM rust:1-alpine AS build
WORKDIR /src
RUN apk add --no-cache musl-dev
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build -p autoscaler --release --locked \
    && cp target/release/autoscaler /autoscaler

FROM docker:29-cli
COPY --from=build /autoscaler /usr/local/bin/autoscaler
ENTRYPOINT ["/usr/local/bin/autoscaler"]
