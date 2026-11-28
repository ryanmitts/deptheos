import CoreGraphics
import Foundation
import os
import SwiftUI
import UIKit
@preconcurrency import DeptheosEngine

nonisolated private let viewLog = AppLog(category: "view")

extension RenderInfo {
    /// The Stack glyph with its top layer filled: marks a render beside
    /// frames, alone in a pill or chip, and in counts; never on a sentence
    /// that already names one.
    nonisolated static let symbol = "square.3.layers.3d.top.filled"

    /// "#4 ★ · sharper · Depth · R8 S5 B3", in the engine's words.
    var menuTitle: UserText {
        .verbatim(AppWords.words.renderMenuTitle(
            number: number, name: name, picked: picked, algorithm: settings.algorithm))
    }

    /// "#4", in the engine's words.
    var numberLabel: UserText {
        .verbatim(AppWords.words.renderName(number: number, name: nil, picked: false))
    }

    /// "#4 · sharper", or "#4 ★ · sharper" with `starred`.
    func nameLabel(starred: Bool = false) -> UserText {
        .verbatim(AppWords.words.renderName(number: number, name: name, picked: starred && picked))
    }

    /// "Render 4, sharper, Pyramid, starred": VoiceOver reads "#4" as
    /// "number 4".
    var spokenLabel: String {
        AppWords.words.renderSpoken(number: number, name: name, algorithm: settings.algorithm, picked: picked)
    }

    /// "Render 4", for VoiceOver.
    var spokenNumber: String {
        AppWords.words.renderSpoken(number: number, name: nil, algorithm: nil, picked: false)
    }
}

/// "Frame 23": frame `number` (from 1) of the list.
func frameLabel(_ number: Int) -> LocalizedStringResource {
    LocalizedStringResource(
        "Frame \(number)", comment: "A frame of the list, by its number from 1. The placeholder is the number.")
}

/// A shown result's display at every size.
///
/// Lifecycle: none, painting (asked of the paint worker) or made. Held in
/// `ShownResult.displayPicture`; changed by `AppModel.fitPictures`,
/// `stopPainting` and the paint's `onPresented`.
///
///     none ──a plain display's made──▶ made
///     none ──a painted display's asked of the paint worker──▶ painting ──presented──▶ made, or none
///     made ──let go of (`fitPictures`)──▶ none
enum DisplayPicture {
    case none
    /// Asked of `PaintSession.present`: not asked again until it lands.
    case painting
    case made(Picture)

    init(_ picture: Picture?) {
        self = picture.map(DisplayPicture.made) ?? .none
    }

    var picture: Picture? {
        if case .made(let picture) = self { picture } else { nil }
    }
}

/// A finished stack on screen. The engine lets go of its own display and
/// map once these are made, so they are the only copies: the flat pictures
/// show first and stand in, and the `Picture`s' tiles show the full size
/// zoomed in past them (ADR-164).
struct ShownResult {
    let stack: StackResult
    let renderID: String
    /// The render with its paint, if any.
    let display: CGImage?
    /// As stacked, for when its paint is reset.
    let plainDisplay: CGImage?
    /// The whole depth map, each frame in its `legend` color; `nil` when
    /// the render has none.
    let map: CGImage?
    /// In pixels: what `display` and `map` are laid out at.
    let fullSize: CGSize
    /// Held only while it can be on screen, let go of in the background
    /// (`AppModel.fitPictures`).
    var displayPicture: DisplayPicture
    /// At half size, framed at full size. Held only while the depth map or
    /// Retouch shows (`AppModel.fitPictures`).
    var mapPicture: Picture?
    /// Each stacked frame's legend color, by path, for its rail tick while
    /// the map shows. Asked once (`StackResult.frameColors`).
    let legend: [String: Color]
    /// By path, each with why.
    let leftOut: [String: String]
    /// By path: those whose contribution can be shown.
    let stacked: Set<String>
    /// Each frame's share, in stack order (0 for those left out); `nil`
    /// without a frame map. Asked once (`StackResult.frameContributions`).
    let contributions: [FrameContribution]?
    /// Its frames' scopes while shown: read to compare, paint and export.
    let lease: Lease

