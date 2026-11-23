#include "dng_bridge_internal.h"

#include <atomic>
#include <cstdio>
#include <new>

#if qWinOS
#include <windows.h>
#else
#include <pthread.h>
#endif

// ---------------------------------------------------------------------------
// Logging
// ---------------------------------------------------------------------------

// Set from the caller's thread, read from SDK worker threads.
static std::atomic<DngLogCallback> g_log_callback{nullptr};

void deptheos_set_dng_log_callback(DngLogCallback callback) {
    g_log_callback.store(callback, std::memory_order_release);
}

void dng_log(DngLogLevel level, const char *format, ...) {
    DngLogCallback callback = g_log_callback.load(std::memory_order_acquire);
    if (!callback) return;
    char buffer[2048];
    va_list args;
    va_start(args, format);
    vsnprintf(buffer, sizeof(buffer), format, args);
    va_end(args);
    callback(static_cast<int32_t>(level), buffer);
}

// ---------------------------------------------------------------------------
// Files
// ---------------------------------------------------------------------------

#if qWinOS
std::wstring widen_utf8(const std::string &utf8) {
    if (utf8.empty()) return std::wstring();
    const int n = MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, utf8.data(),
                                      (int)utf8.size(), nullptr, 0);
    if (n <= 0) ThrowProgramError("path is not valid UTF-8");
    std::wstring wide(static_cast<size_t>(n), wchar_t{});
    MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, utf8.data(), (int)utf8.size(),
                        wide.data(), n);
    return wide;
}
#endif

// ---------------------------------------------------------------------------
// Threads
// ---------------------------------------------------------------------------

// The stack of every engine thread (deptheos_core::threads::STACK_SIZE).
static const size_t kEngineStackSize = 8u << 20;

struct engine_thread {
#if qWinOS
    HANDLE handle;
#else
    pthread_t handle;
#endif
    void (*run)(void *);
    void *arg;
};

#if qWinOS
static DWORD WINAPI engine_thread_main(LPVOID self) {
    engine_thread *thread = static_cast<engine_thread *>(self);
    thread->run(thread->arg);
    return 0;
}
#else
static void *engine_thread_main(void *self) {
    engine_thread *thread = static_cast<engine_thread *>(self);
    thread->run(thread->arg);
    return nullptr;
}
#endif

engine_thread *start_engine_thread(void (*run)(void *), void *arg) {
    engine_thread *thread = new (std::nothrow) engine_thread{};
    if (!thread) return nullptr;
    thread->run = run;
    thread->arg = arg;
#if qWinOS
    thread->handle = CreateThread(nullptr, kEngineStackSize, engine_thread_main, thread,
                                  STACK_SIZE_PARAM_IS_A_RESERVATION, nullptr);
    if (!thread->handle) {
        delete thread;
        return nullptr;
    }
#else
    pthread_attr_t attributes;
    if (pthread_attr_init(&attributes) != 0) {
        delete thread;
        return nullptr;
    }
    int started = pthread_attr_setstacksize(&attributes, kEngineStackSize);
    if (started == 0) {
        started = pthread_create(&thread->handle, &attributes, engine_thread_main, thread);
    }
    pthread_attr_destroy(&attributes);
    if (started != 0) {
        delete thread;
        return nullptr;
    }
#endif
    return thread;
}

void join_engine_thread(engine_thread *thread) {
#if qWinOS
    WaitForSingleObject(thread->handle, INFINITE);
    CloseHandle(thread->handle);
#else
    pthread_join(thread->handle, nullptr);
#endif
    delete thread;
}
