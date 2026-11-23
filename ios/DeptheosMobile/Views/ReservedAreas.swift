import SwiftUI

/// Where hardware crosses a view (iOS 27.1): the fold of a partly open
/// device (a division) and the inner camera (an occlusion), in the view's
/// coordinates from its leading edge. Empty before iOS 27.1 and on a
/// screen with neither.
nonisolated struct ReservedAreas: Equatable, Sendable {
    /// While it divides the view (a device partly open).
    var fold: CGRect?
    /// Whether or not it divides the view now: what a layout that must not
    /// jump keys on.
    var anyFold: CGRect?
    /// The inner camera, while it covers the view.
    var occlusions: [CGRect] = []
    /// What the regions are measured against.
    var size: CGSize = .zero

    static func read(_ proxy: GeometryProxy) -> ReservedAreas {
        var areas = ReservedAreas(size: proxy.size)
        guard #available(iOS 27.1, *) else { return areas }
        let bounds = CGRect(origin: .zero, size: proxy.size)
        let divisions = proxy.reservedRegions(kind: .division, options: .includeInactive)
            .filter { $0.frame.intersects(bounds) }
        areas.anyFold = divisions.first?.frame
        areas.fold = divisions.first(where: \.isActive)?.frame
        areas.occlusions = proxy.reservedRegions(kind: .occlusion)
            .filter { $0.isActive && $0.frame.intersects(bounds) }
            .map(\.frame)
        #if DEBUG
        // `-FakeOcclusion x,y,width,height`: an inner camera there, in the
        // screen's points, for a Simulator, which has none.
        if let fake = fakeOcclusion {
            let origin = proxy.frame(in: .global).origin
            let local = fake.offsetBy(dx: -origin.x, dy: -origin.y)
            if local.intersects(bounds) { areas.occlusions.append(local) }
        }
        #endif
        return areas.rounded
    }

    #if DEBUG
    private static let fakeOcclusion: CGRect? = {
        let parts = (UserDefaults.standard.string(forKey: "FakeOcclusion") ?? "")
            .split(separator: ",").compactMap { Double($0) }
        guard parts.count == 4 else { return nil }
        return CGRect(x: parts[0], y: parts[1], width: parts[2], height: parts[3])
    }()
    #endif

    /// In whole points, so a view that moves by less reads the same areas.
    private var rounded: ReservedAreas {
        func round(_ rect: CGRect) -> CGRect {
            CGRect(x: rect.minX.rounded(), y: rect.minY.rounded(),
                   width: rect.width.rounded(), height: rect.height.rounded())
        }
        var areas = self
        areas.fold = fold.map(round)
        areas.anyFold = anyFold.map(round)
        areas.occlusions = occlusions.map(round)
        areas.size = CGSize(width: size.width.rounded(), height: size.height.rounded())
        return areas
    }

    /// A top-to-bottom fold with room both sides: the leading side's width
    /// and the fold's (at least `minGutter`).
    func verticalFold(minGutter: CGFloat = 2) -> (leading: CGFloat, gutter: CGFloat)? {
        guard let fold, fold.height > fold.width, fold.minX > 0, fold.maxX < size.width else { return nil }
        return (fold.minX, max(fold.width, minGutter))
    }

    /// A side-to-side fold with room both sides: the top's height and the
    /// fold's (at least `minGutter`).
    func horizontalFold(minGutter: CGFloat = 2) -> (top: CGFloat, gutter: CGFloat)? {
        guard let fold, fold.width > fold.height, fold.minY > 0, fold.maxY < size.height else { return nil }
        return (fold.minY, max(fold.height, minGutter))
    }

    /// Folded or not.
    var foldsVertically: Bool {
        guard let anyFold else { return false }
        return anyFold.height > anyFold.width
    }

    /// How far occlusions reach in from each edge they lie along, those
    /// starting within `band` of it, at most `band` each: what controls on
    /// that edge move in by. An occlusion lies along the edge it is longer
    /// than deep against: an inner camera at the top is wide, an iPad
    /// Pro's in landscape a strip the screen's height down one side.
    func occludedInsets(within band: CGFloat) -> EdgeInsets {
        var insets = EdgeInsets()
        for region in occlusions {
            if region.width >= region.height {
                if region.minY < band { insets.top = max(insets.top, region.maxY) }
                if region.maxY > size.height - band { insets.bottom = max(insets.bottom, size.height - region.minY) }
            } else {
                if region.minX < band { insets.leading = max(insets.leading, region.maxX) }
                if region.maxX > size.width - band { insets.trailing = max(insets.trailing, size.width - region.minX) }
            }
        }
        func capped(_ depth: CGFloat) -> CGFloat { min(max(depth, 0), band) }
        return EdgeInsets(
            top: capped(insets.top), leading: capped(insets.leading),
            bottom: capped(insets.bottom), trailing: capped(insets.trailing))
    }
}

nonisolated extension EdgeInsets {
    /// In whole points.
    var rounded: EdgeInsets {
        EdgeInsets(top: top.rounded(), leading: leading.rounded(), bottom: bottom.rounded(), trailing: trailing.rounded())
    }
}
