import UIKit
@preconcurrency import DeptheosEngine

/// Each lifecycle transition, one line.
private let stateLog = AppLog(category: "state")

/// One level of a tiled picture: its pixel size and tiles across and down.
/// Level 0 is full size; each after is half the one before.
nonisolated struct TileLevel: Equatable, Sendable {
    let width: UInt32
    let height: UInt32
    let cols: UInt32
    let rows: UInt32
}

extension TileLevel {
    nonisolated init(_ level: PaintLevel) {
        self.init(width: level.width, height: level.height, cols: level.cols, rows: level.rows)
    }
}

/// A tile of one level of a tiled picture.
nonisolated struct TileKey: Hashable, Sendable {
    let level: UInt32
    let tx: UInt32
    let ty: UInt32
}

/// A picture in tiles at levels, each half the one before, as the engine
/// keeps its displays: tiles `tileCore` pixels square (smaller at the right
/// and bottom) with a one-pixel gutter of their neighbors' pixels, asked a
/// few at a time from the engine's threads. A missing full-size tile shows
/// a quarter of the half-size tile under it until its owner says it changed
/// (`TiledPictureView.tilesChanged`).
///
/// One object per picture: a view tells sources apart by identity.
nonisolated protocol TileSource: AnyObject, Sendable {
    /// Full size first.
    var tileLevels: [TileLevel] { get }
    /// Without its gutter, in its level's pixels.
    var tileCore: UInt32 { get }
    /// Each with its gutter, in order: the tile, or when missing and a
    /// stand-in is asked, the one under it a level coarser.
    func fetchTiles(_ asked: [TileAsk], request: Request) async throws -> [TileFetched]
}

nonisolated extension TileSource {
    var tileCore: UInt32 { paintTileCore() }
}

/// An engine picture (the result, the depth map) as tiles, finest level
/// first. The depth map's finest is half size, so a view at full size draws
/// it at its own resolution.
extension Picture: TileSource {
    nonisolated var tileLevels: [TileLevel] {
        levels().map { TileLevel(width: $0.width, height: $0.height, cols: $0.cols, rows: $0.rows) }
    }

    nonisolated func fetchTiles(_ asked: [TileAsk], request: Request) async throws -> [TileFetched] {
        try await tiles(request: request, asked: asked)
    }
}

/// What a `TiledPictureView` draws.
///
/// Lifecycle: empty, base (the base alone) or tiling (a level's tiles, over
/// the base if any). Held in `TiledPictureView.drawing`; its case changes
/// only in `redraw`, the tiles in `tiling` as they land (ADR-164).
///
/// - `empty` → `base`: a base, with no source or zoomed out to its
///   resolution.
/// - `empty` or `base` → `tiling`: a source without a base, or zoomed past
///   the base. What was drawn stays under until the tiles in view land.
/// - `tiling` → `tiling` at another level as the zoom changes, the level
///   before staying under until those in view land.
/// - `tiling` → `base`: zoomed out to the base, or the source taken away.
/// - any → `empty`: the picture taken away.
enum TiledDrawing {
    case empty
    /// Stretched over the view; the source isn't needed at this zoom.
    case base(CGImage, source: (any TileSource)?)
    /// Over the base when there is one.
    case tiling(Tiling)

    var logName: String {
        switch self {
        case .empty: "empty"
        case .base(_, let source): source == nil ? "base" : "base, tiles not needed"
        case .tiling(let tiling): "tiling level \(tiling.level)\(tiling.base == nil ? "" : " over the base")"
        }
    }
}

/// The tiling state's own: source and level, tiles shown, wanted and to
/// fetch, and what stays under until they land.
struct Tiling {
    let source: any TileSource
    let levels: [TileLevel]
    let base: CGImage?
    let level: UInt32
    /// Without its gutter, in the level's pixels.
    let core: CGFloat
    /// In view, plus a tile's width around.
    var wanted: Set<TileKey> = []
    /// Each in its layer.
    var layers: [TileKey: CALayer] = [:]
    /// Newest last: fetched first.
    var queued: [TileKey] = []
    /// Shown under the tiles until every one wanted has landed.
    var under: [CALayer] = []
}

