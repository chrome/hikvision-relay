use crate::hikvision::ffi;
use crate::list_streams::{
    IpChannelEntry, IpDeviceEntry, MAX_ANALOG_CHANNUM, MAX_IP_DEVICE, NET_DVR_IPCHANINFO_SIZE,
    NET_DVR_IPDEVINFO_V31_SIZE,
};

pub(super) fn read_cstr(raw: &[u8], offset: usize, len: usize) -> String {
    if offset + len > raw.len() {
        return String::new();
    }
    let s = &raw[offset..offset + len];
    let nul = s.iter().position(|b| *b == 0).unwrap_or(s.len());
    String::from_utf8_lossy(&s[..nul]).to_string()
}

pub(super) fn parse_ip_device_entry(raw: &[u8], index: usize) -> IpDeviceEntry {
    let base = 4 + index * NET_DVR_IPDEVINFO_V31_SIZE;
    let enabled = raw.get(base).copied().unwrap_or_default() == 1;
    let protocol_type = raw.get(base + 1).copied().unwrap_or_default();
    let user_name = read_cstr(raw, base + 4, 32);
    let domain = read_cstr(raw, base + 52, 64);
    let ip_v4 = read_cstr(raw, base + 116, 16);
    let port = if base + 262 <= raw.len() {
        u16::from_le_bytes([raw[base + 260], raw[base + 261]])
    } else {
        0
    };
    let device_id = read_cstr(raw, base + 262, 32);
    IpDeviceEntry {
        device_index: index + 1,
        enabled,
        protocol_type,
        user_name,
        domain,
        ip_v4,
        port,
        device_id,
    }
}

pub(super) fn parse_ip_channel_entry(raw: &[u8], index: usize) -> IpChannelEntry {
    let base =
        4 + NET_DVR_IPDEVINFO_V31_SIZE * MAX_IP_DEVICE + MAX_ANALOG_CHANNUM + index * NET_DVR_IPCHANINFO_SIZE;
    let by_enable = raw.get(base).copied().unwrap_or_default();
    let by_ipid = raw.get(base + 1).copied().unwrap_or_default();
    let by_channel = raw.get(base + 2).copied().unwrap_or_default();
    let by_ipid_high = raw.get(base + 3).copied().unwrap_or_default();
    let by_trans = raw.get(base + 4).copied().unwrap_or_default();
    let by_get_stream = raw.get(base + 5).copied().unwrap_or_default();
    IpChannelEntry {
        channel_index: index + 1,
        channel_no: index + 1,
        enabled: by_enable == 1,
        get_stream_enabled: by_get_stream == 0,
        ip_device_id: by_ipid as u16 + ((by_ipid_high as u16) << 8),
        source_channel: by_channel,
        transport_protocol: by_trans,
    }
}

fn byte_cstr(arr: &[u8]) -> String {
    let n = arr.iter().position(|b| *b == 0).unwrap_or(arr.len());
    String::from_utf8_lossy(&arr[..n]).to_string()
}

pub(super) fn ip_device_entry_from_v31(dev: &ffi::NET_DVR_IPDEVINFO_V31, index: usize) -> IpDeviceEntry {
    let sip: &[u8] = unsafe {
        std::slice::from_raw_parts(dev.struIP.sIpV4.as_ptr() as *const u8, dev.struIP.sIpV4.len())
    };
    IpDeviceEntry {
        device_index: index + 1,
        enabled: dev.byEnable == 1,
        protocol_type: dev.byProType,
        user_name: byte_cstr(&dev.sUserName),
        domain: byte_cstr(&dev.byDomain),
        ip_v4: byte_cstr(sip),
        port: dev.wDVRPort,
        device_id: byte_cstr(&dev.szDeviceID),
    }
}
