import Foundation
import SwiftUI
import DeptheosKit

@Observable
@MainActor
final class WorkspaceState {
    var imageItems: [ImageItem] = []
    var selectedImageIndex: Int?
    private var thumbnailTask: Task<Void, Never>?
    private var thumbnailGeneration: Int = 0
    private let thumbnailLoader = ThumbnailLoader()

    /// Included-only image URLs in order, for stacking.
    var includedURLs: [URL] {
        imageItems.filter(\.isIncluded).map(\.url)
    }

    /// All image URLs (for bookmarks, project paths).
    var allURLs: [URL] {
        imageItems.map(\.url)
    }

    var hasCameraRAWImages: Bool {
        imageItems.contains { ImageFormat.detect(from: $0.url) == .cameraRAW }
    }

    // Algorithm settings
    var selectedAlgorithm: AlgorithmChoice = .pyramid
    var pyramidSmoothness: Float = 4.0
    var depthSmoothing: Float = 5.0
    var depthBlendRange: Float = 5.0
    var depthRadius: Float = 5.0
    var waveletDenoiseLevel: Float = 0
    var waveletConsistency: Int = 2
    var cameraRAWBackend: CameraRAWBackend = .libRaw
    let dngConverterCache = DNGConverterCache()

    init() {
        dngConverterCache.purge()
    }

    // MARK: - Image Management

    func addImages(urls: [URL]) {
        let filtered = urls.filter { ImageFormat.allSupportedExtensions.contains($0.pathExtension.lowercased()) }
        let newItems = filtered.map { ImageItem(url: $0) }
        imageItems.append(contentsOf: newItems)
        if !newItems.isEmpty {
            loadThumbnails(for: newItems)
        }
    }

    func removeAllImages() {
        imageItems.removeAll()
        selectedImageIndex = nil
        thumbnailGeneration += 1
    }

    func removeImage(id: UUID) {
        if let index = imageItems.firstIndex(where: { $0.id == id }) {
            if selectedImageIndex == index {
                selectedImageIndex = nil
            } else if let sel = selectedImageIndex, sel > index {
                selectedImageIndex = sel - 1
            }
            imageItems.remove(at: index)
        }
    }

    func moveImages(fromOffsets source: IndexSet, toOffset destination: Int) {
        let selectedID = selectedImageIndex.map { imageItems[$0].id }
        imageItems.move(fromOffsets: source, toOffset: destination)
        if let selectedID {
            selectedImageIndex = imageItems.firstIndex(where: { $0.id == selectedID })
        }
    }

    /// Reset all algorithm parameters to their default values.
    func resetSettingsToDefaults() {
        pyramidSmoothness = 4.0
        depthSmoothing = 5.0
        depthBlendRange = 5.0
        depthRadius = 5.0
        waveletDenoiseLevel = 0
        waveletConsistency = 2
    }

    func setAllIncluded(_ included: Bool) {
        for i in imageItems.indices {
            imageItems[i].isIncluded = included
        }
    }

    /// Set image items from URLs with optional inclusion state (for project restore).
    func setImageItems(from urls: [URL], included: [Bool]) {
        imageItems = urls.enumerated().map { index, url in
            let isIncluded = index < included.count ? included[index] : true
            return ImageItem(url: url, isIncluded: isIncluded)
        }
        loadThumbnails(for: imageItems)
    }

    /// Restore UI stacking settings from a project's saved configuration.
    func restoreSettings(from project: StackingProject) {
        cameraRAWBackend = project.cameraRAWBackend
        switch project.config {
        case .pyramid(let smoothness):
            selectedAlgorithm = .pyramid
            pyramidSmoothness = smoothness
        case .depth(let smoothing, let blendRange, let radius):
            selectedAlgorithm = .depth
            depthSmoothing = smoothing
            depthBlendRange = blendRange
            depthRadius = Float(radius)
        case .wavelet(let denoiseLevel, let consistency):
            selectedAlgorithm = .wavelet
            waveletDenoiseLevel = denoiseLevel
            waveletConsistency = consistency
        }
    }

    private func loadThumbnails(for items: [ImageItem]) {
        thumbnailTask?.cancel()
        thumbnailGeneration += 1
        let generation = thumbnailGeneration
        let urls = items.map(\.url)
        let loader = thumbnailLoader

        thumbnailTask = Task {
            guard let results = try? await loader.loadThumbnails(for: urls) else { return }
            guard generation == self.thumbnailGeneration else { return }

            for (url, thumb) in results {
                guard let thumb else { continue }
                if let index = self.imageItems.firstIndex(where: { $0.url == url }) {
                    self.imageItems[index].thumbnail = thumb
                }
            }
        }
    }
}

/// Algorithm choice for the UI picker.
enum AlgorithmChoice: String, CaseIterable, Identifiable, Sendable {
    case pyramid = "Pyramid"
    case depth = "Depth"
    case wavelet = "Wavelet"

    var id: String { rawValue }
}
