//! 휴대폰 브라우저에서 송출하기: 카메라·마이크 입력을 MediaRecorder로 1초 단위 조각으로 만들어
//! WebSocket으로 서버에 보낸다. 서버가 조각을 HLS로 바꿔 시청자에게 보여 준다(ADR 0003).

use futures_channel::mpsc::UnboundedSender;
use js_sys::{Object, Reflect};
use shared::{Broadcast, IngestClientMessage, IngestServerMessage, LIVE_INGEST_PATH};
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    BinaryType, BlobEvent, CloseEvent, HtmlVideoElement, MediaRecorder, MediaRecorderOptions,
    MediaStream, MediaStreamConstraints, MediaStreamTrack, MessageEvent, RecordingState, WebSocket,
};

use crate::api;

/// 녹화 조각 하나의 길이. 서버의 HLS 조각(2초)보다 짧게 둔다.
const CHUNK_MILLIS: i32 = 1000;
const VIDEO_BITS_PER_SECOND: u32 = 2_500_000;
const AUDIO_BITS_PER_SECOND: u32 = 128_000;

/// 쓸 카메라. 휴대폰에서는 전면(`User`)과 후면(`Environment`) 카메라다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Facing {
    User,
    Environment,
}

impl Facing {
    fn constraint(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Environment => "environment",
        }
    }

    pub fn other(self) -> Self {
        match self {
            Self::User => Self::Environment,
            Self::Environment => Self::User,
        }
    }
}

/// 카메라와 마이크를 켠다. 브라우저가 권한을 묻고, 거부하면 이유를 담은 안내 문구를 돌려준다.
pub async fn open_camera(facing: Facing) -> Result<MediaStream, String> {
    let window = web_sys::window().ok_or_else(unsupported)?;
    if !window.is_secure_context() {
        return Err(
            "카메라와 마이크는 HTTPS 주소(또는 localhost)에서만 쓸 수 있습니다. 휴대폰에서는 README의 HTTPS 접속 방법으로 다시 열어 주세요."
                .into(),
        );
    }
    let devices = window
        .navigator()
        .media_devices()
        .map_err(|_| unsupported())?;
    if recording_mime_type().is_none() {
        return Err(unsupported());
    }

    let video = object(&[
        ("facingMode", JsValue::from_str(facing.constraint())),
        ("width", object(&[("ideal", 1280.into())]).into()),
        ("height", object(&[("ideal", 720.into())]).into()),
        ("frameRate", object(&[("ideal", 30.into())]).into()),
    ]);
    let audio = object(&[
        ("echoCancellation", true.into()),
        ("noiseSuppression", true.into()),
    ]);
    let constraints = MediaStreamConstraints::new();
    constraints.set_video(&video);
    constraints.set_audio(&audio);

    let request = devices
        .get_user_media_with_constraints(&constraints)
        .map_err(|err| camera_error(&err))?;
    let stream = JsFuture::from(request)
        .await
        .map_err(|err| camera_error(&err))?;
    Ok(stream.unchecked_into())
}

fn unsupported() -> String {
    "이 브라우저는 카메라 송출을 지원하지 않습니다. 휴대폰의 Chrome(안드로이드) 또는 Safari(iOS) 최신 버전을 써 주세요."
        .into()
}

/// `getUserMedia` 실패 이유(DOMException 이름)를 사용자 안내 문구로 바꾼다.
fn camera_error(err: &JsValue) -> String {
    let name = Reflect::get(err, &"name".into())
        .ok()
        .and_then(|name| name.as_string())
        .unwrap_or_default();
    match name.as_str() {
        "NotAllowedError" | "SecurityError" => {
            "카메라·마이크 권한이 거부되었습니다. 브라우저 주소창의 사이트 설정에서 카메라와 마이크를 허용한 뒤 다시 시도해 주세요.".into()
        }
        "NotFoundError" | "OverconstrainedError" => {
            "사용할 수 있는 카메라나 마이크를 찾지 못했습니다.".into()
        }
        "NotReadableError" | "AbortError" => {
            "다른 앱이 카메라나 마이크를 쓰고 있습니다. 다른 앱을 닫고 다시 시도해 주세요.".into()
        }
        _ => format!("카메라·마이크를 켤 수 없습니다. ({name})"),
    }
}

