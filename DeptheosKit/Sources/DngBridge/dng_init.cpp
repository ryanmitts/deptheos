//
//  dng_init.cpp
//  DeptheosKit
//
//  Created by Ryan Mitts on 2026-03-22.
//
#include "dng_bridge_internal.h"
#include "dng_xmp_sdk.h"

void deptheos_dng_initialize(void) {
    static std::once_flag flag;
    std::call_once(flag, []() {
        #if qDNGUseXMP
        dng_xmp_sdk::InitializeSDK();
        #endif
    });
}

