import SwiftUI

extension CameraView {
    /// The screen's size inside the safe area, how far it runs past it, and
    /// its fold (iPhone Duo, iOS 27.1).
    nonisolated struct Shape: Equatable, Sendable {
        var size = CGSize.zero
        var insets = EdgeInsets()
        var fold: Fold?

        /// In whole points, so a reading a fraction off isn't a change.
        var rounded: Shape {
            Shape(
                size: CGSize(width: size.width.rounded(), height: size.height.rounded()),
                insets: insets.rounded,
                fold: fold.map { Fold(frame: $0.frame.integral, active: $0.active) })
        }
    }

    /// Where the screen folds, inside the safe area.
    nonisolated struct Fold: Equatable, Sendable {
        var frame: CGRect
        /// It divides the screen (partly open); flat, it is only a crease.
        var active: Bool
    }

    /// Where the viewfinder and controls go, from the screen's and the
    /// picture's shapes. The picture is fitted whole first and the controls
    /// take the room left; every layout has the same controls in the same
    /// order.
    ///
    /// Lifecycle: overlaid, below, beside, book or table, derived each time
    /// by `of(_:picture:phoneUpright:)` from `CameraView.layout`, never
    /// stored (ADR-162); `CameraView` logs each change.
    ///
    ///     overlaid: the viewfinder across the screen, the bars over it;
    ///               centered where the picture then ends above the shutter
    ///               (an iPhone upright), else pinned to the safe area's top
    ///               (the Duo's outer display, a short iPhone)
    ///     below:    the viewfinder at the top, the controls under it (an
    ///               iPad upright)
    ///     beside:   the viewfinder leading, every control in the trailing
    ///               column (the Duo's inner display, an iPad or iPhone on
    ///               its side); `tight` on a short screen
    ///     book:     a fold top to bottom: viewfinder leading, controls
    ///               trailing
    ///     table:    a fold across: viewfinder above, controls below
    ///
    /// A dividing fold always makes a book or table; a flat one only if the
    /// picture loses little (`creaseShare`), so folding partly moves
    /// nothing. Of `below` and `beside`, the bigger picture wins.
    enum ScreenLayout: Equatable {
        /// `pinned`: the picture's height at the safe area's top; nil
        /// centers it on the whole screen.
        case overlaid(pinned: CGFloat?)
        /// The picture's height.
        case below(picture: CGFloat)
        /// The column's width, inside its padding.
        case beside(column: CGFloat, tight: Bool)
        /// The fold's leading and trailing edges.
        case book(leading: CGFloat, trailing: CGFloat)
        /// The fold's top and bottom edges.
        case table(top: CGFloat, bottom: CGFloat)

        /// `picture` is width over height as the preview shows it; until it
        /// says, its long side runs along the screen's. `phoneUpright`:
        /// compact width, regular height.
        static func of(_ shape: Shape, picture: CGFloat?, phoneUpright: Bool) -> ScreenLayout {
            let (width, height) = (shape.size.width, shape.size.height)
            guard width > 0, height > 0 else { return .overlaid(pinned: nil) }
            let full = CGSize(
                width: width + shape.insets.leading + shape.insets.trailing,
                height: height + shape.insets.top + shape.insets.bottom)
            let aspect = picture ?? (full.width > full.height ? 4.0 / 3 : 3.0 / 4)
            let divided = shape.fold.flatMap { divided(shape, by: $0.frame) }
            if let divided, shape.fold?.active == true { return divided }

            // A flat fold: the picture keeps to one side if that costs it
            // little.
            if let divided {
                let room =
                    switch divided {
                    case .book(let leading, _): CGSize(width: leading, height: height)
                    case .table(let top, _): CGSize(width: width, height: top)
                    default: CGSize.zero
                    }
                if area(fitted(aspect, in: room)) >= creaseShare * area(fitted(aspect, in: shape.size)) {
                    return divided
                }
            }

            // Centered on the whole screen, the picture ends above the
            // shutter: the controls cover only its lower edge.
            let centered = fitted(aspect, in: full)
            let shutterTop = shape.insets.top + height - edgePadding - shutterSide
            if (full.height + centered.height) / 2 <= shutterTop { return .overlaid(pinned: nil) }
            if phoneUpright { return .overlaid(pinned: min(height, width / aspect)) }

            let tight = height < tightHeight
            let beside = fitted(
                aspect,
                in: CGSize(width: width - (tight ? tightColumn : column) - 2 * edgePadding, height: height))
            let below = fitted(aspect, in: CGSize(width: width, height: height - controlsHeight))
            return area(beside) >= area(below)
                ? .beside(column: width - 2 * edgePadding - beside.width, tight: tight)
                : .below(picture: below.height)
        }

        /// If the fold crosses the screen inside its edges.
        private static func divided(_ shape: Shape, by fold: CGRect) -> ScreenLayout? {
            if fold.width > fold.height, fold.minY > 0, fold.maxY < shape.size.height {
                return .table(top: fold.minY, bottom: fold.maxY)
            }
            if fold.height > fold.width, fold.minX > 0, fold.maxX < shape.size.width {
                return .book(leading: fold.minX, trailing: fold.maxX)
            }
            return nil
        }

        /// Of `aspect` (width over height).
        private static func fitted(_ aspect: CGFloat, in room: CGSize) -> CGSize {
            guard room.width > 0, room.height > 0, aspect > 0 else { return .zero }
            let width = min(room.width, room.height * aspect)
            return CGSize(width: width, height: width / aspect)
        }

        private static func area(_ size: CGSize) -> CGFloat { size.width * size.height }

        /// `padding()`'s default, kept from the safe area's edges.
        static let edgePadding: CGFloat = 16
        /// The shutter's outer ring's diameter.
        static let shutterSide: CGFloat = 78
        /// The least width beside the picture: the step slider stays usable.
        static let column: CGFloat = 320
        /// On a short screen: fits the top bar's row, the mode switch beside
        /// the shutter.
        static let tightColumn: CGFloat = 280
        /// Under this height the screen is short: the column is `tight`.
        static let tightHeight: CGFloat = 480
        /// Kept under the picture for the bracket's controls, hint, mode row
        /// and shutter, so the picture is one size in Sweep and Bracket.
        static let controlsHeight: CGFloat = 320
        /// A flat fold is kept clear of when the picture on one side has at
        /// least this share of the biggest the screen could hold.
        static let creaseShare: CGFloat = 0.8

        /// For the log.
        var name: String {
            switch self {
            case .overlaid(pinned: nil): "overlaid"
            case .overlaid: "overlaid, pinned"
            case .below: "below"
            case .beside(_, tight: false): "beside"
            case .beside: "beside, tight"
            case .book: "book"
            case .table: "table"
            }
        }
    }
}
