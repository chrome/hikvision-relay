use std::mem::size_of;

use serde::Serialize;

use crate::config::ConnectionConfig;
use crate::error::{AppError, AppResult};
use crate::hikvision::client::{HikvisionClient, IpConfigRaw, IpConfigVersion, SdkMeta};
use crate::hikvision::ffi;

const NET_DVR_IPDEVINFO_V31_SIZE: usize = size_of::<ffi::NET_DVR_IPDEVINFO_V31>();
const NET_DVR_IPCHANINFO_SIZE: usize = size_of::<ffi::NET_DVR_IPCHANINFO>();
const MAX_ANALOG_CHANNUM: usize = ffi::MAX_ANALOG_CHANNUM as usize;
const MAX_IP_DEVICE: usize = ffi::MAX_IP_DEVICE as usize;
const MAX_IP_CHANNEL: usize = ffi::MAX_IP_CHANNEL as usize;

mod builders;
mod parsers;

use builders::{make_stream_item, map_stream_source_type};
use parsers::{ip_device_entry_from_v31, parse_ip_channel_entry, parse_ip_device_entry};

#[derive(Debug, Clone, Serialize)]
pub struct StreamItem {
    pub channel_no: usize,
    pub sdk_channel: usize,
    pub enabled: bool,
    pub stream_source_type: String,
    pub source: StreamSource,
}

