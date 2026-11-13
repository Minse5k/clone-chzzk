//! 라이브 송출. 송출자가 WebSocket으로 보낸 녹화 조각을 ffmpeg로 HLS로 바꾸고, 시청자에게 서빙한다.
//! 설계 근거는 ADR 0003.

use std::{
    collections::HashMap,
    path::{Path as FsPath, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    body::Bytes,
    extract::{
        Path, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::{HeaderValue, header},
    response::{IntoResponse, Response},
};
use chrono::Utc;
use shared::{
    Broadcast, BroadcastStatus, IngestClientMessage, IngestServerMessage, LIVE_PLAYLIST_FILE,
    Recording, User, format_bytes,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, Command},
    sync::{OwnedSemaphorePermit, Semaphore, oneshot},
};
use tokio_util::io::ReaderStream;
use uuid::Uuid;

use crate::{
    AppState, Config,
    api::{ApiError, validate_title},
    auth,
    db::{self, InsertError, RecordingLocation},
};

/// 녹화본을 저장소에 올리기 전에 라이브 디렉터리 안에서 묶는 mp4 파일 이름.
const RECORDING_FILE: &str = "recording.mp4";

/// 연결 후 시작 메시지를 기다리는 시간.
const START_TIMEOUT: Duration = Duration::from_secs(10);
/// 송출이 끝난 뒤 ffmpeg가 남은 조각을 마무리하기를 기다리는 시간.
const FINISH_TIMEOUT: Duration = Duration::from_secs(30);

/// 송출 WebSocket. 연결의 수명이 곧 라이브 방송의 수명이다.
pub async fn ingest(State(state): State<AppState>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| run_ingest(state, socket))
}

async fn run_ingest(state: AppState, mut socket: WebSocket) {
    let (title, owner) = match receive_start(&state, &mut socket).await {
        Ok(started) => started,
        Err(message) => return reject(socket, message).await,
    };
    let (slot, limit) = match admit(&state).await {
        Ok(admitted) => admitted,
        Err(message) => return reject(socket, message).await,
    };

    let id = Uuid::now_v7();
    let dir = live_dir(&state, id);
    let mut encoder = match HlsEncoder::spawn(&state.config.ffmpeg_path, &dir, id).await {
        Ok(encoder) => encoder,
        Err(err) => {
            tracing::error!(broadcast = %id, "ffmpeg를 시작할 수 없습니다: {err:#}");
            let _ = tokio::fs::remove_dir_all(&dir).await;
            return reject(
                socket,
                "서버가 영상을 처리할 수 없어 송출을 시작할 수 없습니다.",
            )
            .await;
        }
    };
    let started_at = Utc::now();
    if let Err(err) = db::insert_live_broadcast(&state.db, id, &title, owner.id, started_at).await {
        tracing::error!(broadcast = %id, "라이브 방송을 기록할 수 없습니다: {err:#}");
        encoder.finish().await;
        let _ = tokio::fs::remove_dir_all(&dir).await;
        return reject(
            socket,
            "송출을 시작할 수 없습니다. 잠시 후 다시 시도해 주세요.",
        )
        .await;
    }
    tracing::info!(broadcast = %id, "송출 시작: {title}");
    let (registration, mut stop) = state.live.register(id);

    let broadcast = Broadcast {
        id,
        title,
        owner,
        created_at: started_at,
        status: BroadcastStatus::Live,
        ended_at: None,
        recording: None,
        live_playlist: false,
    };
    send(&mut socket, &IngestServerMessage::Started { broadcast }).await;

    let reason = relay(
        &mut socket,
        &mut encoder,
        &mut stop,
        limit,
        state.config.live_idle_timeout,
    )
    .await;
    drop(registration);

    // 상태를 먼저 종료로 바꿔, 마무리하는 동안에도 시청자가 라이브로 착각하지 않게 한다.
    if let Err(err) = db::end_broadcast(&state.db, id, Utc::now()).await {
        tracing::error!(broadcast = %id, "방송을 종료로 기록할 수 없습니다: {err:#}");
    }
    encoder.finish().await;
    // 인코딩이 끝났으니 동시 라이브 자리를 돌려준다. 송출자가 종료 응답을 받으면 바로 다시 시작할 수 있다.
    drop(slot);
    tracing::info!(broadcast = %id, "송출 종료");

    send(&mut socket, &IngestServerMessage::Ended { reason }).await;
    let _ = socket.send(Message::Close(None)).await;
    drop(socket);

    save_recording(&state, id).await;
}

