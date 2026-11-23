// One-time DNG/XMP SDK initialization.
#include "dng_bridge_internal.h"
#include "dng_xmp_sdk.h"

// Idempotent. A throw leaves the flag unset, so the next call retries.
void deptheos_dng_initialize(void) {
    static std::once_flag flag;
    std::call_once(flag, []() {
        #if qDNGUseXMP
        dng_xmp_sdk::InitializeSDK();
        #endif
    });
}

