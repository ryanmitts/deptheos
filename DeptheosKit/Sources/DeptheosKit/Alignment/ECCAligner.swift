import Accelerate
import CAccelerateNewLAPACK
import CoreVideo
import DeptheosShaders
import Metal
import MetalPerformanceShaders

// MARK: - GPU ECC Aligner

/// GPU-accelerated ECC alignment with multi-level coarse-to-fine pyramid (Szeliski §3.2 + §3.4).
///
/// Uses bilinear warp with out-of-bounds masking, MPS for blur/gradients,
/// and custom Metal kernels for masked Hessian/projection reductions.
/// Runs ECC gradient descent at each pyramid level, seeding each from the previous.
public class ECCAligner: @unchecked Sendable {
    public let context: MetalContext

    private struct LevelConfig {
        let scaleRatio: Float
        let lw: Int
        let lh: Int
        let blurSigma: Float
        let epsilon: Float
        let maxIterations: Int
    }

    private let coarseMaxDimension: Int
    private let fineMaxDimension: Int
    private let coarseBlurSigma: Float
    private let fineBlurSigma: Float

    private let pipelines: ECCPipelines
    private var blurFilters: [MPSImageGaussianBlur] = []
    private var workspaces: [ECCWorkspace?] = []
    private var cachedBlurred: [MTLTexture?] = []

    // Double-buffered GPU grayscale textures (full resolution, .private).
    private var grayscaleSlotA: MTLTexture?
    private var grayscaleSlotB: MTLTexture?
    private var lastSrcInSlotA: Bool = false
    private var lastSrcBufferPtr: UnsafeMutableRawPointer?

    public init(
        context: MetalContext,
        coarseMaxDimension: Int = 256,
        fineMaxDimension: Int = 2048,
        coarseBlurSigma: Float = 0.0,
        fineBlurSigma: Float = 0.8
    ) throws {
        self.context = context
        self.coarseMaxDimension = coarseMaxDimension
        self.fineMaxDimension = fineMaxDimension
        self.coarseBlurSigma = coarseBlurSigma
        self.fineBlurSigma = fineBlurSigma
        self.pipelines = try ECCPipelines(context: context)
    }

    /// Build a pyramid sized to the actual source image. Levels start from
    /// `coarseMaxDimension` and double up to `min(fineMaxDimension, sourceMax)`.
    /// For sources at or below `coarseMaxDimension`, returns a single level at source size.
    private func buildPyramid(width: Int, height: Int) -> [LevelConfig] {
        let sourceMax = max(width, height)
        let topDim = min(fineMaxDimension, sourceMax)

        var dims: [Int] = []
        if topDim <= coarseMaxDimension {
            dims = [topDim]
        } else {
            dims = [coarseMaxDimension]
            while let last = dims.last, last * 2 < topDim {
                dims.append(last * 2)
            }
            if dims.last! < topDim {
                dims.append(topDim)
            }
        }

        let n = dims.count
        return dims.enumerated().map { (i, dim) -> LevelConfig in
            let scaleRatio = Float(dim) / Float(sourceMax)
            let lw = Int(Float(width) * scaleRatio)
            let lh = Int(Float(height) * scaleRatio)
            let t = n > 1 ? Float(i) / Float(n - 1) : 1.0
            let sigma = coarseBlurSigma + (fineBlurSigma - coarseBlurSigma) * t
            let epsilon: Float
            let maxIter: Int
            if i == 0 {
                epsilon = 0.01; maxIter = 25
            } else if i == n - 1 {
                epsilon = 0.001; maxIter = 50
            } else {
                epsilon = 0.005; maxIter = 30
            }
            return LevelConfig(
                scaleRatio: scaleRatio, lw: lw, lh: lh,
                blurSigma: sigma, epsilon: epsilon, maxIterations: maxIter
            )
        }
    }

    public func findAlignment(reference: MTLTexture, source: MTLTexture) async -> AlignmentResult {
        do {
            return try await findAlignmentGPU(reference: reference, source: source)
        } catch {
            return AlignmentResult(transform: .identity, confidence: 0, converged: false)
        }
    }

