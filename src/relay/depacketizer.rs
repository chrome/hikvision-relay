#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoCodec {
    H264,
    H265,
}

#[derive(Debug, Default, Clone)]
pub struct DepacketizerStats {
    pub packets: usize,
    pub malformed: usize,
    pub seq_gaps: usize,
    pub nals_out: usize,
}

pub struct RtpVideoDepacketizer {
    fu_buffer: Option<Vec<u8>>,
    last_video_seq: Option<u16>,
    pub locked_video_pt: Option<u8>,
    pub locked_video_ssrc: Option<u32>,
    pub locked_audio_pt: Option<u8>,
    pub locked_audio_ssrc: Option<u32>,
    pub video_codec: Option<VideoCodec>,
    pub stats: DepacketizerStats,
}

impl RtpVideoDepacketizer {
    pub fn new() -> Self {
        Self {
            fu_buffer: None,
            last_video_seq: None,
            locked_video_pt: None,
            locked_video_ssrc: None,
            locked_audio_pt: None,
            locked_audio_ssrc: None,
            video_codec: None,
            stats: DepacketizerStats::default(),
        }
    }

    pub fn feed_rtp_packet(&mut self, packet: &[u8]) -> (Vec<Vec<u8>>, Option<AudioRtp>) {
        self.stats.packets += 1;
        let parsed = match parse_rtp(packet) {
            Some(p) => p,
            None => {
                self.stats.malformed += 1;
                return (Vec::new(), None);
            }
        };

        if self.locked_audio_pt.is_none() && is_audio_pt(parsed.pt) {
            self.locked_audio_pt = Some(parsed.pt);
            self.locked_audio_ssrc = Some(parsed.ssrc);
        }
        if self.locked_audio_pt == Some(parsed.pt) && self.locked_audio_ssrc == Some(parsed.ssrc) {
            return (
                Vec::new(),
                Some(AudioRtp {
                    payload: parsed.payload.to_vec(),
                    timestamp: parsed.timestamp,
                    marker: parsed.marker,
                }),
            );
        }

        if self.locked_video_pt.is_none() {
            if let Some(codec) = detect_codec(parsed.payload.first().copied().unwrap_or_default()) {
                self.locked_video_pt = Some(parsed.pt);
                self.locked_video_ssrc = Some(parsed.ssrc);
                self.video_codec = Some(codec);
            } else {
                return (Vec::new(), None);
            }
        }
        if self.locked_video_pt != Some(parsed.pt) || self.locked_video_ssrc != Some(parsed.ssrc) {
            return (Vec::new(), None);
        }
        if let Some(prev) = self.last_video_seq {
            if parsed.sequence != prev.wrapping_add(1) {
                self.stats.seq_gaps += 1;
                self.fu_buffer = None;
            }
        }
        self.last_video_seq = Some(parsed.sequence);

        let out = match self.video_codec.unwrap_or(VideoCodec::H264) {
            VideoCodec::H264 => depack_h264(parsed.payload, &mut self.fu_buffer),
            VideoCodec::H265 => depack_h265(parsed.payload, &mut self.fu_buffer),
        };
        self.stats.nals_out += out.len();
        (out, None)
    }
}

pub struct AudioRtp {
    pub payload: Vec<u8>,
    pub timestamp: u32,
    pub marker: bool,
}

struct ParsedRtp<'a> {
    marker: bool,
    pt: u8,
    sequence: u16,
    timestamp: u32,
    ssrc: u32,
    payload: &'a [u8],
}

fn parse_rtp(packet: &[u8]) -> Option<ParsedRtp<'_>> {
    if packet.len() < 12 {
        return None;
    }
    let b0 = packet[0];
    if (b0 >> 6) != 2 {
        return None;
    }
    let has_padding = (b0 & 0x20) != 0;
    let cc = (b0 & 0x0f) as usize;
    let extension = (b0 & 0x10) != 0;
    let b1 = packet[1];
    let marker = (b1 & 0x80) != 0;
    let pt = b1 & 0x7f;
    let seq = u16::from_be_bytes([packet[2], packet[3]]);
    let ts = u32::from_be_bytes([packet[4], packet[5], packet[6], packet[7]]);
    let ssrc = u32::from_be_bytes([packet[8], packet[9], packet[10], packet[11]]);
    let mut start = 12 + cc * 4;
    if extension {
        if packet.len() < start + 4 {
            return None;
        }
        let ext_len_words = u16::from_be_bytes([packet[start + 2], packet[start + 3]]) as usize;
        start += 4 + ext_len_words * 4;
    }
    if packet.len() <= start {
        return None;
    }
    let mut end = packet.len();
    if has_padding {
        let pad_len = *packet.last()? as usize;
        if pad_len == 0 || pad_len > packet.len().saturating_sub(start) {
            return None;
        }
        end = packet.len() - pad_len;
    }
    if end <= start {
        return None;
    }
    Some(ParsedRtp {
        marker,
        pt,
        sequence: seq,
        timestamp: ts,
        ssrc,
        payload: &packet[start..end],
    })
}

