//
//  StackingConfig.swift
//  DeptheosKit
//
//  Created by Ryan Mitts on 2026-03-24.
//

import Foundation

/// Algorithm choice and parameters used for a stacking session.
/// Persisted in project files to allow re-stacking with the same settings.
public enum StackingConfig: Codable, Sendable, Equatable {
    case pyramid(smoothness: Float)
    case depth(smoothing: Float, blendRange: Float, radius: Int)
    case wavelet(denoiseLevel: Float = 0, consistency: Int = 2)
}

extension StackingConfig {
    /// Human-readable algorithm name.
    public var algorithmName: String {
        switch self {
        case .pyramid:  "Pyramid"
        case .depth:    "Depth"
        case .wavelet:  "Wavelet"
        }
    }

    /// Compact shorthand of parameters, e.g. "S4 R8".
    public var settingsSummary: String {
        switch self {
        case .pyramid(let smoothness):
            "S\(formatParam(smoothness))"
        case .depth(let smoothing, let blendRange, let radius):
            "R\(radius) S\(formatParam(smoothing)) B\(formatParam(blendRange))"
        case .wavelet(let denoiseLevel, let consistency):
            "D\(formatParam(denoiseLevel)) C\(consistency)"
        }
    }

    private func formatParam(_ value: Float) -> String {
        value == value.rounded() ? "\(Int(value))" : String(format: "%.1f", value)
    }
}