    private func findAlignmentGPU(reference: MTLTexture, source: MTLTexture) async throws -> AlignmentResult {
        // Contract: a single ECCAligner instance handles one image size for its lifetime.
        // Stacking jobs construct a fresh aligner per run; StackingSession enforces same-size
        // across frames. We additionally guard ref-vs-source here since nothing else does.
        guard reference.width == source.width, reference.height == source.height else {
            throw GPUECCError.dimensionMismatch(
                reference: (reference.width, reference.height),
                source: (source.width, source.height)
            )
        }

        let width = source.width
        let height = source.height

        // Build a pyramid sized to this source image.
        let levelConfigs = buildPyramid(width: width, height: height)

        // Lazy first-call init: source size is fixed for the lifetime of this instance.
        if workspaces.isEmpty {
            workspaces = Array(repeating: nil, count: levelConfigs.count)
            cachedBlurred = Array(repeating: nil, count: levelConfigs.count)
            blurFilters = levelConfigs.map { pipelines.makeBlurFilter(sigma: $0.blurSigma) }
        }

        // Allocate workspaces on first call.
        for (i, cfg) in levelConfigs.enumerated() where workspaces[i] == nil {
            workspaces[i] = try ECCWorkspace(device: context.device, width: cfg.lw, height: cfg.lh)
        }

        // Grayscale conversion with chain-alignment caching
        let (refGray, srcGray) = try await convertGrayscalePair(reference: reference, source: source)

        // Coarse-to-fine: run ECC at each level, seeding from the previous
        var currentTransform: AffineTransform2D = .identity
        var prevScaleRatio: Float = 0
        var totalIterations = 0
        var lastConfidence: Float = 0
        var lastConverged = false

        for (i, cfg) in levelConfigs.enumerated() {
            let ws = workspaces[i]!

            try await resizeToWorkspace(refGray: refGray, srcGray: srcGray, workspace: ws)

            let seedInvMap = (i == 0) ? AffineTransform2D.identity
                : currentTransform.scaled(fromScale: prevScaleRatio, toScale: cfg.scaleRatio)

            let criteria = AlignmentCriteria(epsilon: cfg.epsilon, maxIterations: cfg.maxIterations)
            pipelines.gaussianBlur = blurFilters[i]
            let result = try await runECC(
                template: ws.inputRef, input: ws.inputSrc,
                pipelines: pipelines, workspace: ws,
                seedInvMap: seedInvMap, criteria: criteria,
                blurSigma: cfg.blurSigma, cachedTemplateBlurred: cachedBlurred[i]
            )
            cachedBlurred[i] = ws.blurredInput

            if i == 0 && !result.converged && result.confidence <= 0.5 {
                for j in 1..<levelConfigs.count { cachedBlurred[j] = nil }
                return AlignmentResult(transform: .identity, confidence: result.confidence,
                                       converged: false, iterations: result.iterations)
            }

            currentTransform = result.transform
            prevScaleRatio = cfg.scaleRatio
            totalIterations += result.iterations
            lastConfidence = result.confidence
            lastConverged = result.converged
        }

        let finestRatio = levelConfigs.last!.scaleRatio
        let finalTransform = currentTransform.scaled(fromScale: finestRatio, toScale: 1.0)
        return AlignmentResult(
            transform: finalTransform, confidence: lastConfidence,
            converged: lastConverged, iterations: totalIterations
        )
    }

