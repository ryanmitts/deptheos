// The compiler settings of every native library deptheos_raw and
// deptheos_dng build: one set for vendored code, one for our own.

use std::path::PathBuf;

/// How a [`cc::Build`] compiles: as vendored code ([`Flags::vendored`]) or
/// as ours ([`Flags::ours`]).
///
/// Both define `NDEBUG` in every profile (cc never does): without it LibRaw's
/// decoders and libjxl's `JXL_ENSURE`/`JXL_DASSERT` trap on a damaged file
/// instead of failing its read.
///
/// On MSVC both turn on C++ exceptions with unwinding, since these libraries
/// report errors by throwing. `/EHs`, not `/EHsc`: the DNG SDK's libjpeg
/// error handler throws through `extern "C"` calls, and the C APIs
/// (`raw_c_api.cpp`, `dng_c_api.cpp`) catch in `extern "C"` functions.
pub trait Flags {
    /// Vendored code: its warnings are not actionable here, so none.
    fn vendored(&mut self) -> &mut Self;
    /// Our own code, warnings on, with `vendored` as `-isystem` headers so a
    /// warning is one of ours. MSVC builds it as vendored code.
    fn ours(&mut self, vendored: &[PathBuf]) -> &mut Self;
}

impl Flags for cc::Build {
    fn vendored(&mut self) -> &mut Self {
        self.warnings(false).define("NDEBUG", "1");
        if self.get_compiler().is_like_msvc() {
            self.flag("/EHs").flag("/w").flag("/utf-8").flag("/bigobj");
        } else {
            // -w leaves GCC's ABI-change notes, irrelevant as we don't mix
            // old and new objects.
            self.flag("-w").flag_if_supported("-Wno-psabi");
        }
        self
    }

    fn ours(&mut self, vendored: &[PathBuf]) -> &mut Self {
        if self.get_compiler().is_like_msvc() {
            self.includes(vendored);
            return self.vendored();
        }
        self.warnings(true).extra_warnings(true).define("NDEBUG", "1");
        self.flag_if_supported("-Wno-psabi");
        for dir in vendored {
            self.flag("-isystem").flag(dir);
        }
        self
    }
}