    /// `size`: a picture turned since (Adjust), else the same.
    func showing(_ display: CGImage?, picture: DisplayPicture, renderID: String, size: CGSize? = nil) -> ShownResult {
        ShownResult(
            stack: stack, renderID: renderID, display: display, plainDisplay: plainDisplay,
            map: map, fullSize: size ?? fullSize, displayPicture: picture, mapPicture: mapPicture,
            legend: legend, leftOut: leftOut, stacked: stacked, contributions: contributions,
            lease: lease)
    }

    var displayPane: PanePicture? {
        display.map { PanePicture($0, source: displayPicture.picture, size: fullSize) }
    }

    var mapPane: PanePicture? {
        map.map { PanePicture($0, source: mapPicture, size: fullSize) }
    }

    /// `colors` names every frame of the stack (`nil` for one left out),
    /// `frames` the stacked ones, in the same order.
    nonisolated static func legend(_ colors: [LegendColor?], frames: [String]) -> [String: Color] {
        let stacked = colors.compactMap { $0 }
        guard stacked.count == frames.count else { return [:] }
        return Dictionary(
            zip(frames, stacked.map(\.color)),
            uniquingKeysWith: { first, _ in first })
    }
}

/// What a viewer pane shows: a flat `base` first and, zoomed in past its
/// resolution, `source`'s tiles (`TiledPictureView`), laid out at `size`
/// in image pixels. Each thing shown carries its `Picture` beside its flat
/// one in the state that holds it, so the two go together. Without one (a
/// preview, the live map, a stand-in) the base shows alone at its size.
struct PanePicture: Equatable {
    let base: CGImage
    let source: (any TileSource)?
    let size: CGSize
    /// The part shown, in its pixels: a render's crop (ADR-171); `nil`
    /// shows the whole.
    var window: CGRect?

    /// Showing only `window` of it.
    func windowed(_ window: CGRect?) -> PanePicture {
        var windowed = self
        windowed.window = window
        return windowed
    }

    init(_ base: CGImage) {
        self.init(base, source: nil, size: CGSize(width: base.width, height: base.height))
    }

    init(_ base: CGImage, picture: Picture?) {
        self.init(base, source: picture, size: Self.size(of: picture) ?? CGSize(width: base.width, height: base.height))
    }

    /// In pixels.
    static func size(of picture: Picture?) -> CGSize? {
        picture.map {
            let full = $0.size()
            return CGSize(width: Int(full.width), height: Int(full.height))
        }
    }

    init(_ base: CGImage, source: (any TileSource)?, size: CGSize) {
        self.base = base
        self.source = source
        self.size = size
    }

    /// By identity.
    static func == (a: PanePicture, b: PanePicture) -> Bool {
        a.base === b.base && a.source === b.source && a.size == b.size && a.window == b.window
    }
}



/// What the viewer shows is the engine's `ViewState` (ADR-149), as on the
/// desktop: changed only by `AppModel.view(_:)`. Its transitions are the
/// engine's (`deptheos_app::view::Mode`), which logs them. It holds no
/// load (`AppModel.alignedLoad`, `AppModel.litMap`). While a stack runs,
/// all but a frame alone shows the live map; in Retouch the canvas is over
/// the result alone.
extension ViewState {
    var isResultAlone: Bool {
        if case .result = self { true } else { false }
    }

    var compare: ViewCompare? {
        if case .compare(let compare) = self { compare } else { nil }
    }

    /// The frame alone's overlay.
    var frame: Overlay? {
        if case .frame(let overlay) = self { overlay } else { nil }
    }

    /// While the map is shown.
    var light: ViewLight? {
        if case .map(_, let light) = self { light } else { nil }
    }

    /// What it lines up with the result, so its load is kept: a compare's
    /// target or a frame's Contribution, also under the depth map.
    var linesUp: LinesUp? {
        switch self {
        case .compare, .map(back: .compare, light: _): .compare
        case .frame(overlay: .contribution), .map(back: .frame(overlay: .contribution), light: _): .contribution
        case .frame(overlay: .none), .result, .map: nil
        }
    }