/// 카메라와 마이크를 끈다.
pub fn stop_camera(stream: &MediaStream) {
    for track in stream.get_tracks() {
        track.unchecked_into::<MediaStreamTrack>().stop();
    }
}

/// 미리보기 `<video>`에 카메라 영상을 붙인다.
pub fn show_preview(video_id: &str, stream: Option<&MediaStream>) {
    if let Some(video) = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.get_element_by_id(video_id))
        .and_then(|element| element.dyn_into::<HtmlVideoElement>().ok())
    {
        // 송출자 자신의 마이크 소리가 스피커로 나오면 하울링이 생긴다.
        // `muted` 속성(attribute)은 이미 만들어진 요소의 실제 음소거 상태를 바꾸지 못하므로
        // 프로퍼티로 직접 끈다.
        video.set_muted(true);
        video.set_src_object(stream);
    }
}

/// 브라우저가 녹화할 수 있는 형식. 서버의 ffmpeg는 둘 다 받는다.
/// 안드로이드 Chrome은 webm, iOS Safari는 mp4를 쓴다.
fn recording_mime_type() -> Option<&'static str> {
    let window = web_sys::window()?;
    if !Reflect::has(&window, &"MediaRecorder".into()).unwrap_or(false) {
        return None;
    }
    [
        "video/webm;codecs=vp8,opus",
        "video/webm;codecs=vp9,opus",
        "video/webm",
        "video/mp4;codecs=avc1,mp4a",
        "video/mp4",
    ]
    .into_iter()
    .find(|mime| MediaRecorder::is_type_supported(mime))
}

/// 송출 중에 화면에 알릴 일.
#[derive(Debug)]
pub enum LiveEvent {
    /// 서버가 라이브 방송을 만들었고 녹화 조각을 보내기 시작했다.
    Started(Broadcast),
    /// 방송이 종료되었다. 서버가 먼저 끝냈다면 이유가 있다.
    Ended(Option<String>),
    /// 서버가 송출 시작을 거부했다.
    Rejected(String),
    /// 연결이 끊겼다.
    Disconnected,
}

/// 진행 중인 송출 하나. 버리면(`Drop`) 녹화를 멈추고 연결을 닫아 방송이 종료된다.
pub struct LiveSession {
    socket: WebSocket,
    recorder: MediaRecorder,
    _on_open: Closure<dyn FnMut()>,
    _on_message: Closure<dyn FnMut(MessageEvent)>,
    _on_close: Closure<dyn FnMut(CloseEvent)>,
    _on_data: Closure<dyn FnMut(BlobEvent)>,
    _on_stop: Closure<dyn FnMut()>,
}

