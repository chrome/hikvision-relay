#ifndef HIKVISION_RELAY_WRAPPER_H
#define HIKVISION_RELAY_WRAPPER_H

/*
 * Bindgen entrypoint. Includes the upstream Hikvision HCNetSDK header.
 *
 * The header is C++-flavoured (uses `extern "C"` and default arguments),
 * so we ask bindgen to invoke clang with C++ mode (-x c++) via build.rs.
 */

#if __has_include("HCNetSDK.h")
#include "HCNetSDK.h"
#elif __has_include("sdk/EN-HCNetSDKV6.1.9.4_build20220412_win64/incEn/HCNetSDK.h")
#include "sdk/EN-HCNetSDKV6.1.9.4_build20220412_win64/incEn/HCNetSDK.h"
#elif __has_include("sdk/EN-HCNetSDKV6.1.9.4_build20220412_win32/incEn/HCNetSDK.h")
#include "sdk/EN-HCNetSDKV6.1.9.4_build20220412_win32/incEn/HCNetSDK.h"
#elif __has_include("sdk/EN-HCNetSDKV6.1.9.4_build20220412_linux64/incEn/HCNetSDK.h")
#include "sdk/EN-HCNetSDKV6.1.9.4_build20220412_linux64/incEn/HCNetSDK.h"
#elif __has_include("sdk/EN-HCNetSDKV6.1.9.4_build20220412_linux32/incEn/HCNetSDK.h")
#include "sdk/EN-HCNetSDKV6.1.9.4_build20220412_linux32/incEn/HCNetSDK.h"
#else
#error "HCNetSDK.h not found in known sdk locations"
#endif

#endif /* HIKVISION_RELAY_WRAPPER_H */
