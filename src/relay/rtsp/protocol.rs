use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum RouteStream {
    Main,
    Sub,
}

impl RouteStream {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            RouteStream::Main => "main",
            RouteStream::Sub => "sub",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct RouteKey {
    pub(crate) channel: u32,
    pub(crate) stream: RouteStream,
}

#[derive(Default)]
pub(crate) struct RtspRequest {
    pub(crate) method: String,
    pub(crate) url: String,
    pub(crate) headers: HashMap<String, String>,
}

pub(crate) fn parse_rtsp_request(input: &[u8]) -> Option<(RtspRequest, usize)> {
    let needle = b"\r\n\r\n";
    let end = input.windows(4).position(|w| w == needle)?;
    let head = String::from_utf8_lossy(&input[..end]).to_string();
    let mut lines = head.split("\r\n");
    let req_line = lines.next()?;
    let mut parts = req_line.split_whitespace();
    let method = parts.next()?.to_uppercase();
    let url = parts.next()?.to_string();
    let _ver = parts.next()?;
    let mut headers = HashMap::new();
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_lowercase(), v.trim().to_string());
        }
    }
    Some((RtspRequest { method, url, headers }, end + 4))
}

pub(crate) fn rtsp_response(status: &str, cseq: &str, headers: Vec<(&str, &str)>, body: Option<&str>) -> Vec<u8> {
    let mut out = format!("RTSP/1.0 {status}\r\nCSeq: {cseq}\r\nServer: HikvisionRtspBridge/1.0\r\n");
    for (k, v) in headers {
        out.push_str(&format!("{k}: {v}\r\n"));
    }
    if let Some(body) = body {
        out.push_str(&format!("Content-Length: {}\r\n\r\n{body}", body.len()));
    } else {
        out.push_str("\r\n");
    }
    out.into_bytes()
}

pub(crate) fn parse_track_id_from_url(url: &str) -> Option<u8> {
    let lower = url.to_lowercase();
    let key = "trackid=";
    let idx = lower.find(key)?;
    let rest = lower[idx + key.len()..].as_bytes();
    let mut acc: u32 = 0;
    let mut any = false;
    for &b in rest {
        if b.is_ascii_digit() {
            any = true;
            acc = acc.saturating_mul(10).saturating_add(u32::from(b - b'0'));
        } else {
            break;
        }
    }
    if !any || acc > u32::from(u8::MAX) {
        return None;
    }
    Some(acc as u8)
}

pub(crate) fn play_track_url(req_url: &str, track: u8) -> String {
    let base = req_url.trim_end_matches('/');
    let l = base.to_lowercase();
    if l.contains("trackid=") {
        base.to_string()
    } else {
        format!("{base}/trackID={track}")
    }
}

pub(crate) fn parse_route_key(url: &str, base_path: &str) -> Option<RouteKey> {
    let path = extract_path(url);
    let normalized_base = normalize_path(base_path);
    let normalized_path = normalize_path(path);
    let prefix = format!("{normalized_base}/");
    if !normalized_path.starts_with(&prefix) {
        return None;
    }
    let tail = &normalized_path[prefix.len()..];
    let mut segments = tail.split('/');
    let channel_raw = segments.next()?;
    let stream_raw = segments.next()?;
    if let Some(extra) = segments.next() {
        if !extra.to_ascii_lowercase().starts_with("trackid=") {
            return None;
        }
    }
    if segments.next().is_some() {
        return None;
    }
    let channel = channel_raw.parse::<u32>().ok()?;
    let stream = match stream_raw.to_ascii_lowercase().as_str() {
        "main" => RouteStream::Main,
        "sub" => RouteStream::Sub,
        _ => return None,
    };
    Some(RouteKey { channel, stream })
}

fn extract_path(url: &str) -> &str {
    let without_query = url.split('?').next().unwrap_or(url);
    if let Some(scheme_idx) = without_query.find("://") {
        let after_scheme = &without_query[scheme_idx + 3..];
        if let Some(path_idx) = after_scheme.find('/') {
            return &after_scheme[path_idx..];
        }
        return "/";
    }
    if without_query.starts_with('/') {
        return without_query;
    }
    "/"
}

fn normalize_path(path: &str) -> String {
    let mut out = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    while out.ends_with('/') && out.len() > 1 {
        out.pop();
    }
    out
}

