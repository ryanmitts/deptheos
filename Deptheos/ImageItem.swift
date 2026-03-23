import Foundation
import CoreGraphics

/// A single image in the stacking workspace.
struct ImageItem: Identifiable {
    let id: UUID
    let url: URL
    let filename: String
    var thumbnail: CGImage?
    var isIncluded: Bool

    init(url: URL, isIncluded: Bool = true) {
        self.id = UUID()
        self.url = url
        self.filename = url.lastPathComponent
        self.isIncluded = isIncluded
    }
}