/// 끝난 송출의 HLS 조각을 다시 인코딩 없이 mp4로 묶어 녹화본 저장소에 넣고 라이브 디렉터리를 지운다.
/// 녹화본을 만들 수 없으면(조각 없음, 저장 총량 초과) 녹화본 없이 종료된 방송으로 남는다.
pub async fn save_recording(state: &AppState, id: Uuid) {
    let dir = live_dir(state, id);
    match make_recording(state, id, &dir).await {
        Ok(Some(size)) => tracing::info!(broadcast = %id, "녹화본 저장: {}", format_bytes(size)),
        Ok(None) => {}
        Err(err) => tracing::error!(broadcast = %id, "녹화본을 만들 수 없습니다: {err:#}"),
    }
    if let Err(err) = tokio::fs::remove_dir_all(&dir).await
        && err.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(broadcast = %id, "라이브 디렉터리를 지울 수 없습니다: {err}");
    }
}

/// 녹화본을 만들어 저장하고 크기를 돌려준다. 만들 녹화본이 없으면 `None`.
async fn make_recording(state: &AppState, id: Uuid, dir: &FsPath) -> anyhow::Result<Option<u64>> {
    let playlist = dir.join(LIVE_PLAYLIST_FILE);
    if !tokio::fs::try_exists(&playlist).await? {
        tracing::info!(broadcast = %id, "송출된 영상이 없어 녹화본을 만들지 않습니다");
        return Ok(None);
    }

    let mp4 = dir.join(RECORDING_FILE);
    let status = Command::new(&state.config.ffmpeg_path)
        .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(&playlist)
        // 재생을 바로 시작할 수 있도록 색인(moov)을 앞에 둔다.
        .args(["-c", "copy", "-movflags", "+faststart"])
        .arg(&mp4)
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .status()
        .await?;
    anyhow::ensure!(
        status.success(),
        "ffmpeg가 녹화본을 묶지 못했습니다: {status}"
    );
    let size = tokio::fs::metadata(&mp4).await?.len();

    // 큰 녹화본을 저장소에 올렸다가 지우지 않도록 먼저 확인한다. 기록할 때 한 번 더 확인한다.
    let used = db::total_recording_bytes(&state.db).await?;
    if used.saturating_add(size) > state.config.storage_quota_bytes {
        tracing::warn!(broadcast = %id, "저장 총량 한도를 넘어 녹화본을 남기지 않습니다");
        return Ok(None);
    }

    let key = format!("{id}.mp4");
    let file = tokio::fs::File::open(&mp4).await?;
    state.store.put(&key, ReaderStream::new(file)).await?;
    let recording = Recording {
        content_type: "video/mp4".into(),
        size_bytes: size,
    };
    let location = RecordingLocation {
        storage_backend: state.store.backend_name().into(),
        object_key: key.clone(),
    };
    let quota = state.config.storage_quota_bytes;
    match db::insert_recording(&state.db, id, &recording, &location, quota).await {
        Ok(()) => Ok(Some(size)),
        Err(err) => {
            if let Err(err) = state.store.delete(&key).await {
                tracing::error!(broadcast = %id, "녹화본 정리 실패: {err:#}");
            }
            match err {
                InsertError::QuotaExceeded => {
                    tracing::warn!(broadcast = %id, "저장 총량 한도를 넘어 녹화본을 남기지 않습니다");
                    Ok(None)
                }
                InsertError::Other(err) => Err(err),
            }
        }
    }
}

/// 송출을 받을 수 있는 서버 상태. 서버를 시작할 때 한 번 확인한다.
pub struct LiveCapacity {
    ffmpeg_available: bool,
    max_live_broadcasts: usize,
    slots: Arc<Semaphore>,
    /// 진행 중인 송출 연결에 종료를 알리는 신호. 소유자가 다른 기기에서 방송을 끝낼 때 쓴다.
    stops: Mutex<HashMap<Uuid, oneshot::Sender<()>>>,
}

/// 소유자가 다른 기기에서 방송을 끝냈을 때 송출자에게 보여 주는 이유.
pub const STOPPED_BY_OWNER: &str = "소유자가 다른 기기에서 방송을 종료했습니다.";

impl LiveCapacity {
    /// 진행 중인 송출 연결에 종료를 알린다. 그 방송을 송출하는 연결이 없으면 `false`.
    pub fn request_stop(&self, id: Uuid) -> bool {
        let sender = self.stops.lock().expect("송출 종료 신호 잠금").remove(&id);
        sender.is_some_and(|sender| sender.send(()).is_ok())
    }