#[derive(Debug, Clone, Serialize)]
pub struct StreamSource {
    pub ip_device_id: u16,
    pub source_channel: u8,
    pub transport_protocol: u8,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeviceSettings {
    pub sdk: super::hikvision::client::SdkMeta,
    pub config_read: ConfigRead,
    pub ip_devices: Vec<IpDeviceEntry>,
    pub ip_channels: Vec<IpChannelEntry>,
    pub enabled_ip_devices: Vec<IpDeviceEntry>,
    pub enabled_ip_channels: Vec<IpChannelEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConfigRead {
    pub version: String,
    pub bytes_returned: usize,
    pub raw_hex: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct IpDeviceEntry {
    pub device_index: usize,
    pub enabled: bool,
    pub protocol_type: u8,
    pub user_name: String,
    pub domain: String,
    pub ip_v4: String,
    pub port: u16,
    pub device_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct IpChannelEntry {
    pub channel_index: usize,
    pub channel_no: usize,
    pub enabled: bool,
    pub get_stream_enabled: bool,
    pub ip_device_id: u16,
    pub source_channel: u8,
    pub transport_protocol: u8,
}

#[derive(Debug, Clone, Serialize)]
pub struct ListStreamsResult {
    pub streams: Vec<StreamItem>,
    pub device_settings: DeviceSettings,
}

/// `byGetStreamType` in `NET_DVR_STREAM_MODE` (subset).
const GET_STREAM_FROM_DEVICE: u8 = 0;
const GET_STREAM_IP_DEVICE_V40: u8 = 6;

fn list_from_v40(
    cfg: &IpConfigRaw,
    sdk_meta: SdkMeta,
    include_raw_config: bool,
) -> AppResult<ListStreamsResult> {
    let need = size_of::<ffi::NET_DVR_IPPARACFG_V40>();
    if cfg.data.len() < need {
        return Err(AppError::Sdk(format!(
            "V40 buffer too small: got {} need {need}",
            cfg.data.len()
        )));
    }
    let v40 = unsafe { (cfg.data.as_ptr() as *const ffi::NET_DVR_IPPARACFG_V40).read_unaligned() };

    let mut ip_devices = Vec::with_capacity(MAX_IP_DEVICE);
    for i in 0..MAX_IP_DEVICE {
        ip_devices.push(ip_device_entry_from_v31(&v40.struIPDevInfo[i], i));
    }

    let dchan = v40.dwDChanNum.min(MAX_IP_CHANNEL as ffi::DWORD) as usize;
    let start_d = v40.dwStartDChan as usize;

    let mut ip_channels = Vec::with_capacity(dchan);
    let mut streams = Vec::new();

    for i in 0..dchan {
        let sm = v40.struStreamMode[i];
        let stream_type = sm.byGetStreamType;
        let (enabled, get_stream_ok, ip_dev_id, src_ch, trans, source_name) = match stream_type {
            GET_STREAM_FROM_DEVICE => unsafe {
                let ch = sm.uGetStream.struChanInfo;
                let ok = ch.byGetStream == 0;
                (
                    ch.byEnable == 1,
                    ok,
                    ch.byIPID as u16 + ((ch.byIPIDHigh as u16) << 8),
                    ch.byChannel,
                    ch.byTransProtocol,
                    map_stream_source_type(stream_type),
                )
            },
            GET_STREAM_IP_DEVICE_V40 => unsafe {
                let ch = sm.uGetStream.struIPChan;
                let ok = true;
                (
                    ch.byEnable == 1,
                    ok,
                    ch.wIPID,
                    ch.dwChannel.min(255) as u8,
                    ch.byTransProtocol,
                    map_stream_source_type(stream_type),
                )
            },
            _ => (false, false, 0, 0, 0, map_stream_source_type(stream_type)),
        };

        ip_channels.push(IpChannelEntry {
            channel_index: i + 1,
            channel_no: i + 1,
            enabled,
            get_stream_enabled: get_stream_ok,
            ip_device_id: ip_dev_id,
            source_channel: src_ch,
            transport_protocol: trans,
        });

        if enabled && get_stream_ok {
            streams.push(make_stream_item(
                i + 1,
                start_d + i,
                source_name,
                ip_dev_id,
                src_ch,
                trans,
            ));
        }
    }

    Ok(ListStreamsResult {
        streams,
        device_settings: DeviceSettings {
            sdk: sdk_meta,
            config_read: ConfigRead {
                version: String::from("v40"),
                bytes_returned: cfg.bytes_returned,
                raw_hex: include_raw_config.then(|| hex::encode(&cfg.data)),
            },
            enabled_ip_devices: ip_devices.iter().filter(|d| d.enabled).cloned().collect(),
            enabled_ip_channels: ip_channels.iter().filter(|c| c.enabled && c.get_stream_enabled).cloned().collect(),
            ip_devices: ip_devices,
            ip_channels: ip_channels,
        },
    })
}

pub async fn list_streams(
    conn: &ConnectionConfig,
    sdk_root_path: Option<&str>,
    include_raw_config: bool,
) -> AppResult<ListStreamsResult> {
    let mut sdk = HikvisionClient::new(sdk_root_path)?;
    sdk.init()?;
    sdk.login(conn)?;
    let cfg = sdk.get_ip_config_with_fallback()?;
    let sdk_meta = sdk.get_sdk_meta();

    if matches!(cfg.version, IpConfigVersion::V40) {
        return list_from_v40(&cfg, sdk_meta, include_raw_config);
    }

    let version_name = match cfg.version {
        IpConfigVersion::V31 => "v31",
        IpConfigVersion::Legacy => "legacy",
        IpConfigVersion::V40 => unreachable!("v40 handled above"),
    };

    let mut ip_devices = Vec::with_capacity(MAX_IP_DEVICE);
    for i in 0..MAX_IP_DEVICE {
        ip_devices.push(parse_ip_device_entry(&cfg.data, i));
    }
    let mut ip_channels = Vec::with_capacity(MAX_IP_CHANNEL);
    let mut streams = Vec::new();
    let start_digital_channel = sdk.start_digital_channel() as usize;
    for i in 0..MAX_IP_CHANNEL {
        let ch = parse_ip_channel_entry(&cfg.data, i);
        let enabled = ch.enabled && ch.get_stream_enabled;
        if enabled {
            streams.push(make_stream_item(
                i + 1,
                start_digital_channel + i,
                String::from("ipDevice"),
                ch.ip_device_id,
                ch.source_channel,
                ch.transport_protocol,
            ));
        }
        ip_channels.push(ch);
    }

    Ok(ListStreamsResult {
        streams,
        device_settings: DeviceSettings {
            sdk: sdk_meta,
            config_read: ConfigRead {
                version: version_name.to_string(),
                bytes_returned: cfg.bytes_returned,
                raw_hex: include_raw_config.then(|| hex::encode(&cfg.data)),
            },
            enabled_ip_devices: ip_devices.iter().filter(|d| d.enabled).cloned().collect(),
            enabled_ip_channels: ip_channels.iter().filter(|c| c.enabled && c.get_stream_enabled).cloned().collect(),
            ip_devices: ip_devices,
            ip_channels: ip_channels,
        },
    })
}