    /// Shared grayscale conversion with double-buffered caching for chain alignment.
    private func convertGrayscalePair(reference: MTLTexture, source: MTLTexture) async throws -> (MTLTexture, MTLTexture) {
        let width = source.width
        let height = source.height

        let refPtr = Unmanaged.passUnretained(reference).toOpaque()
        let cacheHit = (refPtr == lastSrcBufferPtr && grayscaleSlotA != nil)
        try ensureGrayscaleSlot(&grayscaleSlotA, width: width, height: height, label: "ecc_gray_a")
        try ensureGrayscaleSlot(&grayscaleSlotB, width: width, height: height, label: "ecc_gray_b")

        let refGray: MTLTexture
        let srcGray: MTLTexture
        if cacheHit {
            refGray = lastSrcInSlotA ? grayscaleSlotA! : grayscaleSlotB!
            srcGray = lastSrcInSlotA ? grayscaleSlotB! : grayscaleSlotA!
        } else {
            let refSlot = lastSrcInSlotA ? grayscaleSlotB! : grayscaleSlotA!
            try await convertToGrayscaleGPU(reference, into: refSlot)
            refGray = refSlot
            srcGray = lastSrcInSlotA ? grayscaleSlotA! : grayscaleSlotB!
        }
        try await convertToGrayscaleGPU(source, into: srcGray)
        lastSrcInSlotA = (srcGray === grayscaleSlotA!)
        lastSrcBufferPtr = Unmanaged.passUnretained(source).toOpaque()

        return (refGray, srcGray)
    }

    /// Resize full-resolution grayscale textures into workspace input textures using MPS.
    private func resizeToWorkspace(
        refGray: MTLTexture, srcGray: MTLTexture,
        workspace: ECCWorkspace
    ) async throws {
        guard let cmdBuf = context.commandQueue.makeCommandBuffer() else {
            throw GPUECCError.commandBufferFailed
        }
        pipelines.scaler.encode(commandBuffer: cmdBuf, sourceTexture: refGray,
                                destinationTexture: workspace.inputRef)
        pipelines.scaler.encode(commandBuffer: cmdBuf, sourceTexture: srcGray,
                                destinationTexture: workspace.inputSrc)
        cmdBuf.commit()
        await cmdBuf.completed()
    }

    // MARK: - Single-Level ECC (matches OpenCV findTransformECC)

