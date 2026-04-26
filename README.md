# hikvision-relay

`hikvision-relay` is a CLI utility for working with Hikvision cameras and NVRs through Hikvision HCNetSDK.

It can list available device channels and streams, and it can also run a local RTSP relay that exposes Hikvision video streams through simple RTSP routes.

## Features

- Connect to Hikvision cameras and NVRs through HCNetSDK.
- List available digital channels and stream metadata.
- Output stream information as human-readable text or JSON.
- Run a local RTSP relay server.
- Expose relay routes in the following format:

  ```text
  /live/<channel>/<main|sub>
  ```

- Create SDK streaming pipelines lazily when RTSP clients connect.
- Stop idle pipelines automatically after clients disconnect.
- Optionally restart failed pipelines on recoverable stream errors.

## Requirements

This project requires the Hikvision HCNetSDK package for your platform.

The SDK is not included in this repository. Download it from official Hikvision sources and make sure you comply with Hikvision licensing terms.

Useful links:

- [Hikvision SDK downloads](https://www.hikvision.com/us-en/support/download/sdk/)
- [Hikvision Materials License Agreement](https://www.hikvision.com/en/policies/materials-license-agreement/)

Expected SDK package layout:

```text
<SDK_ROOT>/
├── incEn/
└── lib/
```

Example SDK package names:

```text
EN-HCNetSDKV6.1.9.4_build20220412_win64
EN-HCNetSDKV6.1.9.4_build20220412_linux64
EN-HCNetSDKV6.1.9.4_build20220412_linux32
```

## Installation

Download a prebuilt binary from the project releases:

[GitHub Releases](https://github.com/chrome/hikvision-relay/releases)

Available binary names may include:

```text
hikvision
hikvision.exe
```

You also need to install or unpack the Hikvision HCNetSDK package separately.

## SDK setup

The application needs to know where the Hikvision SDK is located.

You can provide the SDK path using either:

- the `--sdk-path` CLI option;
- or the `HIKVISION_SDK_PATH` environment variable.

Examples:

### Windows

```powershell
$env:HIKVISION_SDK_PATH="D:\sdk\EN-HCNetSDKV6.1.9.4_build20220412_win64"
```

### Linux

```bash
export HIKVISION_SDK_PATH=/opt/hikvision/EN-HCNetSDKV6.1.9.4_build20220412_linux64
```

The SDK root must contain both `incEn/` and `lib/`.

## Usage

### List channels and streams

```bash
hikvision \
  --host 192.168.1.64 \
  --port 8000 \
  --user admin \
  --password "your_password"
```

By default, the application connects to the device and prints available channels and streams.

To force JSON output:

```bash
hikvision \
  --host 192.168.1.64 \
  --port 8000 \
  --user admin \
  --password "your_password" \
  --json
```

### Run RTSP relay

```bash
hikvision \
  --host 192.168.1.64 \
  --port 8000 \
  --user admin \
  --password "your_password" \
  --rtsp-relay
```

By default, the relay listens on:

```text
rtsp://127.0.0.1:8554
```

Stream URLs use the following format:

```text
rtsp://127.0.0.1:8554/live/<channel>/<main|sub>
```

Example:

```text
rtsp://127.0.0.1:8554/live/1/main
rtsp://127.0.0.1:8554/live/1/sub
```

## CLI options

### Device connection

| Option | Description | Default |
| --- | --- | --- |
| `--host <HOST>` | Camera, DVR, or NVR IP address | `127.0.0.1` |
| `--port <PORT>` | Hikvision SDK port | `8000` |
| `--user <USER>` | Device username | `admin` |
| `--password <PASSWORD>` | Device password | empty |

### SDK

| Option | Description |
| --- | --- |
| `--sdk-path <SDK_PATH>` | Path to the Hikvision SDK root. Must contain `incEn/` and `lib/`. |
| `HIKVISION_SDK_PATH` | Environment variable alternative to `--sdk-path`. |

### Output

| Option | Description |
| --- | --- |
| `--json` | Output list mode result as JSON. |
| `--include-raw-config` | Include raw device configuration blob in list output. |
| `--verbose` | Enable detailed diagnostic logs. |

### RTSP relay

| Option | Description | Default |
| --- | --- | --- |
| `--rtsp-relay` | Run embedded RTSP relay server | disabled |
| `--listen-host <LISTEN_HOST>` | Relay bind host | `127.0.0.1` |
| `--listen-port <LISTEN_PORT>` | Relay bind port | `8554` |
| `--listen-path <LISTEN_PATH>` | Relay route prefix | `/live` |
| `--probe-timeout-ms <PROBE_TIMEOUT_MS>` | Time to wait for first stream data before watchdog restart | `15000` |
| `--stale-after-ms <STALE_AFTER_MS>` | No-data timeout before watchdog restart | `10000` |
| `--idle-grace-ms <IDLE_GRACE_MS>` | Delay before stopping an idle route pipeline | `5000` |
| `--restart-on-fail` | Enable automatic pipeline restart on recoverable failures | disabled |
| `--max-restarts <MAX_RESTARTS>` | Maximum automatic restarts per route | `5` |

## Output

### List mode

In list mode, the application prints detected device channels, stream information, and selected device settings.

Use `--json` for machine-readable output.

### Relay mode

In relay mode, the application prints RTSP URL hints, for example:

```text
rtsp://127.0.0.1:8554/live/<channel>/<main|sub>
```

Streaming pipelines are started only when an RTSP client connects to a route.

## Troubleshooting

### `SDK library not found`

Check that `HIKVISION_SDK_PATH` or `--sdk-path` points to the SDK root directory.

Verify that the SDK library exists:

#### Windows

```text
lib/HCNetSDK.dll
```

#### Linux

```text
lib/libhcnetsdk.so
```

### SDK loads, but fails at runtime

Make sure all runtime dependency files from the vendor SDK `lib/` directory are present.

HCNetSDK usually depends on multiple additional libraries shipped with the SDK package. Copying only `HCNetSDK.dll` or `libhcnetsdk.so` may not be enough.

### Device connection fails

Check the following:

- the device IP address is correct;
- the SDK port is correct, usually `8000`;
- the username and password are valid;
- the device allows SDK access;
- your firewall allows connections to the device SDK port.

### RTSP relay starts, but no video appears

Try the following:

- verify that list mode detects the expected channels;
- try both `main` and `sub` streams;
- enable `--verbose`;
- increase `--probe-timeout-ms`;
- enable `--restart-on-fail`.

Example:

```bash
hikvision \
  --host 192.168.1.64 \
  --user admin \
  --password "your_password" \
  --rtsp-relay \
  --verbose \
  --restart-on-fail
```

## Developer guide

### Build from source

Requirements:

- Rust stable toolchain;
- Cargo;
- Hikvision HCNetSDK headers for binding generation;
- optional: GNU Make and Bash for Makefile shortcuts.

Build debug version:

```bash
cargo build
```

Build release version:

```bash
cargo build --release
```

Makefile shortcuts:

```bash
make build
make build-release
make clean
```

## Bindings

Generated bindings are committed to the repository:

```text
src/hikvision/generated/windows_x86_64.rs
src/hikvision/generated/linux_x86_64.rs
src/hikvision/generated/linux_x86.rs
```

### Regenerate all bindings

```bash
make bindings
```

### Regenerate bindings for a specific target

```bash
make bindings-win64
make bindings-linux64
make bindings-linux32
```

Binding generation requirements:

- `bindgen` CLI available in `PATH`;
- `libclang` installed and discoverable;
- SDK include directories available as configured in the `Makefile`.

The Makefile can install `bindgen-cli` automatically if it is missing.