    enum LinesUp {
        case compare
        case contribution
    }
}

extension ViewLight {
    var file: String? {
        switch self {
        case .whole: nil
        case .lit(let file, _), .marked(let file): file
        }
    }

    /// While a frame is lit.
    var token: UInt64? {
        if case .lit(_, let token) = self { token } else { nil }
    }
}

/// How the depth map is lit, with the lit picture as it loads: the
/// viewer's `AppModel.litMap` and Retouch's `PaintView.depthMap`, changed
/// only by their owners through `MapLighter`.
///
///     whole ──a frame lit──▶ lit(f, its picture loading)
///     lit(f) ──another frame lit──▶ lit(g, f's picture held while g's loads)
///     lit(f) ──put out──▶ whole
enum MapLighting {
    case whole
    case lit(MapLight)

    var light: MapLight? {
        if case .lit(let light) = self { light } else { nil }
    }

    /// For the log: no file names.
    var logName: String {
        switch self {
        case .whole: "whole"
        case .lit(let light): "lit: \(light.logName)"
        }
    }
}

/// A frame lit on the depth map: what it supplied in its legend color, the
/// rest dimmed (`PictureOf.litMap`). Its picture is a `Latest` loaded by
/// its `MapLighter`. A frame left out lights nothing: no picture, the map
/// whole, its tick marked.
///
///     ──a frame lit──▶ loading(r, showing: the lit picture before, if any)
///     loading(r, showing: a) ──an overtaken load lands, newer than a──▶ loading(r, showing: it)
///     loading(r, _) ──r lands──▶ loaded
///     loaded without its picture ──back in front──▶ loading(s, showing: it) ──s lands──▶ loaded
///     ──a frame left out lit──▶ none
struct MapLight {
    let file: String
    let result: ObjectIdentifier
    var picture: Latest<LitRequest, LitMap>

    /// For the log: no file names.
    var logName: String {
        if case .none = picture { return "a frame left out" }
        return "a frame, \(picture.logName)"
    }
}

/// A load of a lit map, by its number.
struct LitRequest: LoadRequest {
    let ticket: Ticket
}

/// The depth map lit for `file` against `result`, by load `request`, so a
/// held one is replaced only by a newer one.
struct LitMap: LoadMade {
    let file: String
    private(set) var request: Ticket
    let result: ObjectIdentifier
    let image: CGImage
    /// The half-size map's, framed at full size; let go of in the
    /// background.
    var picture: Picture?
    let size: CGSize

    init(file: String, request: Ticket, result: ObjectIdentifier, image: CGImage, picture: Picture?) {
        (self.file, self.request, self.result, self.image, self.picture) = (file, request, result, image, picture)
        size = PanePicture.size(of: picture) ?? CGSize(width: image.width, height: image.height)
    }

    var pane: PanePicture { PanePicture(image, source: picture, size: size) }

    /// With its picture made again (`nil`: not kept) by `request`.
    func made(again picture: Picture?, by request: Ticket) -> LitMap {
        var made = self
        made.picture = picture
        made.request = request
        return made
    }
}

/// Lights the depth map for one frame at a time, one each for the viewer
/// and Retouch. The lighting lives in the state that shows the map
/// (`current` reads it, `set` changes it). Loads go through a `LatestLoad`:
/// an overtaken one is held while the newer loads if it lights the same
/// result.
final class MapLighter {
    private let loads: LatestLoad

    init(name: String) {
        loads = LatestLoad(name: name)
    }

    /// Back in front: a stopped load, if still awaited, loads.
    func askAgain() {
        loads.askAgain()
    }

