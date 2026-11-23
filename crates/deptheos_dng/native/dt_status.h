// Why a call into the DNG SDK or libjxl failed (dng_c_api.h, jxl_c_api.h),
// for the Rust side to say (src/lib.rs's `Cause`). An entry point that
// takes one fills it in when it catches what the call threw (guard.h); a
// call that fails without throwing leaves it as it was. May be NULL.

#pragma once

#include <stdint.h>

typedef struct DtStatus {
    // A dng_error_code (dng_errors.h): dng_error_memory for std::bad_alloc
    // too, dng_error_unknown for any other exception. 0 until filled in.
    int32_t code;
    // For the log, not the user: a standard exception's what(), cut short
    // and NUL-terminated; empty for the SDK's own exceptions, which carry
    // only their code.
    char message[120];
} DtStatus;

// The bridge's own code, past the SDK's: a DNG whose header claims more
// pixels than a read takes (dt_dng_read_into's `max_pixels`).
#define DT_ERROR_TOO_LARGE 200001
