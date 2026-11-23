import Foundation
@preconcurrency import DeptheosEngine
import Observation

/// The raw value is what `@AppStorage` keeps, so it never changes.
enum AlgorithmKind: String, CaseIterable, Identifiable, Sendable {
    case pyramid
    case depth
    case wavelet
    var id: Self { self }

    init(_ algorithm: Algorithm) {
        self = switch algorithm {
        case .pyramid: .pyramid
        case .depth: .depth
        case .wavelet: .wavelet
        }
    }

    var label: LocalizedStringResource {
        switch self {
        case .pyramid: LocalizedStringResource("Pyramid", comment: "The name of a stacking method, Pyramid.")
        case .depth: LocalizedStringResource("Depth", comment: "The name of a stacking method, Depth.")
        case .wavelet: LocalizedStringResource("Wavelet", comment: "The name of a stacking method, Wavelet.")
        }
    }
}

/// The stacking settings as the inspector edits them: the chosen algorithm
/// and every algorithm's values (kept while another is chosen), alignment,
/// crop, and the RAW decoder (ADR-131).
@Observable
final class StackSettingsModel {
    let ranges = algorithmRanges()
    var algorithm = AlgorithmKind.pyramid
    var smoothness: Float = 0
    var radius: Float = 0
    var smoothing: Float = 0
    var blendRange: Float = 0
    var denoise: Float = 0
    /// The wavelet consistency filter: set from the command line only, kept
    /// from a restored render so it stacks the same.
    var consistency: UInt32 = 0
    var align = true
    var alignMethod = AlignMode.ecc
    var crop = true
    /// Apple's (rendered: TIFF, JPEG or PNG) or LibRaw (linear: DNG too),
    /// for camera RAW other than DNG.
    var rawDecoder = RawBackend.apple

    init() {
        reset()
    }

    var settings: StackSettings {
        let algorithm: Algorithm = switch self.algorithm {
        case .pyramid: .pyramid(smoothness: smoothness)
        case .depth: .depth(radius: radius, smoothing: smoothing, blendRange: blendRange)
        case .wavelet: .wavelet(denoise: denoise, consistency: consistency)
        }
        return StackSettings(
            algorithm: algorithm, align: align, alignMethod: alignMethod, crop: crop,
            rawDecoder: rawDecoder)
    }

    /// Every value to its default, pyramid chosen.
    func reset() {
        use(algorithm: .depth(
            radius: ranges.radius.defaultValue, smoothing: ranges.smoothing.defaultValue,
            blendRange: ranges.blendRange.defaultValue))
        use(algorithm: .wavelet(denoise: ranges.denoise.defaultValue, consistency: 0))
        // Last, so it is the one chosen.
        use(algorithm: .pyramid(smoothness: ranges.smoothness.defaultValue))
        align = true
        alignMethod = .ecc
        crop = true
        rawDecoder = .apple
    }

    /// The other algorithms' values stay.
    func use(_ kept: StackSettings) {
        use(algorithm: kept.algorithm)
        align = kept.align
        alignMethod = kept.alignMethod
        crop = kept.crop
        // The desktop's converter isn't here; LibRaw reads the same files linear.
        rawDecoder = kept.rawDecoder == .adobeDng ? .libRaw : kept.rawDecoder
    }

    func use(algorithm chosen: Algorithm) {
        switch chosen {
        case .pyramid(let smoothness):
            algorithm = .pyramid
            self.smoothness = smoothness
        case .depth(let radius, let smoothing, let blendRange):
            algorithm = .depth
            self.radius = radius
            self.smoothing = smoothing
            self.blendRange = blendRange
        case .wavelet(let denoise, let consistency):
            algorithm = .wavelet
            self.denoise = denoise
            self.consistency = consistency
        }
    }
}
