//! HTTP `Range` 헤더 해석. 단일 구간(`bytes=a-b`, `bytes=a-`, `bytes=-n`)만 지원한다.

use crate::storage::ByteRange;

pub enum RangeRequest {
    /// Range 헤더가 없거나 해석할 수 없어 전체를 보낸다.
    Full,
    Partial(ByteRange),
    /// 구간이 녹화본 크기를 벗어났다(416).
    Unsatisfiable,
}

pub fn parse(header: Option<&str>, size: u64) -> RangeRequest {
    let Some(spec) = header.and_then(|h| h.trim().strip_prefix("bytes=")) else {
        return RangeRequest::Full;
    };
    // 여러 구간 요청은 지원하지 않으므로 전체를 보낸다(RFC 9110 허용).
    if spec.contains(',') {
        return RangeRequest::Full;
    }
    let Some((start, end)) = spec.split_once('-') else {
        return RangeRequest::Full;
    };
    let (start, end) = (start.trim(), end.trim());

    let (first, last) = match (start.is_empty(), end.is_empty()) {
        // bytes=-n: 마지막 n바이트
        (true, false) => match end.parse::<u64>() {
            Ok(0) | Err(_) => return RangeRequest::Unsatisfiable,
            Ok(_) if size == 0 => return RangeRequest::Unsatisfiable,
            Ok(n) => (size.saturating_sub(n), size - 1),
        },
        // bytes=a-
        (false, true) => match start.parse::<u64>() {
            Ok(a) => (a, size.saturating_sub(1)),
            Err(_) => return RangeRequest::Full,
        },
        // bytes=a-b
        (false, false) => match (start.parse::<u64>(), end.parse::<u64>()) {
            (Ok(a), Ok(b)) if a <= b => (a, b.min(size.saturating_sub(1))),
            _ => return RangeRequest::Full,
        },
        (true, true) => return RangeRequest::Full,
    };

    if first >= size || first > last {
        return RangeRequest::Unsatisfiable;
    }
    RangeRequest::Partial(ByteRange { start: first, len: last - first + 1 })
}