/// A picture drawn one point per full-size pixel for a scroll view to zoom:
/// a base stretched over the view, and zoomed past its resolution, the
/// `TileSource`'s tiles for the zoom in view plus a tile's width. Without a
/// base it always tiles (Retouch's canvas).
///
/// The owner sizes it (bounds are full size), reports what is in view
/// (`update(visible:scale:)`), and says when tiles changed. Its own
/// sublayers go above the tiles.
///
/// Tiles are fetched a few at a time, newest first; each lands only if its
/// source and level are still the ones shown.
final class TiledPictureView: UIView {
    private(set) var drawing = TiledDrawing.empty {
        didSet {
            if drawing.logName != oldValue.logName {
                stateLog.debug("tiles: \(oldValue.logName, privacy: .public) → \(drawing.logName, privacy: .public)")
            }
            let full = if case .tiling(let tiling) = drawing, tiling.level == 0 { tiling.wanted.count } else { 0 }
            if full != fullTilesWanted {
                fullTilesWanted = full
                onFullTilesWanted?()
            }
        }
    }

    /// In view plus a tile's width while tiling the full size, else none.
    /// Told through `onFullTilesWanted` before they are asked for.
    private(set) var fullTilesWanted = 0
    var onFullTilesWanted: (() -> Void)?

    private var base: CGImage?
    private var source: (any TileSource)?
    private var levels: [TileLevel] = []
    /// In the view's coordinates, and screen pixels per point.
    private var visible = CGRect.null
    private var scale: CGFloat = 1
    /// `nil` before the first `update`.
    private var updatedSize: CGSize?

    /// The level before and anything else kept under.
    private let underLayer = CALayer()
    private let tilesLayer = CALayer()
    /// One batch at a time.
    private var fetching = false
    private static let batch = 4

    override init(frame: CGRect) {
        super.init(frame: frame)
        layer.addSublayer(underLayer)
        layer.addSublayer(tilesLayer)
    }

    required init?(coder: NSCoder) { fatalError("not used") }

    /// Show `base` stretched over the view, with `source`'s tiles where the
    /// zoom wants more; either can be `nil`. Nothing when unchanged.
    func show(base: CGImage?, source: (any TileSource)?) {
        guard base !== self.base || source !== self.source else { return }
        self.base = base
        self.source = source
        levels = source?.tileLevels ?? []
        withoutAnimation { layer.contents = base }
        redraw()
    }

    /// Keeping the base (the app went to the background).
    func dropSource() {
        show(base: base, source: nil)
    }

    /// What is in sight, in the view's coordinates, and screen pixels per
    /// point. Nothing when neither nor the size changed: an owner laid out
    /// on every SwiftUI update asks repeatedly.
    func update(visible: CGRect, scale: CGFloat) {
        guard visible != self.visible || scale != self.scale || bounds.size != updatedSize else { return }
        self.visible = visible
        self.scale = scale
        updatedSize = bounds.size
        redraw()
    }

    /// Changed tiles, or ones the source lacked and has now: those wanted
    /// are fetched again.
    func tilesChanged(_ keys: [TileKey]) {
        guard case .tiling(var tiling) = drawing else { return }
        for key in keys where tiling.wanted.contains(key) && !tiling.queued.contains(key) {
            tiling.queued.append(key)
        }
        drawing = .tiling(tiling)
        fetch()
    }

    // MARK: Drawing

    private func redraw() {
        guard let source, !levels.isEmpty, bounds.width > 0, bounds.height > 0 else {
            leaveTiling()
            drawing = base.map { .base($0, source: source) } ?? .empty
            return
        }
        let k = level(for: scale)
        if let base, baseSuffices(base, level: levels[Int(k)]) {
            leaveTiling()
            drawing = .base(base, source: source)
            return
        }
        var tiling: Tiling
        if case .tiling(let current) = drawing, current.source === source, current.base === base,
           current.level == k
        {
            tiling = current
        } else {
            tiling = Tiling(
                source: source, levels: levels, base: base, level: k, core: CGFloat(source.tileCore))
            switch drawing {
            case .tiling(let current):
                // The level or picture before stays under until the new tiles
                // are in.
                tiling.under = current.under + current.layers.values
                withoutAnimation {
                    for layer in current.layers.values { underLayer.addSublayer(layer) }
                }
            case .base(let shown, _) where shown !== base:
                // Another picture's base, until this one's tiles are in.
                tiling.under = [standInLayer(shown)]
            case .empty, .base:
                break
            }
        }
        updateWanted(&tiling)
        drawing = .tiling(tiling)
        fetch()
    }

