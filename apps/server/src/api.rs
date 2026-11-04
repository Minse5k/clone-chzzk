use std::io;

use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, Multipart, Path, State, multipart::MultipartError},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use chrono::Utc;
use futures_util::TryStreamExt;
use serde_json::json;
use shared::{Broadcast, Recording, registration_fields};
use uuid::Uuid;

use crate::{
    AppState,
    db::{self, NewBroadcast, RecordingLocation},
    range::{self, RangeRequest},
    storage::ByteRange,
};

const TITLE_MAX_CHARS: usize = 100;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/broadcasts",
            get(list_broadcasts)
                .post(register_broadcast)
                // 녹화본은 스트리밍으로 저장하므로 axum 기본 본문 한도(2MB)를 쓰지 않는다.
                .layer(DefaultBodyLimit::disable()),
        )
        .route("/api/broadcasts/{id}", get(get_broadcast))
        .route("/api/broadcasts/{id}/recording", get(play_recording))
}

async fn list_broadcasts(State(state): State<AppState>) -> Result<Json<Vec<Broadcast>>, ApiError> {
    Ok(Json(db::list_broadcasts(&state.db).await?))
}

async fn get_broadcast(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Broadcast>, ApiError> {
    Ok(Json(find(&state, &id).await?.broadcast))
}

/// 방송 등록. multipart로 제목과 녹화본을 받는다.
/// 녹화본을 저장소에 끝까지 저장한 뒤에 DB에 방송을 기록하고, 도중에 실패하면 저장한 녹화본을 지운다.
async fn register_broadcast(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Result<(StatusCode, Json<Broadcast>), ApiError> {
    let id = Uuid::now_v7();
    let mut saved = None;
    let result = async {
        let title = receive_registration(&state, id, &mut multipart, &mut saved).await?;
        let recording = saved
            .as_ref()
            .ok_or_else(|| ApiError::bad_request("녹화본 파일을 선택해 주세요."))?;
        let new = NewBroadcast {
            id,
            title,
            created_at: Utc::now(),
            recording: Recording {
                content_type: recording.content_type.into(),
                size_bytes: recording.size_bytes,
            },
            recording_location: RecordingLocation {
                storage_backend: state.store.backend_name().into(),
                object_key: recording.object_key.clone(),
            },
        };
        db::insert_broadcast(&state.db, &new).await?;
        Ok::<_, ApiError>(Broadcast {
            id: new.id,
            title: new.title,
            created_at: new.created_at,
            recording: Some(new.recording),
        })
    }
    .await;

    if result.is_err()
        && let Some(recording) = &saved
        && let Err(err) = state.store.delete(&recording.object_key).await
    {
        tracing::error!("등록 실패 후 녹화본 정리 실패: {err:#}");
    }
    Ok((StatusCode::CREATED, Json(result?)))
}

/// 저장소에 저장을 마친 녹화본.
struct SavedRecording {
    object_key: String,
    content_type: &'static str,
    size_bytes: u64,
}

/// multipart 필드를 읽어 녹화본을 저장하고, 검증한 제목을 돌려준다.
/// 녹화본을 저장했다면 실패하더라도 `saved`에 남겨 호출자가 정리하게 한다.
async fn receive_registration(
    state: &AppState,
    id: Uuid,
    multipart: &mut Multipart,
    saved: &mut Option<SavedRecording>,
) -> Result<String, ApiError> {
    let mut title = None;
    while let Some(field) = multipart.next_field().await.map_err(upload_interrupted)? {
        match field.name() {
            Some(registration_fields::TITLE) => {
                title = Some(field.text().await.map_err(upload_interrupted)?);
            }
            Some(registration_fields::RECORDING) => {
                if saved.is_some() {
                    return Err(ApiError::bad_request("녹화본은 하나만 등록할 수 있습니다."));
                }
                let video = VideoType::detect(field.file_name(), field.content_type())
                    .ok_or_else(|| ApiError::new(
                        StatusCode::UNSUPPORTED_MEDIA_TYPE,
                        "mp4 또는 webm 영상만 등록할 수 있습니다.",
                    ))?;
                let object_key = format!("{id}.{}", video.extension());
                let stream = field.map_err(io::Error::other);
                let size_bytes = state.store.put(&object_key, stream).await.map_err(|err| {
                    if err.get_ref().is_some_and(|inner| inner.is::<MultipartError>()) {
                        ApiError::bad_request("업로드가 중간에 끊겼습니다. 다시 시도해 주세요.")
                    } else {
                        anyhow::Error::from(err).context("녹화본 저장 실패").into()
                    }
                })?;
                *saved = Some(SavedRecording { object_key, content_type: video.content_type(), size_bytes });
            }
            _ => {}
        }
    }
    validate_title(title)
}

fn validate_title(title: Option<String>) -> Result<String, ApiError> {
    let title = title.as_deref().map(str::trim).unwrap_or_default();
    if title.is_empty() {
        return Err(ApiError::bad_request("제목을 입력해 주세요."));
    }
    if title.chars().count() > TITLE_MAX_CHARS {
        return Err(ApiError::bad_request(format!("제목은 {TITLE_MAX_CHARS}자 이하로 입력해 주세요.")));
    }
    Ok(title.to_owned())
}

fn upload_interrupted(err: MultipartError) -> ApiError {
    tracing::warn!("multipart 읽기 실패: {err}");
    ApiError::bad_request("업로드 요청을 읽을 수 없습니다. 다시 시도해 주세요.")
}

/// 브라우저 `<video>`로 바로 재생되는, 등록 가능한 녹화본 형식.
#[derive(Clone, Copy)]
enum VideoType {
    Mp4,
    Webm,
}

impl VideoType {
    /// 파일 확장자를 먼저 보고, 없으면 콘텐츠 타입을 본다.
    fn detect(file_name: Option<&str>, content_type: Option<&str>) -> Option<Self> {
        let extension = file_name
            .and_then(|name| name.rsplit_once('.'))
            .map(|(_, ext)| ext.to_ascii_lowercase());
        match extension.as_deref() {
            Some("mp4") => Some(Self::Mp4),
            Some("webm") => Some(Self::Webm),
            Some(_) => None,
            None => match content_type {
                Some("video/mp4") => Some(Self::Mp4),
                Some("video/webm") => Some(Self::Webm),
                _ => None,
            },
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Self::Mp4 => "mp4",
            Self::Webm => "webm",
        }
    }

    fn content_type(self) -> &'static str {
        match self {
            Self::Mp4 => "video/mp4",
            Self::Webm => "video/webm",
        }
    }
}

/// 녹화본 재생. Range 요청을 지원해 원하는 지점으로 건너뛸 수 있다.
async fn play_recording(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let stored = find(&state, &id).await?;
    let (Some(recording), Some(location)) = (stored.broadcast.recording, stored.recording_location) else {
        return Err(ApiError::not_found("이 방송에는 녹화본이 없습니다."));
    };
    let size = recording.size_bytes;
    let range_header = headers.get(header::RANGE).and_then(|v| v.to_str().ok());

    let (status, range) = match range::parse(range_header, size) {
        RangeRequest::Full => (StatusCode::OK, ByteRange { start: 0, len: size }),
        RangeRequest::Partial(range) => (StatusCode::PARTIAL_CONTENT, range),
        RangeRequest::Unsatisfiable => {
            return Ok((
                StatusCode::RANGE_NOT_SATISFIABLE,
                [(header::CONTENT_RANGE, format!("bytes */{size}"))],
            )
                .into_response());
        }
    };

    let body = if range.len == 0 {
        Body::empty()
    } else {
        state.store.read_range(&location.object_key, range).await?
    };
    let mut response = Response::new(body);
    *response.status_mut() = status;
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, header_value(&recording.content_type)?);
    headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(range.len));
    if status == StatusCode::PARTIAL_CONTENT {
        let last = range.start + range.len - 1;
        headers.insert(header::CONTENT_RANGE, header_value(&format!("bytes {}-{last}/{size}", range.start))?);
    }
    Ok(response)
}

fn header_value(value: &str) -> Result<HeaderValue, ApiError> {
    HeaderValue::from_str(value).map_err(|err| anyhow::Error::from(err).into())
}

/// 방송을 찾는다. ID 형식이 틀려도 없는 방송과 똑같이 404로 답한다.
async fn find(state: &AppState, id: &str) -> Result<db::StoredBroadcast, ApiError> {
    let not_found = || ApiError::not_found("방송을 찾을 수 없습니다.");
    let id = Uuid::parse_str(id).map_err(|_| not_found())?;
    db::find_broadcast(&state.db, id).await?.ok_or_else(not_found)
}

/// API 오류. 본문에 사용자에게 보여 줄 한국어 메시지를 담는다.
#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self { status, message: message.into() }
    }

    fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, message)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({ "message": self.message }))).into_response()
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(err: anyhow::Error) -> Self {
        tracing::error!("{err:#}");
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "서버 오류가 발생했습니다. 잠시 후 다시 시도해 주세요.",
        )
    }
}
