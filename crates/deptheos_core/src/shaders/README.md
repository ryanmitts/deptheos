# Shared WGSL modules

This folder holds only shader modules that other shaders import by name
(`deptheos::warp`, `deptheos::resample`, `deptheos::focus`,
`deptheos::laplacian`, `deptheos::accum`, `deptheos::plane`,
`deptheos::quantize`, `deptheos::color`, `deptheos::metric`), each in the
file of its name but `deptheos::warp`, in `warp_common.wgsl`. They are
registered in `pipelines.rs` (`SHARED_SHADER_MODULES`) and `pack.rs`
(`QUANTIZE_MODULE`).

A shader that is compiled into its own pipeline lives next to the Rust file
that builds that pipeline, named after it (`warp.rs` → `warp.wgsl`,
`depth/pipeline.rs` → `depth/depth.wgsl`).
