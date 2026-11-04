# Dioxus fullstack 대신 axum 서버를 따로 둔다

Dioxus는 server functions로 웹과 서버를 한 crate에 묶을 수 있지만, 우리는 `apps/server`(axum)를 `apps/web`(Dioxus web)과 분리한 Cargo workspace로 간다. 최종 목표에서 데스크톱 송출 앱(`apps/desktop`)과 라이브 수집용 장기 연결이 같은 서버를 쓰므로, 웹 앱에 종속되지 않은 독립 HTTP API가 경계를 분명하게 한다. 배포 시에는 axum 서버가 빌드된 웹 정적 파일도 함께 서빙한다.