impl LiveSession {
    /// 서버에 연결해 송출을 시작한다. 진행 상황은 `events`로 알린다.
    pub fn start(
        stream: &MediaStream,
        title: &str,
        events: UnboundedSender<LiveEvent>,
    ) -> Result<Self, String> {
        let mime_type = recording_mime_type().ok_or_else(unsupported)?;
        let options = MediaRecorderOptions::new();
        options.set_mime_type(mime_type);
        options.set_video_bits_per_second(VIDEO_BITS_PER_SECOND);
        options.set_audio_bits_per_second(AUDIO_BITS_PER_SECOND);
        let recorder =
            MediaRecorder::new_with_media_stream_and_media_recorder_options(stream, &options)
                .map_err(|_| "녹화를 준비할 수 없습니다.".to_owned())?;
        let socket = WebSocket::new(&api::ws_url(LIVE_INGEST_PATH))
            .map_err(|_| "서버에 연결할 수 없습니다.".to_owned())?;
        socket.set_binary_type(BinaryType::Arraybuffer);

        // 연결되면 송출 시작을 요청한다.
        let on_open = {
            let socket = socket.clone();
            let start = IngestClientMessage::Start {
                title: title.to_owned(),
                mime_type: mime_type.to_owned(),
            };
            Closure::<dyn FnMut()>::new(move || send_message(&socket, &start))
        };

        // 서버가 방송을 만들면 녹화를 시작한다.
        let on_message = {
            let recorder = recorder.clone();
            let events = events.clone();
            Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
                let Some(text) = event.data().as_string() else {
                    return;
                };
                let Ok(message) = serde_json::from_str::<IngestServerMessage>(&text) else {
                    return;
                };
                let event = match message {
                    IngestServerMessage::Started { broadcast } => {
                        if recorder.start_with_time_slice(CHUNK_MILLIS).is_err() {
                            LiveEvent::Rejected("녹화를 시작할 수 없습니다.".into())
                        } else {
                            LiveEvent::Started(broadcast)
                        }
                    }
                    IngestServerMessage::Ended { reason } => LiveEvent::Ended(reason),
                    IngestServerMessage::Error { message } => LiveEvent::Rejected(message),
                };
                let _ = events.unbounded_send(event);
            })
        };

        let on_close = {
            let recorder = recorder.clone();
            Closure::<dyn FnMut(CloseEvent)>::new(move |_: CloseEvent| {
                if recorder.state() != RecordingState::Inactive {
                    let _ = recorder.stop();
                }
                let _ = events.unbounded_send(LiveEvent::Disconnected);
            })
        };

        // 녹화 조각이 나올 때마다 그대로 서버로 보낸다.
        let on_data = {
            let socket = socket.clone();
            Closure::<dyn FnMut(BlobEvent)>::new(move |event: BlobEvent| {
                if let Some(blob) = event.data()
                    && blob.size() > 0.0
                    && socket.ready_state() == WebSocket::OPEN
                {
                    let _ = socket.send_with_blob(&blob);
                }
            })
        };

        // 녹화를 멈추면 마지막 조각이 나온 뒤에 이 이벤트가 온다. 그때 종료를 알린다.
        let on_stop = {
            let socket = socket.clone();
            Closure::<dyn FnMut()>::new(move || send_message(&socket, &IngestClientMessage::End))
        };

        socket.set_onopen(Some(on_open.as_ref().unchecked_ref()));
        socket.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
        socket.set_onclose(Some(on_close.as_ref().unchecked_ref()));
        recorder.set_ondataavailable(Some(on_data.as_ref().unchecked_ref()));
        recorder.set_onstop(Some(on_stop.as_ref().unchecked_ref()));

        Ok(Self {
            socket,
            recorder,
            _on_open: on_open,
            _on_message: on_message,
            _on_close: on_close,
            _on_data: on_data,
            _on_stop: on_stop,
        })
    }

    /// 송출을 끝낸다. 남은 조각을 보낸 뒤 서버에 종료를 알리고, 서버가 [`LiveEvent::Ended`]로 답한다.
    pub fn stop(&self) {
        if self.recorder.state() != RecordingState::Inactive {
            let _ = self.recorder.stop();
        } else {
            send_message(&self.socket, &IngestClientMessage::End);
        }
    }
}

impl Drop for LiveSession {
    fn drop(&mut self) {
        // 버린 클로저를 브라우저가 부르지 않도록 먼저 떼어 낸다.
        self.socket.set_onopen(None);
        self.socket.set_onmessage(None);
        self.socket.set_onclose(None);
        self.recorder.set_ondataavailable(None);
        self.recorder.set_onstop(None);
        if self.recorder.state() != RecordingState::Inactive {
            let _ = self.recorder.stop();
        }
        let _ = self.socket.close();
    }
}

fn send_message(socket: &WebSocket, message: &IngestClientMessage) {
    if socket.ready_state() == WebSocket::OPEN
        && let Ok(text) = serde_json::to_string(message)
    {
        let _ = socket.send_with_str(&text);
    }
}

fn object(entries: &[(&str, JsValue)]) -> Object {
    let object = Object::new();
    for (key, value) in entries {
        let _ = Reflect::set(&object, &JsValue::from_str(key), value);
    }
    object
}