    /// Light `file` (stacked by `stack`, or left out and lighting nothing)
    /// on the map `current` reads, `nil` once the map is gone. `set`
    /// changes it at once and as the picture lands; `failed` when it
    /// couldn't be made.
    func light(
        _ file: String, stacked: Bool, stack: StackResult,
        current: @escaping @MainActor () -> MapLighting?,
        set: @escaping @MainActor (MapLighting) -> Void,
        failed: @escaping @MainActor (String) -> Void
    ) {
        guard let now = current() else { return }
        let id = ObjectIdentifier(stack)
        if let lit = now.light, lit.file == file, lit.result == id { return }
        guard stacked else {
            set(.lit(MapLight(file: file, result: id, picture: .none)))
            return
        }
        let request = LitRequest(ticket: loads.ticket())
        let before = now.light?.picture ?? .none
        set(.lit(MapLight(file: file, result: id, picture: before.asking(request) { $0.result == id })))
        let side = AppModel.displaySide
        let name = loads.name
        load(request, of: id, current: current, set: set, landed: { if $0 == nil { failed(file) } }) {
            let started = ContinuousClock.now
            // The picture and its base, made together: one pyramid.
            let lit = try await Request().load {
                try await stack.picture(request: $0, of: .litMap(frame: file), baseSide: side)
            }
            let (image, picture) = (lit?.base?.makeCGImage(), lit?.picture)
            viewLog.debug("""
                \(name, privacy: .public) \(request.ticket.number) took \(AppModel.milliseconds(since: started)) ms, \
                \(image?.width ?? 0)×\(image?.height ?? 0)
                """)
            guard let image else {
                viewLog.error("\(name, privacy: .public) \(request.ticket.number) couldn't be made")
                return nil
            }
            return LitMap(file: file, request: request.ticket, result: id, image: image, picture: picture)
        }
    }

    /// Make again the picture of the lit map shown, let go of in the
    /// background, landing it if `wanted` still says so.
    func remake(
        stack: StackResult, wanted: @escaping @MainActor () -> Bool,
        current: @escaping @MainActor () -> MapLighting?,
        set: @escaping @MainActor (MapLighting) -> Void
    ) {
        let id = ObjectIdentifier(stack)
        guard var light = current()?.light, light.result == id, case .loaded(let map) = light.picture,
              map.picture == nil
        else { return }
        let request = LitRequest(ticket: loads.ticket())
        light.picture = .loading(request, showing: map)
        set(.lit(light))
        let file = map.file
        let name = loads.name
        load(request, of: id, current: current, set: set) {
            let started = ContinuousClock.now
            let picture = try await Request().load {
                try await stack.picture(request: $0, of: .litMap(frame: file), baseSide: 0)?.picture
            }
            if picture == nil { viewLog.error("\(name, privacy: .public) \(request.ticket.number): its picture couldn't be made again") }
            let kept = UIApplication.shared.applicationState != .background && wanted() ? picture : nil
            viewLog.debug("\(name, privacy: .public) \(request.ticket.number): its picture made again in \(AppModel.milliseconds(since: started)) ms\(kept == nil ? ", not kept" : "", privacy: .public)")
            return map.made(again: kept, by: request.ticket)
        }
    }

    /// While it lights result `id`; an overtaken picture of it is held
    /// while the newer one loads.
    private func load(
        _ request: LitRequest, of id: ObjectIdentifier,
        current: @escaping @MainActor () -> MapLighting?,
        set: @escaping @MainActor (MapLighting) -> Void,
        landed: @escaping @MainActor (LitMap?) -> Void = { _ in },
        make: @escaping @MainActor () async throws -> LitMap?
    ) {
        loads.load(
            request,
            latest: {
                guard let light = current()?.light, light.result == id else { return nil }
                return light.picture
            },
            set: { picture in
                guard var light = current()?.light, light.result == id else { return }
                light.picture = picture
                set(.lit(light))
            },
            holds: { _, _, _ in true },
            make: make, landed: landed)
    }
}

/// A frame's own preview (`AppModel.sourcePreview`): its file, its image
/// (`nil` when unreadable), and the load that made it, so an earlier load
/// never replaces it. During a drag it may not be the frame selected; the
/// labels name the frame whose pixels these are.
struct FramePreview: LoadMade {
    let file: String
    let image: CGImage?
    /// In the order asked for (`previewLoads`).
    let request: Ticket
    /// Changed only by `AppModel.setFullFrame`.
    var full = FrameFull.none

