//! Print the adapter, negotiated capabilities, and optional format support.
//! Run with: cargo run -p deptheos_gpu --features examples --example gpu_info

fn main() {
    let ctx = match deptheos_gpu::GpuContext::new() {
        Ok(ctx) => ctx,
        Err(e) => {
            eprintln!("GPU init failed: {e}");
            std::process::exit(1);
        }
    };

    println!(
        "adapter:  {} ({:?}, {:?})",
        ctx.adapter_info.name, ctx.adapter_info.device_type, ctx.adapter_info.backend
    );
    println!("timestamp queries: {}", ctx.timestamp_queries);
    println!("format support: {:#?}", ctx.format_support);
    println!("source format:     {:?}", ctx.source_format());
    println!("map format:        {:?}", ctx.map_format());

    let limits = ctx.device.limits();
    println!("max texture dim:   {}", limits.max_texture_dimension_2d);
    println!("max buffer size:   {} MiB", limits.max_buffer_size / (1024 * 1024));
    println!("max storage textures/stage: {}", limits.max_storage_textures_per_shader_stage);
}