    private func runECC(
        template: MTLTexture, input: MTLTexture,
        pipelines: ECCPipelines, workspace: ECCWorkspace,
        seedInvMap: AffineTransform2D, criteria: AlignmentCriteria,
        blurSigma: Float, cachedTemplateBlurred: MTLTexture?
    ) async throws -> AlignmentResult {
        let queue = context.commandQueue
        let w = workspace.width
        let h = workspace.height

        // Step 1: Blur template and input, compute gradients on input
        guard let cmdBuf = queue.makeCommandBuffer() else { throw GPUECCError.commandBufferFailed }

        if let cached = cachedTemplateBlurred {
            guard let blit = cmdBuf.makeBlitCommandEncoder() else { throw GPUECCError.commandBufferFailed }
            blit.copy(from: cached, sourceSlice: 0, sourceLevel: 0,
                      sourceOrigin: MTLOrigin(), sourceSize: MTLSize(width: w, height: h, depth: 1),
                      to: workspace.blurredTemplate, destinationSlice: 0, destinationLevel: 0,
                      destinationOrigin: MTLOrigin())
            blit.endEncoding()
        } else if blurSigma > 0 {
            pipelines.gaussianBlur.encode(commandBuffer: cmdBuf, sourceTexture: template,
                                          destinationTexture: workspace.blurredTemplate)
        } else {
            guard let blit = cmdBuf.makeBlitCommandEncoder() else { throw GPUECCError.commandBufferFailed }
            blit.copy(from: template, sourceSlice: 0, sourceLevel: 0,
                      sourceOrigin: MTLOrigin(), sourceSize: MTLSize(width: w, height: h, depth: 1),
                      to: workspace.blurredTemplate, destinationSlice: 0, destinationLevel: 0,
                      destinationOrigin: MTLOrigin())
            blit.endEncoding()
        }

        if blurSigma > 0 {
            pipelines.gaussianBlur.encode(commandBuffer: cmdBuf, sourceTexture: input,
                                          destinationTexture: workspace.blurredInput)
        } else {
            guard let blit = cmdBuf.makeBlitCommandEncoder() else { throw GPUECCError.commandBufferFailed }
            blit.copy(from: input, sourceSlice: 0, sourceLevel: 0,
                      sourceOrigin: MTLOrigin(), sourceSize: MTLSize(width: w, height: h, depth: 1),
                      to: workspace.blurredInput, destinationSlice: 0, destinationLevel: 0,
                      destinationOrigin: MTLOrigin())
            blit.endEncoding()
        }

        // Compute gradients on blurred input (once, then warped each iteration)
        pipelines.sobelX.encode(commandBuffer: cmdBuf, sourceTexture: workspace.blurredInput,
                                destinationTexture: workspace.gradX)
        pipelines.sobelY.encode(commandBuffer: cmdBuf, sourceTexture: workspace.blurredInput,
                                destinationTexture: workspace.gradY)

        cmdBuf.commit()
        await cmdBuf.completed()

        // ECC iteration loop — uses inverse mapping convention (like OpenCV).
        // The map is updated directly: map += deltaP. The map IS the dst→src mapping.
        //
        // GPU pipeline per iteration (2 syncs instead of 5):
        //   Sync 1: warp → stats(img) → stats(tmpl) → Hessian   [1 command buffer]
        //   CPU:    phase2 reduction, 6×6 solve, lambda
        //   Sync 2: projection                                    [1 command buffer]
        //   CPU:    phase2 reduction, deltaP, update invMap
        var invMap = seedInvMap
        var rho: Float = -1
        var lastRho: Float = -criteria.epsilon
        var finalIter = 0
        var converged = false

        let wgSize = 128
        let numWGs = workspace.numWorkgroups
        let reductionFloats = 36

        var reductionParams = EccReductionParams(
            width: UInt32(w), height: UInt32(h),
            pixel_count: UInt32(w * h),
            num_workgroups: UInt32(numWGs)
        )

        for iter in 1...criteria.maxIterations {
            finalIter = iter
            if abs(rho - lastRho) < criteria.epsilon && criteria.epsilon >= 0 { converged = true; break }

            // ── Sync 1: warp → stats(img) → stats(tmpl) → Hessian ──
            guard let cmdBuf1 = queue.makeCommandBuffer(),
                  let enc = cmdBuf1.makeComputeCommandEncoder() else { throw GPUECCError.commandBufferFailed }
            cmdBuf1.label = "ECC Warp+Stats+Hessian"
            enc.label = "ECC Warp+Stats+Hessian"

            // 1a. Warp image + gradients + mask
            var warpParams = EccWarpParams(
                inv_a: invMap.a, inv_b: invMap.b,
                inv_c: invMap.c, inv_d: invMap.d,
                inv_tx: invMap.tx, inv_ty: invMap.ty,
                width: UInt32(w), height: UInt32(h)
            )
            enc.setComputePipelineState(pipelines.warpTriple)
            enc.setTexture(workspace.blurredInput, index: 0)
            enc.setTexture(workspace.gradX, index: 1)
            enc.setTexture(workspace.gradY, index: 2)
            enc.setTexture(workspace.packedWarp, index: 3)
            enc.setTexture(workspace.warpedMask, index: 4)
            enc.setBytes(&warpParams, length: MemoryLayout<EccWarpParams>.stride, index: 0)
            enc.dispatchThreads(MTLSize(width: w, height: h, depth: 1),
                                threadsPerThreadgroup: MTLSize(width: 16, height: 16, depth: 1))
            enc.memoryBarrier(scope: .textures)

            // 1b. Stats phase 1+2 for warped image
            var pixelCount = UInt32(w * h)
            var widthU32 = UInt32(w)
            var numWGsU32 = UInt32(numWGs)

            enc.setComputePipelineState(pipelines.statsPhase1)
            enc.setTexture(workspace.packedWarp, index: 0)  // .r = warped image value
            enc.setTexture(workspace.warpedMask, index: 1)
            enc.setBuffer(workspace.statsPartialsBuffer, offset: 0, index: 0)
            enc.setBytes(&pixelCount, length: MemoryLayout<UInt32>.stride, index: 1)
            enc.setBytes(&widthU32, length: MemoryLayout<UInt32>.stride, index: 2)
            enc.dispatchThreadgroups(MTLSize(width: numWGs, height: 1, depth: 1),
                                     threadsPerThreadgroup: MTLSize(width: wgSize, height: 1, depth: 1))
            enc.memoryBarrier(scope: .buffers)

            enc.setComputePipelineState(pipelines.statsPhase2)
            enc.setBuffer(workspace.imgStatsResultBuffer, offset: 0, index: 1)
            enc.setBytes(&numWGsU32, length: MemoryLayout<UInt32>.stride, index: 2)
            enc.dispatchThreadgroups(MTLSize(width: 1, height: 1, depth: 1),
                                     threadsPerThreadgroup: MTLSize(width: wgSize, height: 1, depth: 1))
            enc.memoryBarrier(scope: .buffers)

            // 1c. Stats phase 1+2 for template (same mask, reuse statsPartialsBuffer)
            enc.setComputePipelineState(pipelines.statsPhase1)
            enc.setTexture(workspace.blurredTemplate, index: 0)
            enc.setBytes(&pixelCount, length: MemoryLayout<UInt32>.stride, index: 1)
            enc.setBytes(&widthU32, length: MemoryLayout<UInt32>.stride, index: 2)
            enc.dispatchThreadgroups(MTLSize(width: numWGs, height: 1, depth: 1),
                                     threadsPerThreadgroup: MTLSize(width: wgSize, height: 1, depth: 1))
            enc.memoryBarrier(scope: .buffers)

            enc.setComputePipelineState(pipelines.statsPhase2)
            enc.setBuffer(workspace.tmplStatsResultBuffer, offset: 0, index: 1)
            enc.setBytes(&numWGsU32, length: MemoryLayout<UInt32>.stride, index: 2)
            enc.dispatchThreadgroups(MTLSize(width: 1, height: 1, depth: 1),
                                     threadsPerThreadgroup: MTLSize(width: wgSize, height: 1, depth: 1))
            enc.memoryBarrier(scope: .buffers)

            // 1d. Hessian phase 1 (reads means from GPU-resident stats buffers)
            enc.setComputePipelineState(pipelines.hessianPhase1)
            enc.setTexture(workspace.packedWarp, index: 0)
            enc.setTexture(workspace.blurredTemplate, index: 1)
            enc.setTexture(workspace.warpedMask, index: 2)
            enc.setBuffer(workspace.hessianPartialsBuffer, offset: 0, index: 0)
            enc.setBytes(&reductionParams, length: MemoryLayout<EccReductionParams>.stride, index: 1)
            enc.setBuffer(workspace.imgStatsResultBuffer, offset: 0, index: 2)
            enc.setBuffer(workspace.tmplStatsResultBuffer, offset: 0, index: 3)
            enc.dispatchThreadgroups(MTLSize(width: numWGs, height: 1, depth: 1),
                                     threadsPerThreadgroup: MTLSize(width: wgSize, height: 1, depth: 1))

            enc.endEncoding()
            cmdBuf1.commit()
            await cmdBuf1.completed()

            // CPU: read back stats + Hessian partials
            let imgStatsPtr = workspace.imgStatsResultBuffer.contents().bindMemory(to: Float.self, capacity: 3)
            let tmplStatsPtr = workspace.tmplStatsResultBuffer.contents().bindMemory(to: Float.self, capacity: 3)
            let imgStd = imgStatsPtr[1]
            let tmplStd = tmplStatsPtr[1]
            let validCount = Int(imgStatsPtr[2])

            let sqrtN = sqrt(Float(validCount))
            let imgNorm = abs(imgStd) * sqrtN
            let tmplNorm = abs(tmplStd) * sqrtN

            // Phase 2: reduce Hessian partials in double precision on CPU
            let partialsPtr = workspace.hessianPartialsBuffer.contents().bindMemory(
                to: Float.self, capacity: numWGs * reductionFloats)
            var hessianResult = [Double](repeating: 0, count: reductionFloats)
            for g in 0..<numWGs {
                let offset = g * reductionFloats
                for i in 0..<reductionFloats {
                    hessianResult[i] += Double(partialsPtr[offset + i])
                }
            }

            var hessian = [[Float]](repeating: [Float](repeating: 0, count: 6), count: 6)
            var idx = 0
            for i in 0..<6 {
                for j in i..<6 {
                    hessian[i][j] = Float(hessianResult[idx])
                    hessian[j][i] = Float(hessianResult[idx])
                    idx += 1
                }
            }

            let correlation = Float(hessianResult[21])
            var imgProj = [Float](repeating: 0, count: 6)
            var tmplProj = [Float](repeating: 0, count: 6)
            for i in 0..<6 {
                imgProj[i] = Float(hessianResult[22 + i])
                tmplProj[i] = Float(hessianResult[28 + i])
            }

            guard let hFactor = choleskyFactor6x6(hessian) else { break }

            lastRho = rho
            rho = correlation / (imgNorm * tmplNorm)
            if rho.isNaN { break }

            var imgProjHessianD = imgProj.map { Double($0) }
            guard choleskySolve6(hFactor, &imgProjHessianD) else { break }
            let imgProjHessian = imgProjHessianD.map { Float($0) }
            let imgNormSq = imgNorm * imgNorm
            let lambdaN = imgNormSq - dotProduct6(imgProj, imgProjHessian)
            let lambdaD = correlation - dotProduct6(tmplProj, imgProjHessian)
            guard lambdaD > 0 else { break }
            let lambda = lambdaN / lambdaD

            // ── Sync 2: projection ──
            guard let cmdBuf2 = queue.makeCommandBuffer(),
                  let enc2 = cmdBuf2.makeComputeCommandEncoder() else { throw GPUECCError.commandBufferFailed }
            cmdBuf2.label = "ECC Projection"
            enc2.label = "ECC Projection"

            var lambdaVal = lambda
            enc2.setComputePipelineState(pipelines.projectionPhase1)
            enc2.setTexture(workspace.packedWarp, index: 0)
            enc2.setTexture(workspace.blurredTemplate, index: 1)
            enc2.setTexture(workspace.warpedMask, index: 2)
            enc2.setBuffer(workspace.projPartialsBuffer, offset: 0, index: 0)
            enc2.setBytes(&reductionParams, length: MemoryLayout<EccReductionParams>.stride, index: 1)
            enc2.setBuffer(workspace.imgStatsResultBuffer, offset: 0, index: 2)
            enc2.setBuffer(workspace.tmplStatsResultBuffer, offset: 0, index: 3)
            enc2.setBytes(&lambdaVal, length: MemoryLayout<Float>.stride, index: 4)
            enc2.dispatchThreadgroups(MTLSize(width: numWGs, height: 1, depth: 1),
                                      threadsPerThreadgroup: MTLSize(width: wgSize, height: 1, depth: 1))
            enc2.endEncoding()
            cmdBuf2.commit()
            await cmdBuf2.completed()

            // CPU: reduce projection partials in double precision
            let projPartialsPtr = workspace.projPartialsBuffer.contents().bindMemory(
                to: Float.self, capacity: numWGs * 6)
            var errorProjection = [Float](repeating: 0, count: 6)
            for i in 0..<6 {
                var sum = Double(0)
                for g in 0..<numWGs { sum += Double(projPartialsPtr[g * 6 + i]) }
                errorProjection[i] = Float(sum)
            }

            var deltaPD = errorProjection.map { Double($0) }
            guard choleskySolve6(hFactor, &deltaPD) else { break }
            let deltaP = deltaPD.map { Float($0) }

            invMap.a += deltaP[0]
            invMap.b += deltaP[1]
            invMap.c += deltaP[2]
            invMap.d += deltaP[3]
            invMap.tx += deltaP[4]
            invMap.ty += deltaP[5]
        }

        return AlignmentResult(transform: invMap, confidence: rho,
                               converged: converged, iterations: finalIter)
    }

