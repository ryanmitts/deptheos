import Metal

/// All compute pipeline states for wavelet stacking operations.
/// Created eagerly at init — reused for every frame.
/// Port from deptheos-rust/deptheos_core/src/gpu/wavelet/pipeline.rs
struct WaveletPipelines {
    let common: StackingPipelines

    let extractLuma: MTLComputePipelineState
    let padReflect: MTLComputePipelineState
    let forwardDecompose: MTLComputePipelineState
    let inverseReconstruct: MTLComputePipelineState
    let coupledWta: MTLComputePipelineState
    let llAccumulate: MTLComputePipelineState
    let llWta: MTLComputePipelineState
    let llNormalize: MTLComputePipelineState
    let colorReassignRw: MTLComputePipelineState
    let initBestDistance: MTLComputePipelineState
    let sumEnergyMap: MTLComputePipelineState
    let subbandConsistency: MTLComputePipelineState
    let neighborConsistency: MTLComputePipelineState
    let waveletDenoise: MTLComputePipelineState

    init(common: StackingPipelines) throws {
        self.common = common
        let library = common.shaderLibrary
        let device = common.metalContext.device

        self.extractLuma = try Self.make(library: library, device: device, name: "extract_luma")
        self.padReflect = try Self.make(library: library, device: device, name: "pad_reflect")
        self.forwardDecompose = try Self.make(library: library, device: device, name: "forward_decompose")
        self.inverseReconstruct = try Self.make(library: library, device: device, name: "inverse_reconstruct")
        self.coupledWta = try Self.make(library: library, device: device, name: "coupled_wta")
        self.llAccumulate = try Self.make(library: library, device: device, name: "ll_accumulate")
        self.llWta = try Self.make(library: library, device: device, name: "ll_wta")
        self.llNormalize = try Self.make(library: library, device: device, name: "ll_normalize")
        self.colorReassignRw = try Self.make(library: library, device: device, name: "color_reassign_rw")
        self.initBestDistance = try Self.make(library: library, device: device, name: "init_best_distance")
        self.sumEnergyMap = try Self.make(library: library, device: device, name: "sum_energy_map")
        self.subbandConsistency = try Self.make(library: library, device: device, name: "subband_consistency")
        self.neighborConsistency = try Self.make(library: library, device: device, name: "neighbor_consistency")
        self.waveletDenoise = try Self.make(library: library, device: device, name: "wavelet_denoise")
    }

    private static func make(library: MTLLibrary, device: MTLDevice, name: String) throws -> MTLComputePipelineState {
        guard let function = library.makeFunction(name: name) else {
            throw StackingError.metalError("Shader function not found: \(name)")
        }
        return try device.makeComputePipelineState(function: function)
    }
}
