.PHONY: help ensure-bindgen bindings bindings-win64 bindings-linux64 bindings-linux32 build build-release clean

SHELL := /usr/bin/env bash

WRAPPER := wrapper.h
GEN_DIR := src/hikvision/generated

WIN64_SDK_INC := sdk/EN-HCNetSDKV6.1.9.4_build20220412_win64/incEn
LINUX64_SDK_INC := sdk/EN-HCNetSDKV6.1.9.4_build20220412_linux64/incEn
LINUX32_SDK_INC := sdk/EN-HCNetSDKV6.1.9.4_build20220412_linux32/incEn

BINDGEN := bindgen

ALLOWLIST_FUNCTIONS := \
	--allowlist-function NET_DVR_Init \
	--allowlist-function NET_DVR_Cleanup \
	--allowlist-function NET_DVR_GetLastError \
	--allowlist-function NET_DVR_SetConnectTime \
	--allowlist-function NET_DVR_Login_V40 \
	--allowlist-function NET_DVR_Logout \
	--allowlist-function NET_DVR_GetSDKVersion \
	--allowlist-function NET_DVR_GetSDKBuildVersion \
	--allowlist-function NET_DVR_GetDVRConfig \
	--allowlist-function NET_DVR_RealPlay_V40 \
	--allowlist-function NET_DVR_StopRealPlay \
	--allowlist-function NET_DVR_SetStandardDataCallBack

ALLOWLIST_TYPES := \
	--allowlist-type NET_DVR_USER_LOGIN_INFO \
	--allowlist-type NET_DVR_DEVICEINFO_V40 \
	--allowlist-type NET_DVR_DEVICEINFO_V30 \
	--allowlist-type NET_DVR_PREVIEWINFO \
	--allowlist-type NET_DVR_IPPARACFG_V40 \
	--allowlist-type NET_DVR_IPPARACFG_V31 \
	--allowlist-type NET_DVR_IPPARACFG \
	--allowlist-type NET_DVR_IPCHANINFO \
	--allowlist-type NET_DVR_IPCHANINFO_V40 \
	--allowlist-type NET_DVR_IPDEVINFO_V31 \
	--allowlist-type NET_DVR_IPDEVINFO \
	--allowlist-type NET_DVR_STREAM_MODE \
	--allowlist-type REALDATACALLBACK

ALLOWLIST_VARS := \
	--allowlist-var NET_DVR_GET_IPPARACFG \
	--allowlist-var NET_DVR_GET_IPPARACFG_V31 \
	--allowlist-var NET_DVR_GET_IPPARACFG_V40 \
	--allowlist-var NET_DVR_PARAMETER_ERROR \
	--allowlist-var NET_DVR_SYSHEAD \
	--allowlist-var NET_DVR_STREAMDATA \
	--allowlist-var MAX_ANALOG_CHANNUM \
	--allowlist-var MAX_IP_DEVICE \
	--allowlist-var MAX_IP_CHANNEL

COMMON_BINDGEN_FLAGS := \
	--dynamic-loading HCNetSdkLib \
	--no-layout-tests \
	--no-doc-comments \
	--with-derive-default \
	--no-derive-debug \
	--no-prepend-enum-name \
	$(ALLOWLIST_FUNCTIONS) \
	$(ALLOWLIST_TYPES) \
	$(ALLOWLIST_VARS)

help:
	@echo "Targets:"
	@echo "  make bindings         - regenerate all committed bindings"
	@echo "  make bindings-win64   - regenerate Windows x86_64 bindings"
	@echo "  make bindings-linux64 - regenerate Linux x86_64 bindings"
	@echo "  make bindings-linux32 - regenerate Linux x86 bindings"
	@echo "  make build            - debug build"
	@echo "  make build-release    - release build"
	@echo "  make clean            - cargo clean"

ensure-bindgen:
	@command -v $(BINDGEN) >/dev/null 2>&1 || { \
		echo "bindgen CLI not found; installing..."; \
		cargo install bindgen-cli; \
	}

bindings: bindings-win64 bindings-linux64 bindings-linux32

bindings-win64: ensure-bindgen
	@mkdir -p "$(GEN_DIR)"
	$(BINDGEN) "$(WRAPPER)" \
		$(COMMON_BINDGEN_FLAGS) \
		-o "$(GEN_DIR)/windows_x86_64.rs" \
		-- \
		-I$(WIN64_SDK_INC) \
		-x c++ \
		-std=c++14 \
		-DWIN32 \
		-D_WIN32 \
		-D_MSC_VER=1929

bindings-linux64: ensure-bindgen
	@mkdir -p "$(GEN_DIR)"
	$(BINDGEN) "$(WRAPPER)" \
		$(COMMON_BINDGEN_FLAGS) \
		-o "$(GEN_DIR)/linux_x86_64.rs" \
		-- \
		-I$(LINUX64_SDK_INC) \
		-x c++ \
		-std=c++14 \
		-D__linux__ \
		-D_GNU_SOURCE \
		-D__GNUC__=11

bindings-linux32: ensure-bindgen
	@mkdir -p "$(GEN_DIR)"
	$(BINDGEN) "$(WRAPPER)" \
		$(COMMON_BINDGEN_FLAGS) \
		-o "$(GEN_DIR)/linux_x86.rs" \
		-- \
		-I$(LINUX32_SDK_INC) \
		-x c++ \
		-std=c++14 \
		-D__linux__ \
		-D_GNU_SOURCE \
		-D__GNUC__=11

build:
	cargo build

build-release:
	cargo build --release

clean:
	cargo clean