fn detect_codec(b: u8) -> Option<VideoCodec> {
    let h264 = b & 0x1f;
    let h265 = (b >> 1) & 0x3f;
    if matches!(h265, 1 | 19 | 20 | 21 | 32 | 33 | 34 | 35 | 39 | 40 | 48 | 49) {
        return Some(VideoCodec::H265);
    }
    if matches!(h264, 1 | 5 | 6 | 7 | 8 | 9 | 24 | 28 | 29) {
        return Some(VideoCodec::H264);
    }
    None
}

fn depack_h264(payload: &[u8], fu_buffer: &mut Option<Vec<u8>>) -> Vec<Vec<u8>> {
    if payload.is_empty() {
        return Vec::new();
    }
    let ntype = payload[0] & 0x1f;
    if (1..=23).contains(&ntype) {
        return vec![payload.to_vec()];
    }
    if ntype == 24 {
        let mut out = Vec::new();
        let mut off = 1;
        while off + 2 <= payload.len() {
            let len = u16::from_be_bytes([payload[off], payload[off + 1]]) as usize;
            off += 2;
            if len == 0 || off + len > payload.len() {
                break;
            }
            out.push(payload[off..off + len].to_vec());
            off += len;
        }
        return out;
    }
    if ntype == 28 || ntype == 29 {
        if payload.len() < 2 {
            return Vec::new();
        }
        let fu_header = payload[1];
        let start = (fu_header & 0x80) != 0;
        let end = (fu_header & 0x40) != 0;
        let orig_type = fu_header & 0x1f;
        let data_off = if ntype == 29 { 4 } else { 2 };
        if payload.len() < data_off {
            return Vec::new();
        }
        if start {
            let h = (payload[0] & 0xe0) | orig_type;
            *fu_buffer = Some([vec![h], payload[data_off..].to_vec()].concat());
        } else if let Some(buf) = fu_buffer {
            buf.extend_from_slice(&payload[data_off..]);
        } else {
            return Vec::new();
        }
        if end {
            if let Some(buf) = fu_buffer.take() {
                return vec![buf];
            }
        }
    }
    Vec::new()
}

fn depack_h265(payload: &[u8], fu_buffer: &mut Option<Vec<u8>>) -> Vec<Vec<u8>> {
    if payload.len() < 2 {
        return Vec::new();
    }
    let ntype = (payload[0] >> 1) & 0x3f;
    if ntype <= 47 {
        return vec![payload.to_vec()];
    }
    if ntype == 48 {
        let mut out = Vec::new();
        let mut off = 2;
        while off + 2 <= payload.len() {
            let len = u16::from_be_bytes([payload[off], payload[off + 1]]) as usize;
            off += 2;
            if len == 0 || off + len > payload.len() {
                break;
            }
            out.push(payload[off..off + len].to_vec());
            off += len;
        }
        return out;
    }
    if ntype == 49 {
        if payload.len() < 3 {
            return Vec::new();
        }
        let fu_header = payload[2];
        let start = (fu_header & 0x80) != 0;
        let end = (fu_header & 0x40) != 0;
        let orig_type = fu_header & 0x3f;
        if start {
            let b0 = (payload[0] & 0x81) | ((orig_type & 0x3f) << 1);
            let b1 = payload[1];
            *fu_buffer = Some([vec![b0, b1], payload[3..].to_vec()].concat());
        } else if let Some(buf) = fu_buffer {
            buf.extend_from_slice(&payload[3..]);
        } else {
            return Vec::new();
        }
        if end {
            if let Some(buf) = fu_buffer.take() {
                return vec![buf];
            }
        }
    }
    Vec::new()
}

fn is_audio_pt(pt: u8) -> bool {
    matches!(pt, 0 | 3 | 4 | 5 | 6 | 7 | 8 | 9 | 10 | 11 | 12 | 13 | 14 | 15 | 16 | 17 | 18)
}

