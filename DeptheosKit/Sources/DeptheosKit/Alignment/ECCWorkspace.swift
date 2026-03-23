import Metal

// MARK: - GPU ECC Workspace

/// GPU-resident textures and buffers reused across ECC iterations at a fixed resolution.
final class ECCWorkspace: @unchecked Sendable {
    let device: MTLDevice
    let width: Int
    let height: Int
    let pixelCount: Int

    let inputRef: MTLTexture       // r32Float: resized reference (before blur)
    let inputSrc: MTLTexture       // r32Float: resized source (before blur)
    let blurredTemplate: MTLTexture
    let blurredInput: MTLTexture
    let gradX: MTLTexture
    let gradY: MTLTexture
    let packedWarp: MTLTexture     // rgba32Float: (.r=image, .g=gradX, .b=gradY)
    let warpedMask: MTLTexture

    let hessianPartialsBuffer: MTLBuffer
    let projPartialsBuffer: MTLBuffer
    let statsPartialsBuffer: MTLBuffer  // reused for image and template stats
    let imgStatsResultBuffer: MTLBuffer // [mean, stddev, count] — GPU-resident
    let tmplStatsResultBuffer: MTLBuffer // [mean, stddev, count] — GPU-resident

    let numWorkgroups: Int

    init(device: MTLDevice, width: Int, height: Int) throws {
        self.device = device
        self.width = width
        self.height = height
        self.pixelCount = width * height

        let wgSize = 128
        self.numWorkgroups = min((pixelCount + wgSize - 1) / wgSize, 256)

        func makeR32Float(w: Int, h: Int, label: String) throws -> MTLTexture {
            let desc = MTLTextureDescriptor.texture2DDescriptor(
                pixelFormat: .r32Float, width: w, height: h, mipmapped: false
            )
            desc.usage = [.shaderRead, .shaderWrite]
            desc.storageMode = .private
            guard let tex = device.makeTexture(descriptor: desc) else {
                throw GPUECCError.allocationFailed(label)
            }
            tex.label = label
            return tex
        }

        self.inputRef = try makeR32Float(w: width, h: height, label: "ecc_input_ref")
        self.inputSrc = try makeR32Float(w: width, h: height, label: "ecc_input_src")
        self.blurredTemplate = try makeR32Float(w: width, h: height, label: "ecc_blurred_template")
        self.blurredInput = try makeR32Float(w: width, h: height, label: "ecc_blurred_input")
        self.gradX = try makeR32Float(w: width, h: height, label: "ecc_grad_x")
        self.gradY = try makeR32Float(w: width, h: height, label: "ecc_grad_y")
        self.warpedMask = try makeR32Float(w: width, h: height, label: "ecc_warped_mask")

        // Packed warp output: (image, gradX, gradY, 0) in a single rgba32Float texture
        let packedDesc = MTLTextureDescriptor.texture2DDescriptor(
            pixelFormat: .rgba32Float, width: width, height: height, mipmapped: false
        )
        packedDesc.usage = [.shaderRead, .shaderWrite]
        packedDesc.storageMode = .private
        guard let packedTex = device.makeTexture(descriptor: packedDesc) else {
            throw GPUECCError.allocationFailed("ecc_packed_warp")
        }
        packedTex.label = "ecc_packed_warp"
        self.packedWarp = packedTex

        let reductionFloats = 36
        guard let hessianBuf = device.makeBuffer(
            length: numWorkgroups * reductionFloats * MemoryLayout<Float>.stride, options: .storageModeShared),
              let projBuf = device.makeBuffer(
            length: numWorkgroups * 6 * MemoryLayout<Float>.stride, options: .storageModeShared),
              let statsBuf = device.makeBuffer(
            length: numWorkgroups * 3 * MemoryLayout<Float>.stride, options: .storageModeShared),
              let imgStatsBuf = device.makeBuffer(
            length: 3 * MemoryLayout<Float>.stride, options: .storageModeShared),
              let tmplStatsBuf = device.makeBuffer(
            length: 3 * MemoryLayout<Float>.stride, options: .storageModeShared)
        else {
            throw GPUECCError.allocationFailed("ECC reduction buffers")
        }
        self.hessianPartialsBuffer = hessianBuf
        self.projPartialsBuffer = projBuf
        self.statsPartialsBuffer = statsBuf
        self.imgStatsResultBuffer = imgStatsBuf
        self.tmplStatsResultBuffer = tmplStatsBuf
    }
}
