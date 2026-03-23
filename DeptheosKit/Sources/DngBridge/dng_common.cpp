#include "dng_bridge_internal.h"

// ---------------------------------------------------------------------------
// Logging
// ---------------------------------------------------------------------------

static DngLogCallback g_log_callback = nullptr;

void deptheos_set_dng_log_callback(DngLogCallback callback) {
    g_log_callback = callback;
}

void dng_log(DngLogLevel level, const char *format, ...) {
    if (!g_log_callback) return;
    char buffer[2048];
    va_list args;
    va_start(args, format);
    vsnprintf(buffer, sizeof(buffer), format, args);
    va_end(args);
    g_log_callback(level, buffer);
}