    /// At full size with tiles once in, else the file's preview alone.
    var pane: PanePicture? {
        full.shown?.pane ?? image.map(PanePicture.init)
    }
}

/// The frame selected as asked for, in its stack.
struct PreviewRequest: LoadRequest {
    let ticket: Ticket
    let file: String
    let stack: String
}

/// The frame's preview as it loads (`AppModel.previewLoad`): the one shown
/// before held meanwhile, or during a drag the last to land on the way.
typealias PreviewLoad = Latest<PreviewRequest, FramePreview>

/// A preview's full size. The file's quick preview shows during a drag;
/// once the selection settles, the engine's own decode (as stacked,
/// `Engine.framePicture`) loads, so colors and geometry match the stack at
/// every zoom. Only the frame selected has one.
///
/// Lifecycle: a `Latest`: none, loading, loaded (with or without its
/// picture) or failed. Held as `FramePreview.full`, changed only by
/// `AppModel.setFullFrame` (ADR-164).
///
///     none ──settled──▶ loading(r) ──r lands──▶ loaded, or failed (the file's preview stays)
///     loaded ──the app went to the background, or a stack started──▶ loaded without its picture (its base kept)
///     loaded without its picture ──back in front, or the stack over──▶ loading(s, showing: it) ──s lands──▶ loaded (`AppModel.fitPictures`)
///     loading ──a stack started──▶ what it holds, else none (asked for again when it ends; `AppModel.frameWantsPicture`)
///     any ──the view leaves the frame──▶ none
typealias FrameFull = Latest<FullRequest, FullFrame>

/// A frame's full-size load: `call` is canceled once it is left, so an
/// unwanted decode doesn't start, or stops.
struct FullRequest: LoadRequest {
    let ticket: Ticket
    let call: Request
}

/// A frame at full size: its base laid out at `size`, and the picture,
/// which goes in the background.
struct FullFrame: LoadMade {
    private(set) var request: Ticket
    let base: CGImage
    var picture: Picture?
    let size: CGSize

    var loadName: String { picture == nil ? "loaded without its picture" : "loaded" }

    var pane: PanePicture { PanePicture(base, source: picture, size: size) }

    /// With its picture made again (`nil`: not kept) by `request`.
    func made(again picture: Picture?, by request: Ticket) -> FullFrame {
        var made = self
        made.picture = picture
        made.request = request
        return made
    }
}

extension Latest where Asked == FullRequest {
    var call: Request? { asked?.call }
}

/// What the result is compared with: the selected frame with its overlay,
/// or another render. Made from the view, never stored.
enum CompareTarget: Equatable {
    case frame(path: String, overlay: Overlay)
    case render(id: String)

    var framePath: String? {
        if case .frame(let path, _) = self { path } else { nil }
    }

    var renderID: String? {
        if case .render(let id) = self { id } else { nil }
    }
}

/// A frame, with Contribution or not, or a render.
enum CompareSource: Equatable {
    case frame(String, contribution: Bool)
    case render(String)

    var framePath: String? {
        if case .frame(let path, _) = self { path } else { nil }
    }

    /// Same kind: one may stand in while the other loads, so Contribution
    /// toggled mid-drag never shows the frame the other way.
    func isKind(of other: CompareSource) -> Bool {
        switch (self, other) {
        case (.frame(_, let contribution), .frame(_, let other)): contribution == other
        case (.render, .render): true
        case (.frame, .render), (.render, .frame): false
        }
    }

    var labelTarget: CompareTarget {
        switch self {
        case .frame(let path, _): .frame(path: path, overlay: .none)
        case .render(let id): .render(id: id)
        }
    }
}

/// Something lined up with the result as it loads (`AppModel.alignedLoad`).
/// While loading it holds the one before, or a newer overtaken load of the
/// same kind (`CompareSource.isKind`) for the same result, so a drag shows
/// each frame as it lands. A picture let go of in the background is made
/// again by a load, the image held without it meanwhile.
typealias CompareLoad = Latest<CompareRequest, CompareImage>

