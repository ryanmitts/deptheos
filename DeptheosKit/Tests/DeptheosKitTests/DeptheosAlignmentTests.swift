//
//  DeptheosAlignmentTests.swift
//  DeptheosKit
//
//  Created by Ryan Mitts on 2026-03-26.
//

import Testing
import Foundation
import CoreImage
import CoreVideo
import DeptheosKit

@Suite("DeptheosAlignment")
struct DeptheosAlignmentTests {

    let context: MetalContext
    let aligner: ECCAligner
    let pixelBuffer: CVPixelBuffer
    let reference: MTLTexture

    init() async throws {
        context = try MetalContext()
        aligner = try ECCAligner(context: context)

        let url = try #require(
            Bundle.module.url(forResource: "fruits", withExtension: "jpg", subdirectory: "Resources"),
            "Put a fruits.jpg in Tests/DeptheosKitTests/Resources/"
        )
        let reader = ImageReader(ciContext: context.ciContext)
        let (pb, _) = try reader.loadWithBitDepth(url: url)
        pixelBuffer = pb
        reference = try #require(
            await pb.standaloneMetalTexture(
                device: context.device,
                commandQueue: context.commandQueue,
                cache: context.textureCache, label: "reference"
            )
        )
    }

    private var imageCenter: (Float, Float) {
        (Float(reference.width) / 2, Float(reference.height) / 2)
    }

    // MARK: - Tests

    @Test func canAlignTranslation() async throws {
        let applied = AffineTransform2D(a: 1, b: 0, c: 0, d: 1, tx: 5.0, ty: -3.0)
        try await assertAlignment(recovers: applied)
    }

    @Test func canAlignScale() async throws {
        let (cx, cy) = imageCenter
        let s: Float = 1.02
        let applied = AffineTransform2D(
            a: s, b: 0, c: 0, d: s,
            tx: cx * (1 - s), ty: cy * (1 - s)
        )
        try await assertAlignment(recovers: applied)
    }

    @Test func canAlignRotation() async throws {
        let (cx, cy) = imageCenter
        let theta: Float = 0.5 * .pi / 180
        let cosT = cos(theta), sinT = sin(theta)
        let applied = AffineTransform2D(
            a: cosT, b: sinT, c: -sinT, d: cosT,
            tx: cx * (1 - cosT) + cy * sinT,
            ty: cy * (1 - cosT) - cx * sinT
        )
        try await assertAlignment(recovers: applied)
    }

    @Test func canAlignCombined() async throws {
        let (cx, cy) = imageCenter

        let theta: Float = 0.3 * .pi / 180
        let cosT = cos(theta), sinT = sin(theta)
        let rotation = AffineTransform2D(
            a: cosT, b: sinT, c: -sinT, d: cosT,
            tx: cx * (1 - cosT) + cy * sinT,
            ty: cy * (1 - cosT) - cx * sinT
        )

        let s: Float = 1.015
        let scale = AffineTransform2D(
            a: s, b: 0, c: 0, d: s,
            tx: cx * (1 - s), ty: cy * (1 - s)
        )

        let translation = AffineTransform2D(a: 1, b: 0, c: 0, d: 1, tx: 3.0, ty: -2.0)

        let applied = translation.composed(with: scale).composed(with: rotation)
        try await assertAlignment(recovers: applied)
    }

    @Test func canChainTranslations() async throws {
        let t1 = AffineTransform2D(a: 1, b: 0, c: 0, d: 1, tx: 4.0, ty: -2.0)
        let t2 = AffineTransform2D(a: 1, b: 0, c: 0, d: 1, tx: -3.0, ty: 5.0)

        let tex0 = reference
        let tex1 = try await applyTransform(t1)
        let tex2 = try await applyTransform(t1.composed(with: t2))

        // Pairwise alignment (same as StackingSession does)
        let pair01 = await aligner.findAlignment(reference: tex0, source: tex1)
        let pair12 = await aligner.findAlignment(reference: tex1, source: tex2)

        // Feed through ChainAligner
        var chain = ChainAligner(aligner: aligner)
        let cum0 = chain.addImage(tex0, pairwiseResult: nil)
        let cum1 = chain.addImage(tex1, pairwiseResult: pair01)
        let cum2 = chain.addImage(tex2, pairwiseResult: pair12)

        let transforms = chain.getAllTransforms()
        #expect(transforms.count == 3)

        assertTransformMatch(cum0, expected: .identity, threshold: 0.001, label: "frame 0")
        assertTransformMatch(cum1, expected: t1, threshold: 0.5, label: "frame 1")
        assertTransformMatch(cum2, expected: t1.composed(with: t2), threshold: 1.0, label: "frame 2")
    }

    // MARK: - Helpers

    private func assertAlignment(
        recovers expected: AffineTransform2D,
        threshold: Float = 0.5,
        sourceLocation: SourceLocation = #_sourceLocation
    ) async throws {
        let transformed = try await applyTransform(expected)
        let result = await aligner.findAlignment(reference: reference, source: transformed)
        #expect(result.converged, "ECC did not converge", sourceLocation: sourceLocation)
        assertTransformMatch(result.transform, expected: expected, threshold: threshold,
                             label: "converged: \(result.converged), confidence: \(result.confidence), iterations: \(result.iterations)",
                             sourceLocation: sourceLocation)
    }

    /// Apply an AffineTransform2D to the test image via CoreImage, converting
    /// from our top-left origin convention to CIImage's bottom-left origin.
    private func applyTransform(_ transform: AffineTransform2D) async throws -> MTLTexture {
        let width = CVPixelBufferGetWidth(pixelBuffer)
        let height = CVPixelBufferGetHeight(pixelBuffer)
        let h = CGFloat(height)

        // M_bl = F * T * F^{-1} where F flips Y around height
        let cgTransform = CGAffineTransform(
            a: CGFloat(transform.a), b: CGFloat(-transform.b),
            c: CGFloat(-transform.c), d: CGFloat(transform.d),
            tx: CGFloat(transform.c) * h + CGFloat(transform.tx),
            ty: h * (1 - CGFloat(transform.d)) - CGFloat(transform.ty)
        )

        let ciImage = CIImage(cvPixelBuffer: pixelBuffer)
            .transformed(by: cgTransform)
            .cropped(to: CGRect(x: 0, y: 0, width: width, height: height))

        let dstBuffer = try #require(
            CVPixelBuffer.create(width: width, height: height, pixelFormat: kCVPixelFormatType_64RGBALE)
        )
        context.ciContext.render(
            ciImage, to: dstBuffer,
            bounds: CGRect(x: 0, y: 0, width: width, height: height),
            colorSpace: CGColorSpace(name: CGColorSpace.linearSRGB)
        )

        return try #require(
            await dstBuffer.standaloneMetalTexture(
                device: context.device,
                commandQueue: context.commandQueue,
                cache: context.textureCache, label: "transformed"
            )
        )
    }
}