    /// The largest level still reduced on screen, so each screen pixel
    /// averages at most 2×2.
    private func level(for scale: CGFloat) -> UInt32 {
        // Screen pixels per pixel of the full-size level.
        let scale = scale * bounds.width / CGFloat(max(levels[0].width, 1))
        guard scale < 1 else { return 0 }
        let k = Int(floor(log2(1 / scale)))
        return UInt32(min(max(k, 0), levels.count - 1))
    }

    /// Shown at most a screen pixel per pixel, or the zoom's level no finer.
    private func baseSuffices(_ base: CGImage, level: TileLevel) -> Bool {
        scale * bounds.width <= CGFloat(base.width) || level.width <= UInt32(base.width)
    }

    /// Want the tiles in view plus a tile's width, drop the rest, and queue
    /// the missing ones, nearest the middle newest.
    private func updateWanted(_ tiling: inout Tiling) {
        let level = tiling.levels[Int(tiling.level)]
        let core = tiling.core
        // View points per level pixel.
        let sx = bounds.width / CGFloat(level.width)
        let sy = bounds.height / CGFloat(level.height)
        let seen = visible
            .insetBy(dx: -core * sx, dy: -core * sy)
            .intersection(bounds)
        var wanted = Set<TileKey>()
        if !visible.isNull, !seen.isNull, !seen.isEmpty {
            let range = { (lo: CGFloat, hi: CGFloat, step: CGFloat, count: UInt32) -> ClosedRange<UInt32> in
                let first = UInt32(max(floor(lo / step), 0))
                let last = UInt32(min(max(ceil(hi / step) - 1, 0), CGFloat(count - 1)))
                return first...max(first, last)
            }
            let cols = range(seen.minX, seen.maxX, core * sx, level.cols)
            let rows = range(seen.minY, seen.maxY, core * sy, level.rows)
            for ty in rows {
                for tx in cols { wanted.insert(TileKey(level: tiling.level, tx: tx, ty: ty)) }
            }
        }
        withoutAnimation {
            for (key, layer) in tiling.layers where !wanted.contains(key) {
                layer.removeFromSuperlayer()
                tiling.layers[key] = nil
            }
        }
        tiling.wanted = wanted
        tiling.queued.removeAll { !wanted.contains($0) }
        let middle = CGPoint(x: visible.midX / (core * sx), y: visible.midY / (core * sy))
        let distance = { (key: TileKey) -> CGFloat in
            hypot(CGFloat(key.tx) + 0.5 - middle.x, CGFloat(key.ty) + 0.5 - middle.y)
        }
        let missing = wanted
            .filter { tiling.layers[$0] == nil && !tiling.queued.contains($0) }
            .sorted { distance($0) > distance($1) }
        tiling.queued += missing
        dropUnderIfCovered(&tiling)
    }

    private func dropUnderIfCovered(_ tiling: inout Tiling) {
        guard !tiling.under.isEmpty, tiling.wanted.allSatisfy({ tiling.layers[$0] != nil }) else { return }
        withoutAnimation {
            for layer in tiling.under { layer.removeFromSuperlayer() }
        }
        tiling.under = []
    }

    private func leaveTiling() {
        guard case .tiling(let tiling) = drawing else { return }
        withoutAnimation {
            for layer in tiling.layers.values { layer.removeFromSuperlayer() }
            for layer in tiling.under { layer.removeFromSuperlayer() }
        }
    }

    /// Over the whole view, to keep under tiles.
    private func standInLayer(_ picture: CGImage) -> CALayer {
        let layer = CALayer()
        withoutAnimation {
            layer.frame = bounds
            layer.contents = picture
            underLayer.addSublayer(layer)
        }
        return layer
    }

    // MARK: Fetching

    private enum Fetched: Sendable {
        case tile(CGImage)
        /// Not there yet: the tile a level coarser, to show a quarter of.
        case coarser(CGImage)
        case nothing
    }

