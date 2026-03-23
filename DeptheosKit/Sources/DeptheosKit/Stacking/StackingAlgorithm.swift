//
//  StackingAlgorithm.swift
//  DeptheosKit
//
//  Created by Ryan Mitts on 2026-03-23.
//
import Metal

public protocol StackingAlgorithm: AnyObject, Sendable {
    func accumulate(image: DeptheosImage, transform: AffineTransform2D?) throws
    func finalize() async throws
    
    var resultTexture: MTLTexture? { get }
    var focusMapTexture: MTLTexture? { get }
    var frameCount: Int { get }
    /// Release scratch textures, keeping result + focus map alive.
    func releaseGPUResources()
    /// Explicitly free the result texture.
    func releaseResultTexture()
}

protocol BlendableAlgorithm: StackingAlgorithm {
    /// Run the finalization pipeline between accumulate and blend phases.
    func runFinalizationPipeline() async throws
    func beginBlend() throws
    func blendFrame(
        frame: DeptheosImage,
        transform: AffineTransform2D?,
        frameIndex: Int,
    ) throws
}

public enum StackingError: Error {
    case noFrames
    case invalidState(String)
    case unsupported(String)
    case metalError(String)
    case dimensionMismatch(expected: (Int, Int), got: (Int, Int))
}