// MARK: - Assertion Utility

private func assertTransformMatch(
    _ actual: AffineTransform2D,
    expected: AffineTransform2D,
    threshold: Float,
    label: String = "",
    sourceLocation: SourceLocation = #_sourceLocation
) {
    let fields: [(String, Float, Float)] = [
        ("a",  actual.a,  expected.a),
        ("b",  actual.b,  expected.b),
        ("c",  actual.c,  expected.c),
        ("d",  actual.d,  expected.d),
        ("tx", actual.tx, expected.tx),
        ("ty", actual.ty, expected.ty),
    ]

    var maxError: Float = 0
    for (_, act, exp) in fields {
        maxError = max(maxError, abs(act - exp))
    }

    let prefix = label.isEmpty ? "" : "\(label) — "
    print("\(prefix)max error: \(maxError)")
    print("  Expected  → a=\(expected.a) b=\(expected.b) c=\(expected.c) d=\(expected.d) tx=\(expected.tx) ty=\(expected.ty)")
    print("  Recovered → a=\(actual.a) b=\(actual.b) c=\(actual.c) d=\(actual.d) tx=\(actual.tx) ty=\(actual.ty)")

    for (name, act, exp) in fields {
        #expect(
            abs(act - exp) < threshold,
            "\(prefix)\(name): \(act) not within \(threshold) of expected \(exp)",
            sourceLocation: sourceLocation
        )
    }
}
