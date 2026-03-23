#ifndef JXL_SHIM_H
#define JXL_SHIM_H

// Re-export just the encode/decode API headers we need from libjxl.
// This avoids the cms.h → jxl_cms_export.h transitive include problem
// when building a Swift module from CLibJXL's public headers directly.

#include <jxl/types.h>
#include <jxl/codestream_header.h>
#include <jxl/color_encoding.h>
#include <jxl/encode.h>
#include <jxl/decode.h>
#include <jxl/thread_parallel_runner.h>

#endif /* JXL_SHIM_H */
