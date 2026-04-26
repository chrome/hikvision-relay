use crate::list_streams::{StreamItem, StreamSource};

pub(super) fn map_stream_source_type(by: u8) -> String {
    match by {
        0 => String::from("ipDevice"),
        1 => String::from("streamMedia"),
        2 => String::from("ipServer"),
        3 => String::from("ddnsStream"),
        4 => String::from("streamMediaUrl"),
        5 => String::from("hkDdns"),
        6 => String::from("ipDeviceV40"),
        7 => String::from("rtsp"),
        _ => format!("unknown:{by}"),
    }
}

pub(super) fn make_stream_item(
    channel_no: usize,
    sdk_channel: usize,
    source_name: String,
    ip_device_id: u16,
    source_channel: u8,
    transport_protocol: u8,
) -> StreamItem {
    StreamItem {
        channel_no,
        sdk_channel,
        enabled: true,
        stream_source_type: source_name,
        source: StreamSource {
            ip_device_id,
            source_channel,
            transport_protocol,
        },
    }
}
