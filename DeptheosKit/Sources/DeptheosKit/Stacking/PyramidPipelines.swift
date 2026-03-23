import Metal

/// All compute pipeline states for pyramid stacking operations.
/// Created eagerly at init — reused for every frame.
/// Port from deptheos-rust/deptheos_core/src/gpu/pyramid/pipeline.rs
struct PyramidPipelines {
    let common: StackingPipelines

    // Pyramid-specific kernels
    let downsample: MTLComputePipelineState
    let laplacianAdd: MTLComputePipelineState
    let fusedLaplacianWta: MTLComputePipelineState
    let accumulateSum: MTLComputePipelineState
    let normalizeLevel: MTLComputePipelineState
    let focusToGrayscale8bit: MTLComputePipelineState

    init(common: StackingPipelines) throws {
        self.common = common
        let library = common.shaderLibrary
        let device = common.metalContext.device

        self.downsample = try Self.make(library: library, device: device, name: "downsample")
        self.laplacianAdd = try Self.make(library: library, device: device, name: "laplacian_add")
        self.fusedLaplacianWta = try Self.make(library: library, device: device, name: "fused_laplacian_wta")
        self.accumulateSum = try Self.make(library: library, device: device, name: "accumulate_sum")
        self.normalizeLevel = try Self.make(library: library, device: device, name: "normalize_level")
        self.focusToGrayscale8bit = try Self.make(library: library, device: device, name: "focus_to_grayscale_8bit")
    }

    private static func make(library: MTLLibrary, device: MTLDevice, name: String) throws -> MTLComputePipelineState {
        guard let function = library.makeFunction(name: name) else {
            throw StackingError.metalError("Shader function not found: \(name)")
        }
        return try device.makeComputePipelineState(function: function)
    }
}