    // MARK: - GPU Grayscale Conversion

    /// Convert a texture to r32Float grayscale on GPU, writing into the given .private texture.
    /// Uses MPSImageConversion with RGB→Gray color space for proper Bt.709 luma weighting.
    /// For already-grayscale inputs, blits directly.
    private func convertToGrayscaleGPU(_ texture: MTLTexture, into destination: MTLTexture) async throws {
        guard let cmdBuf = context.commandQueue.makeCommandBuffer() else {
            throw GPUECCError.commandBufferFailed
        }

        if texture.pixelFormat == .r32Float {
            guard let blit = cmdBuf.makeBlitCommandEncoder() else { throw GPUECCError.commandBufferFailed }
            blit.copy(from: texture, to: destination)
            blit.endEncoding()
        } else {
            let conversion = pipelines.grayscaleConversion
            conversion.encode(commandBuffer: cmdBuf, sourceTexture: texture,
                              destinationTexture: destination)
        }

        cmdBuf.commit()
        await cmdBuf.completed()
    }

    /// Ensure a grayscale texture slot is allocated at the given dimensions.
    private func ensureGrayscaleSlot(_ slot: inout MTLTexture?, width: Int, height: Int, label: String) throws {
        if let tex = slot, tex.width == width, tex.height == height { return }
        let desc = MTLTextureDescriptor.texture2DDescriptor(
            pixelFormat: .r32Float, width: width, height: height, mipmapped: false
        )
        desc.usage = [.shaderRead, .shaderWrite]
        desc.storageMode = .private
        guard let tex = context.device.makeTexture(descriptor: desc) else {
            throw GPUECCError.allocationFailed(label)
        }
        tex.label = label
        slot = tex
    }
}