    /// Unless one is under way, which fetches the next when it lands.
    private func fetch() {
        guard !fetching, case .tiling(var tiling) = drawing, !tiling.queued.isEmpty else { return }
        let keys = Array(tiling.queued.suffix(Self.batch).reversed())
        tiling.queued.removeLast(keys.count)
        // A missing tile stands in with the coarser one; a shown one keeps
        // what it shows.
        let asked = keys.map { TileAsk(level: $0.level, tx: $0.tx, ty: $0.ty, standIn: tiling.layers[$0] == nil) }
        drawing = .tiling(tiling)
        fetching = true
        let source = tiling.source
        Task { [weak self] in
            do {
                let fetched = try await Request().run { try await source.fetchTiles(asked, request: $0) }
                self?.landed(zip(keys, fetched).map { ($0, Self.drawable($1)) }, from: source)
            } catch {
                self?.failed(keys, from: source)
            }
        }
    }

    private static let retryAfter = Duration.milliseconds(500)

    /// Requeue the tiles still wanted behind the rest and go on after a
    /// pause, so a lasting failure doesn't loop.
    private func failed(_ keys: [TileKey], from source: any TileSource) {
        fetching = false
        if case .tiling(var tiling) = drawing, tiling.source === source {
            let again = keys.filter {
                $0.level == tiling.level && tiling.wanted.contains($0) && tiling.layers[$0] == nil
                    && !tiling.queued.contains($0)
            }
            tiling.queued.insert(contentsOf: again, at: 0)
            drawing = .tiling(tiling)
        }
        Task { [weak self] in
            try? await Task.sleep(for: Self.retryAfter)
            self?.fetch()
        }
    }

    private static func drawable(_ fetched: TileFetched) -> Fetched {
        switch fetched {
        case .tile(let image): image.makeCGImage().map(Fetched.tile) ?? .nothing
        case .coarser(let image): image.makeCGImage().map(Fetched.coarser) ?? .nothing
        case .nothing: .nothing
        }
    }

    /// Each shown only while its source is shown and it is still wanted at
    /// the level tiled.
    private func landed(_ fetched: [(TileKey, Fetched)], from source: any TileSource) {
        fetching = false
        guard case .tiling(var tiling) = drawing, tiling.source === source else {
            fetch()
            return
        }
        withoutAnimation {
            for (key, result) in fetched where key.level == tiling.level && tiling.wanted.contains(key) {
                place(key, result, in: &tiling)
            }
        }
        dropUnderIfCovered(&tiling)
        drawing = .tiling(tiling)
        fetch()
    }

    private func place(_ key: TileKey, _ fetched: Fetched, in tiling: inout Tiling) {
        if case .nothing = fetched { return }
        let level = tiling.levels[Int(key.level)]
        let tileCore = tiling.core
        let sx = bounds.width / CGFloat(level.width)
        let sy = bounds.height / CGFloat(level.height)
        // The part of the level the tile covers, without its gutter.
        let core = CGSize(
            width: min(tileCore, CGFloat(level.width) - CGFloat(key.tx) * tileCore),
            height: min(tileCore, CGFloat(level.height) - CGFloat(key.ty) * tileCore))
        let layer = tiling.layers[key] ?? {
            let layer = CALayer()
            layer.magnificationFilter = .nearest
            layer.minificationFilter = .linear
            tilesLayer.addSublayer(layer)
            tiling.layers[key] = layer
            return layer
        }()
        layer.frame = CGRect(
            x: CGFloat(key.tx) * tileCore * sx, y: CGFloat(key.ty) * tileCore * sy,
            width: core.width * sx, height: core.height * sy)
        switch fetched {
        case .tile(let image):
            // The gutter repeats a pixel of each neighbor so the tile
            // filters at its edges as the whole image would; it is drawn
            // outside the layer.
            let (w, h) = (CGFloat(image.width), CGFloat(image.height))
            layer.contentsRect = CGRect(x: 1 / w, y: 1 / h, width: (w - 2) / w, height: (h - 2) / h)
            layer.contents = image
        case .coarser(let half):
            // A quarter of the coarser tile, each pixel covering 2×2 of this
            // one's.
            let (w, h) = (CGFloat(half.width), CGFloat(half.height))
            let x = 1 + CGFloat(key.tx % 2) * tileCore / 2
            let y = 1 + CGFloat(key.ty % 2) * tileCore / 2
            layer.contentsRect = CGRect(x: x / w, y: y / h, width: core.width / 2 / w, height: core.height / 2 / h)
            layer.contents = half
        case .nothing:
            break
        }
    }

    private func withoutAnimation(_ body: () -> Void) {
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        body()
        CATransaction.commit()
    }
}
