//
//  dng_bridge_internal.h
//  DeptheosKit
//
//  Internal header — includes DNG SDK types. Not part of the public module.
//
#pragma once

#include "dng_bridge.h"

#include <stdbool.h>
#include <stdexcept>
#include <cstring>
#include <memory>
#include <cstdio>
#include <cstdarg>
#include <cmath>
#include <algorithm>
#include <mutex>
#include <thread>
#include <dispatch/dispatch.h>

#include "dng_host.h"
#include "dng_camera_profile.h"
#include "dng_file_stream.h"
#include "dng_memory_stream.h"
#include "dng_info.h"
#include "dng_negative.h"
#include "dng_image.h"
#include "dng_pixel_buffer.h"
#include "dng_simple_image.h"
#include "dng_image_writer.h"
#include "dng_render.h"
#include "dng_preview.h"
#include "dng_color_space.h"
#include "dng_color_spec.h"
#include "dng_exceptions.h"
#include "dng_tag_codes.h"
#include "dng_tag_values.h"
#include "dng_sdk_limits.h"
#include "dng_area_task.h"
#include "dng_jpeg_image.h"

class deptheos_host : public dng_host {
public:
    deptheos_host()
        : dng_host()
        , fThreads(std::min((uint32)std::thread::hardware_concurrency(),
                            (uint32)kMaxMPThreads))
    {
        if (fThreads < 1) fThreads = 1;
        deptheos_dng_initialize();
        dng_log(DNG_LOG_INFO, "[deptheos_host] Created with %u threads", fThreads);
    }

    uint32 PerformAreaTaskThreads() override {
        return fThreads;
    }

    void PerformAreaTask(dng_area_task &task,
                         const dng_rect &area,
                         dng_area_task_progress *progress = nullptr) override {

        uint32 threadCount = std::min(fThreads, task.MaxThreads());

        dng_point tileSize(task.FindTileSize(area));
        uint32 tileRows = (area.H() + tileSize.v - 1) / tileSize.v;
        uint32 tileCols = (area.W() + tileSize.h - 1) / tileSize.h;
        uint32 totalTiles = tileRows * tileCols;
        threadCount = std::min(threadCount, totalTiles);
        if (threadCount < 1) threadCount = 1;

        dng_log(DNG_LOG_INFO, "[deptheos_host] PerformAreaTask: %u threads, area %dx%d, tile %dx%d",
                threadCount, area.W(), area.H(), tileSize.h, tileSize.v);

        task.Start(threadCount, area, tileSize, &Allocator(), Sniffer());

        if (threadCount == 1) {
            task.ProcessOnThread(0, area, tileSize, Sniffer(), progress);
        } else if (tileRows >= threadCount) {
            int32 rowsPerThread = tileSize.v * ((tileRows + threadCount - 1) / threadCount);

            dispatch_apply(threadCount, DISPATCH_APPLY_AUTO, ^(size_t t) {
                int32 top = area.t + (int32)t * rowsPerThread;
                int32 bot = std::min(top + rowsPerThread, area.b);
                if (top >= area.b) return;
                dng_rect subArea(top, area.l, bot, area.r);
                task.ProcessOnThread((uint32)t, subArea, tileSize, Sniffer(), progress);
            });
        } else {
            int32 colsPerThread = tileSize.h * ((tileCols + threadCount - 1) / threadCount);

            dispatch_apply(threadCount, DISPATCH_APPLY_AUTO, ^(size_t t) {
                int32 left = area.l + (int32)t * colsPerThread;
                int32 right = std::min(left + colsPerThread, area.r);
                if (left >= area.r) return;
                dng_rect subArea(area.t, left, area.b, right);
                task.ProcessOnThread((uint32)t, subArea, tileSize, Sniffer(), progress);
            });
        }

        task.Finish(threadCount);
    }

private:
    uint32 fThreads;
};

void fill_image_metadata(dng_host &host, dng_negative &negative,
                         DngImageMetadata &out);

void fill_render_params(dng_negative &negative, DngRenderParams &out);

dng_negative * rebuild_negative_from_metadata(
    dng_host &host, const DngImageMetadata &meta,
    const uint16_t *pixels, int w, int h, int ch);