// MARK: - ECC Criteria

private struct AlignmentCriteria: Sendable {
    let epsilon: Float
    let maxIterations: Int
}

// MARK: - CPU Matrix Utilities (double precision for stability)

/// Cholesky-factor a 6×6 symmetric positive-definite matrix.
/// Returns the factor as a column-major flat [Double] of length 36, or nil if H is not SPD.
private func choleskyFactor6x6(_ hFloat: [[Float]]) -> [Double]? {
    // Pack symmetric H into column-major flat layout. Only the upper triangle is read by dpotrf.
    var hFlat = [Double](repeating: 0, count: 36)
    for col in 0..<6 {
        for row in 0...col {
            hFlat[col * 6 + row] = Double(hFloat[row][col])
        }
    }
    var n = __LAPACK_int(6)
    var lda = __LAPACK_int(6)
    var info = __LAPACK_int(0)
    var uplo: Int8 = 0x55  // 'U' — upper triangle
    dpotrf_(&uplo, &n, &hFlat, &lda, &info)
    return info == 0 ? hFlat : nil
}

/// Solve H · x = b given the Cholesky factor from `choleskyFactor6x6`.
/// `b` is overwritten with the solution. Returns false if the solve fails.
private func choleskySolve6(_ factor: [Double], _ b: inout [Double]) -> Bool {
    var n = __LAPACK_int(6)
    var nrhs = __LAPACK_int(1)
    var lda = __LAPACK_int(6)
    var ldb = __LAPACK_int(6)
    var info = __LAPACK_int(0)
    var uplo: Int8 = 0x55  // 'U'
    var factorCopy = factor  // dpotrs reads only; copy for safety/clarity
    dpotrs_(&uplo, &n, &nrhs, &factorCopy, &lda, &b, &ldb, &info)
    return info == 0
}

private func dotProduct6(_ a: [Float], _ b: [Float]) -> Float {
    var sum = Double(0)
    for i in 0..<6 { sum += Double(a[i]) * Double(b[i]) }
    return Float(sum)
}

enum GPUECCError: Error {
    case commandBufferFailed
    case uploadFailed
    case allocationFailed(String)
    case dimensionMismatch(reference: (Int, Int), source: (Int, Int))
}
