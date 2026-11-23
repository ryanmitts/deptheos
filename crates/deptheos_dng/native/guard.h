// The exception barrier. The DNG SDK, XMP and libjxl report errors by
// throwing, and a C++ exception unwinding out of an extern "C" function
// into Rust aborts the process, so every function here that reaches them
// runs through `guarded`.
#pragma once

#include "dng_bridge.h"
#include "dng_exceptions.h"
#include "dt_status.h"

#include <cstdio>
#include <exception>
#include <new>
#include <utility>

// The codes src/lib.rs's `Cause` maps, as the SDK numbers them.
static_assert(dng_error_unknown == 100000, "dng_error_code changed");
static_assert(dng_error_not_yet_implemented == 100001, "dng_error_code changed");
static_assert(dng_error_user_canceled == 100003, "dng_error_code changed");
static_assert(dng_error_memory == 100005, "dng_error_code changed");
static_assert(dng_error_bad_format == 100006, "dng_error_code changed");
static_assert(dng_error_end_of_file == 100011, "dng_error_code changed");
static_assert(dng_error_file_is_damaged == 100012, "dng_error_code changed");
static_assert(dng_error_unsupported_dng == 100015, "dng_error_code changed");
static_assert(dng_error_jxl_decoder == 100018, "dng_error_code changed");

// Runs `body` and returns its result, or `fallback` if it throws, logging
// the exception as "[what] ..." with `subject` (a path, or nullptr)
// appended and saying in `status` (may be NULL) what it was.
template <typename R, typename Body>
R guarded_or(R fallback, const char *what, const char *subject, DtStatus *status, Body &&body) {
    const char *sep = subject ? " for " : "";
    if (!subject) subject = "";
    const auto fail = [status](int32_t code, const char *message) {
        if (status) {
            status->code = code;
            std::snprintf(status->message, sizeof status->message, "%s", message);
        }
    };
    try {
        return body();
    } catch (const dng_exception &e) {
        fail(e.ErrorCode(), "");
        // A cancel is no failure: the caller asked for it.
        if (e.ErrorCode() == dng_error_user_canceled) {
            dng_log(DNG_LOG_DEBUG, "[%s] canceled%s%s", what, sep, subject);
            return fallback;
        }
        dng_log(DNG_LOG_ERROR, "[%s] DNG exception: %d%s%s", what, (int)e.ErrorCode(), sep, subject);
    } catch (const std::bad_alloc &e) {
        fail(dng_error_memory, e.what());
        dng_log(DNG_LOG_ERROR, "[%s] Out of memory%s%s", what, sep, subject);
    } catch (const std::exception &e) {
        fail(dng_error_unknown, e.what());
        dng_log(DNG_LOG_ERROR, "[%s] Standard exception: %s%s%s", what, e.what(), sep, subject);
    } catch (...) {
        fail(dng_error_unknown, "unknown exception");
        dng_log(DNG_LOG_ERROR, "[%s] Unknown exception%s%s", what, sep, subject);
    }
    return fallback;
}

// `guarded_or` with `R{}` (nullptr, 0, std::nullopt, false) on a throw.
template <typename R, typename Body>
R guarded(const char *what, const char *subject, DtStatus *status, Body &&body) {
    return guarded_or<R>(R{}, what, subject, status, std::forward<Body>(body));
}

template <typename R, typename Body>
R guarded(const char *what, DtStatus *status, Body &&body) {
    return guarded<R>(what, nullptr, status, std::forward<Body>(body));
}