/// One load, numbered: it applies only while the state holds it.
struct CompareRequest: LoadRequest {
    let ticket: Ticket
    let source: CompareSource
    let result: ObjectIdentifier

    func asks(_ source: CompareSource, result: ObjectIdentifier) -> Bool {
        self.source == source && self.result == result
    }
}

/// How a compare lays out the result with what it is compared with: split,
/// side by side, above and below, or flicked in place.
extension CompareLayout {
    /// As the view menu and keys name them.
    enum Kind: CaseIterable {
        case split
        case sideBySide
        case topBottom
        case flicker

        /// The look that chooses it.
        var look: Look {
            switch self {
            case .split: .split
            case .sideBySide: .sideBySide
            case .topBottom: .topAndBottom
            case .flicker: .flicker
            }
        }

        var label: LocalizedStringResource {
            switch self {
            case .split:
                LocalizedStringResource("Split", comment: "Compare layout: the result and what it is compared with, split by a divider. In the view menu under the photo and the View menu.")
            case .sideBySide:
                LocalizedStringResource("Side by Side", comment: "Compare layout: the result beside what it is compared with. In the view menu under the photo and the View menu.")
            case .topBottom:
                LocalizedStringResource("Top and Bottom", comment: "Compare layout: the result below what it is compared with. In the view menu under the photo and the View menu.")
            case .flicker:
                LocalizedStringResource("Flicker", comment: "Compare layout: Flip switches between the result and what it is compared with, in place. In the view menu under the photo and the View menu.")
            }
        }

        /// The desktop's glyphs (design/README.md, Icons).
        var systemImage: String {
            switch self {
            case .split: "square.split.2x1"
            case .sideBySide: "rectangle.split.2x1"
            case .topBottom: "rectangle.split.1x2"
            case .flicker: "circle.dotted.and.circle"
            }
        }

        /// As on the desktop.
        var key: KeyEquivalent {
            switch self {
            case .split: "2"
            case .sideBySide: "3"
            case .topBottom: "4"
            case .flicker: "\\"
            }
        }
    }

    var kind: Kind {
        switch self {
        case .split: .split
        case .sideBySide: .sideBySide
        case .topAndBottom: .topBottom
        case .flicker: .flicker
        }
    }

    /// Flicker shows what the result is compared with, not the result.
    var showsSource: Bool {
        if case .flicker(let source) = self { source } else { false }
    }

    /// As the View menu and the `1` key name it.
    static let resultLabel = LocalizedStringResource(
        "Result", comment: "View menu item (key 1): the result alone, not compared.")
}

/// Something lined up with the result.
struct CompareImage: LoadMade {
    /// A held image is replaced only by a newer one.
    private(set) var request: Ticket
    let source: CompareSource
    let result: ObjectIdentifier
    let image: CGImage
    /// In the result's geometry; none for a frame that couldn't be lined
    /// up. Let go of in the background or off screen.
    var picture: Picture?
    /// A frame that couldn't be (no alignment, or automation's
    /// `-FakeResult`) is shown as it is, fitted.
    let aligned: Bool
    let contribution: Bool
    let size: CGSize

    init(
        request: Ticket, source: CompareSource, result: ObjectIdentifier, image: CGImage, picture: Picture?,
        aligned: Bool, contribution: Bool
    ) {
        (self.request, self.source, self.result, self.image, self.picture) = (request, source, result, image, picture)
        (self.aligned, self.contribution) = (aligned, contribution)
        size = PanePicture.size(of: picture) ?? CGSize(width: image.width, height: image.height)
    }

    var pane: PanePicture { PanePicture(image, source: picture, size: size) }

    /// With its picture, or a frame that couldn't be lined up.
    var complete: Bool { picture != nil || !aligned }

    /// With its picture made again (`nil`: not kept) by `request`.
    func made(again picture: Picture?, by request: Ticket) -> CompareImage {
        var made = self
        made.picture = picture
        made.request = request
        return made
    }
}
