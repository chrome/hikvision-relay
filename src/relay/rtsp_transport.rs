#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransportSpec {
    pub rtp_channel: u8,
    pub rtcp_channel: u8,
}

pub fn parse_tcp_interleaved_transport(transport: &str) -> Option<TransportSpec> {
    let upper = transport.to_uppercase();
    if !upper.contains("RTP/AVP/TCP") {
        return None;
    }
    let lower = transport.to_lowercase();
    let idx = lower.find("interleaved=")?;
    let right = &lower[idx + "interleaved=".len()..];
    let pair = right.split(';').next().unwrap_or(right);
    let (a, b) = pair.split_once('-')?;
    let rtp_channel: u8 = a.parse().ok()?;
    let rtcp_channel: u8 = b.parse().ok()?;
    if rtcp_channel != rtp_channel.wrapping_add(1) {
        return None;
    }
    Some(TransportSpec {
        rtp_channel,
        rtcp_channel,
    })
}