    /// 송출 연결을 종료 신호 대상으로 등록한다. 돌려준 값이 사라지면 등록이 풀린다.
    fn register(self: &Arc<Self>, id: Uuid) -> (StopRegistration, oneshot::Receiver<()>) {
        let (sender, receiver) = oneshot::channel();
        self.stops
            .lock()
            .expect("송출 종료 신호 잠금")
            .insert(id, sender);
        let registration = StopRegistration {
            live: Arc::clone(self),
            id,
        };
        (registration, receiver)
    }
}

/// 송출이 끝나면 종료 신호 등록을 푼다.
struct StopRegistration {
    live: Arc<LiveCapacity>,
    id: Uuid,
}

impl Drop for StopRegistration {
    fn drop(&mut self) {
        if let Ok(mut stops) = self.live.stops.lock() {
            stops.remove(&self.id);
        }
    }
}

impl LiveCapacity {
    /// ffmpeg를 실행할 수 있는지 확인한다. 없어도 서버는 시작하고 송출만 거부한다.
    pub async fn check(config: &Config) -> Self {
        let ffmpeg_available = Command::new(&config.ffmpeg_path)
            .arg("-version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .is_ok_and(|status| status.success());
        if !ffmpeg_available {
            tracing::warn!(
                "ffmpeg({})를 실행할 수 없어 라이브 송출을 받지 않습니다. 녹화방송 기능은 그대로 동작합니다",
                config.ffmpeg_path.display()
            );
        }
        Self {
            ffmpeg_available,
            max_live_broadcasts: config.max_live_broadcasts,
            slots: Arc::new(Semaphore::new(config.max_live_broadcasts)),
            stops: Mutex::default(),
        }
    }
}

/// 송출 하나가 받을 수 있는 녹화 조각 크기의 합.
#[derive(Clone, Copy)]
struct ReceiveLimit {
    max_bytes: u64,
    /// 녹화본 상한이 아니라 남은 저장 공간 때문에 정해진 한도인지.
    by_quota: bool,
}

impl ReceiveLimit {
    fn reason(self) -> String {
        if self.by_quota {
            "저장 공간 한도에 닿아 방송을 종료했습니다.".into()
        } else {
            format!(
                "녹화본 크기 상한({})에 닿아 방송을 종료했습니다.",
                format_bytes(self.max_bytes)
            )
        }
    }
}

/// 송출을 시작해도 되는지 확인한다. 받아들이면 동시 라이브 자리 하나와 받을 수 있는 크기를 돌려준다.
async fn admit(state: &AppState) -> Result<(OwnedSemaphorePermit, ReceiveLimit), String> {
    let live = &state.live;
    if !live.ffmpeg_available {
        return Err(
            "서버에 ffmpeg가 없어 라이브 송출을 할 수 없습니다. 녹화방송 등록과 시청은 그대로 쓸 수 있습니다."
                .into(),
        );
    }
    let used = db::total_recording_bytes(&state.db).await.map_err(|err| {
        tracing::error!("저장 공간을 확인할 수 없습니다: {err:#}");
        "송출을 시작할 수 없습니다. 잠시 후 다시 시도해 주세요.".to_owned()
    })?;
    let remaining = state.config.storage_quota_bytes.saturating_sub(used);
    if remaining == 0 {
        return Err("저장 공간 한도에 도달해 더 이상 방송을 시작할 수 없습니다.".into());
    }
    let slot = live.slots.clone().try_acquire_owned().map_err(|_| {
        format!(
            "지금은 동시에 진행할 수 있는 라이브 방송 수({}개)가 모두 차 있습니다. 잠시 후 다시 시도해 주세요.",
            live.max_live_broadcasts
        )
    })?;
    let limit = ReceiveLimit {
        max_bytes: state.config.max_upload_bytes.min(remaining),
        by_quota: remaining < state.config.max_upload_bytes,
    };
    Ok((slot, limit))
}

/// 서버가 송출 도중에 멈춰 라이브 중으로 남은 방송을 종료하고, 남은 조각이 있으면 녹화본으로 만든다.
pub async fn recover(state: &AppState) -> anyhow::Result<()> {
    let leftovers = db::list_broadcasts(&state.db, Some(BroadcastStatus::Live)).await?;
    for broadcast in leftovers {
        tracing::warn!(broadcast = %broadcast.id, "서버 재시작 전의 라이브 방송을 종료합니다");
        db::end_broadcast(&state.db, broadcast.id, Utc::now()).await?;
        save_recording(state, broadcast.id).await;
    }
    Ok(())
}

/// 첫 메시지로 송출 시작을 받아 검증한 제목과 송출한 사용자(소유자)를 돌려준다.
/// 로그인하지 않았거나 토큰이 잘못되었으면 방송을 만들기 전에 거절한다.
async fn receive_start(state: &AppState, socket: &mut WebSocket) -> Result<(String, User), String> {
    let message = tokio::time::timeout(START_TIMEOUT, socket.recv())
        .await
        .map_err(|_| "송출 시작 요청이 오지 않았습니다.".to_owned())?;
    let Some(Ok(Message::Text(text))) = message else {
        return Err("송출 시작 요청을 먼저 보내야 합니다.".into());
    };
    match serde_json::from_str(&text) {
        Ok(IngestClientMessage::Start {
            token,
            title,
            mime_type,
        }) => {
            tracing::debug!("송출 녹화 형식: {mime_type}");
            let token = token
                .filter(|token| !token.is_empty())
                .ok_or_else(|| auth::login_required().message().to_owned())?;
            let owner = auth::authenticate(state, &token)
                .await
                .map_err(|err| err.message().to_owned())?
                .user;
            let title = validate_title(&title).map_err(|err| err.message().to_owned())?;
            Ok((title, owner))
        }
        _ => Err("송출 시작 요청을 먼저 보내야 합니다.".into()),
    }
}

/// 녹화 조각을 ffmpeg로 흘려 넣는다. 송출자가 끝내거나 연결이 끊기면 `None`,
/// 서버 사정으로 끝내야 하면 송출자에게 보여 줄 이유를 돌려준다.
async fn relay(
    socket: &mut WebSocket,
    encoder: &mut HlsEncoder,
    stop: &mut oneshot::Receiver<()>,
    limit: ReceiveLimit,
    idle_timeout: Duration,
) -> Option<String> {
    let mut received: u64 = 0;
    loop {
        let message = tokio::select! {
            message = tokio::time::timeout(idle_timeout, socket.recv()) => message,
            Ok(()) = &mut *stop => return Some(STOPPED_BY_OWNER.into()),
        };
        let Ok(message) = message else {
            return Some(format!(
                "{}초 동안 영상이 오지 않아 방송을 종료했습니다.",
                idle_timeout.as_secs()
            ));
        };
        match message {
            Some(Ok(Message::Binary(chunk))) => {
                received += chunk.len() as u64;
                if received > limit.max_bytes {
                    return Some(limit.reason());
                }
                if let Err(err) = encoder.write(&chunk).await {
                    tracing::warn!(broadcast = %encoder.id, "ffmpeg에 조각을 넘길 수 없습니다: {err}");
                    return Some("영상을 처리하는 중 문제가 생겨 방송을 종료했습니다.".into());
                }
            }
            Some(Ok(Message::Text(text))) => {
                if let Ok(IngestClientMessage::End) = serde_json::from_str(&text) {
                    return None;
                }
            }
            Some(Ok(Message::Close(_)) | Err(_)) | None => return None,
            Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
        }
    }
}

async fn reject(mut socket: WebSocket, message: impl Into<String>) {
    let message = message.into();
    send(&mut socket, &IngestServerMessage::Error { message }).await;
    let _ = socket.send(Message::Close(None)).await;
}

/// 송출자에게 메시지를 보낸다. 이미 연결이 끊겼으면 무시한다.
async fn send(socket: &mut WebSocket, message: &IngestServerMessage) {
    let text = serde_json::to_string(message).expect("송출 메시지는 항상 JSON으로 바뀐다");
    let _ = socket.send(Message::Text(text.into())).await;
}

fn live_dir(state: &AppState, id: Uuid) -> PathBuf {
    state.config.live_dir.join(id.to_string())
}

/// 방송의 라이브 재생 목록이 만들어졌는지.
pub async fn has_playlist(state: &AppState, id: Uuid) -> bool {
    tokio::fs::try_exists(live_dir(state, id).join(LIVE_PLAYLIST_FILE))
        .await
        .unwrap_or(false)
}

/// 라이브 재생 목록과 조각 파일. 정해진 이름 형식만 받아 다른 파일에 접근할 수 없게 한다.
pub async fn serve_file(
    State(state): State<AppState>,
    Path((id, file)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let not_found = || ApiError::not_found("라이브 영상을 찾을 수 없습니다.");
    let id = Uuid::parse_str(&id).map_err(|_| not_found())?;
    let (content_type, cache_control) = if file == LIVE_PLAYLIST_FILE {
        // 재생 목록은 조각이 생길 때마다 바뀐다.
        ("application/vnd.apple.mpegurl", "no-cache")
    } else if is_segment_name(&file) {
        ("video/mp2t", "public, max-age=60")
    } else {
        return Err(not_found());
    };
    let bytes = match tokio::fs::read(live_dir(&state, id).join(&file)).await {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Err(not_found()),
        Err(err) => return Err(anyhow::Error::from(err).into()),
    };
    Ok((
        [
            (header::CONTENT_TYPE, HeaderValue::from_static(content_type)),
            (
                header::CACHE_CONTROL,
                HeaderValue::from_static(cache_control),
            ),
        ],
        Bytes::from(bytes),
    )
        .into_response())
}

/// ffmpeg가 만드는 조각 이름: `seg00000.ts`, `seg00001.ts`, ...
fn is_segment_name(name: &str) -> bool {
    name.strip_prefix("seg")
        .and_then(|rest| rest.strip_suffix(".ts"))
        .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
}

/// 송출 하나를 HLS로 바꾸는 ffmpeg 프로세스.
struct HlsEncoder {
    id: Uuid,
    child: Child,
    stdin: Option<ChildStdin>,
}

impl HlsEncoder {
    async fn spawn(ffmpeg: &FsPath, dir: &FsPath, id: Uuid) -> anyhow::Result<Self> {
        tokio::fs::create_dir_all(dir).await?;
        let mut child = Command::new(ffmpeg)
            .args(["-hide_banner", "-loglevel", "warning", "-i", "pipe:0"])
            .args(["-map", "0:v:0", "-map", "0:a:0?"])
            // 짧은 변이 720px를 넘지 않게 줄인다(세로 화면도 그대로 세로로).
            .args([
                "-vf",
                "scale='if(gt(iw,ih),-2,min(720,iw))':'if(gt(iw,ih),min(720,ih),-2)'",
            ])
            .args([
                "-c:v",
                "libx264",
                "-preset",
                "veryfast",
                "-tune",
                "zerolatency",
            ])
            .args(["-pix_fmt", "yuv420p", "-b:v", "2500k", "-maxrate", "2500k"])
            .args(["-bufsize", "5000k"])
            // 조각을 2초마다 자를 수 있도록 2초마다 키프레임을 넣는다.
            .args([
                "-force_key_frames",
                "expr:gte(t,n_forced*2)",
                "-sc_threshold",
                "0",
            ])
            .args(["-c:a", "aac", "-b:a", "128k", "-ar", "48000", "-ac", "2"])
            .args(["-f", "hls", "-hls_time", "2", "-hls_list_size", "0"])
            .args([
                "-hls_playlist_type",
                "event",
                "-hls_flags",
                "independent_segments",
            ])
            .arg("-hls_segment_filename")
            .arg(dir.join("seg%05d.ts"))
            .arg(dir.join(LIVE_PLAYLIST_FILE))
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;

        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::warn!(broadcast = %id, "ffmpeg: {line}");
                }
            });
        }
        let stdin = child.stdin.take();
        Ok(Self { id, child, stdin })
    }

    async fn write(&mut self, chunk: &[u8]) -> std::io::Result<()> {
        match &mut self.stdin {
            Some(stdin) => stdin.write_all(chunk).await,
            None => Err(std::io::ErrorKind::BrokenPipe.into()),
        }
    }

    /// 입력을 닫아 ffmpeg가 마지막 조각과 재생 목록 끝(`#EXT-X-ENDLIST`)을 쓰게 하고 기다린다.
    async fn finish(mut self) {
        drop(self.stdin.take());
        match tokio::time::timeout(FINISH_TIMEOUT, self.child.wait()).await {
            Ok(Ok(status)) if status.success() => {}
            Ok(Ok(status)) => tracing::warn!(broadcast = %self.id, "ffmpeg 비정상 종료: {status}"),
            Ok(Err(err)) => tracing::warn!(broadcast = %self.id, "ffmpeg 대기 실패: {err}"),
            Err(_) => {
                tracing::warn!(broadcast = %self.id, "ffmpeg가 끝나지 않아 강제로 종료합니다");
                let _ = self.child.kill().await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::is_segment_name;

    #[test]
    fn 조각_이름은_seg_숫자_ts만_허용한다() {
        assert!(is_segment_name("seg00000.ts"));
        assert!(is_segment_name("seg123456.ts"));
        assert!(!is_segment_name("seg.ts"));
        assert!(!is_segment_name("seg0a.ts"));
        assert!(!is_segment_name("../test.db"));
        assert!(!is_segment_name("seg00000.ts.bak"));
    }
}
