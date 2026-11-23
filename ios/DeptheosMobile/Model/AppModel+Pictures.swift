import CoreGraphics
import UIKit
@preconcurrency import DeptheosEngine

nonisolated private let pictureLog = AppLog(category: "pictures")

extension Picture {
    /// "8256×5504, ~243 MB": full size and what its levels hold at 4 bytes
    /// a pixel, for the log.
    nonisolated var logSize: String {
        let size = size()
        let bytes = levels().reduce(0) { $0 + Int($1.width) * Int($1.height) * 4 }
        return "\(size.width)×\(size.height), ~\(bytes / 1_000_000) MB"
    }
}

/// Only the pictures on screen are held (ADR-164): the result's, the depth
/// map's while shown or under Retouch, what the result is compared with
/// (and the one it replaces until that lands), one lit map, one frame
/// alone. Every holder keeps its full size beside its picture, so a pane
/// lays out the same either way.
///
/// In the background every full-size picture goes and the flat ones stay;
/// back in front, those on screen are made again through their
/// `LatestLoad`. While a stack or batch runs the memory is the stack's:
/// the result it may replace, its compare and a frame alone hold none
/// (`wantsPictures`, `frameWantsPicture`). Retouch's pictures are the paint
/// worker's.
extension AppModel {
    /// Let go of the pictures not on screen (all in the `background`), and
    /// make again those on screen that have none.
    func fitPictures(background: Bool = UIApplication.shared.applicationState == .background) {
        fitResultPictures(background: background)
        fitViewPictures(background: background)
        fitFramePicture(background: background)
    }

    /// No stack is on its way to replace `shown` (unless it is that stack's
    /// result, shown as kept) and no batch runs, so it and its compare may
    /// hold full-size pictures.
    func wantsPictures(of shown: ShownResult) -> Bool {
        guard batchRun == nil else { return false }
        switch stacking?.stage {
        case nil: return true
        case .running: return false
        case .saving(let made): return made === shown.stack
        }
    }

    // MARK: The result and its map

    private func fitResultPictures(background: Bool) {
        guard var shown = result else { return }
        // Under Retouch the canvas shows the render, not the result's picture.
        let wanted = !background && wantsPictures(of: shown)
        let wantsDisplay = wanted && !paint.active
        let wantsMap = wanted && shown.map != nil && (showsDepthMap || paint.active)
        var changed = false
        if !wantsDisplay, let picture = shown.displayPicture.picture {
            pictureLog.debug("result's picture let go of: \(picture.logSize, privacy: .public)")
            shown.displayPicture = .none
            changed = true
        }
        if !wantsMap, let picture = shown.mapPicture {
            pictureLog.debug("depth map's picture let go of: \(picture.logSize, privacy: .public)")
            shown.mapPicture = nil
            changed = true
        }
        if changed {
            setResult(shown)
            return
        }
        let stack = shown.stack
        if wantsDisplay, case .none = shown.displayPicture, let display = shown.display {
            if display === shown.plainDisplay {
                makePicture(
                    resultPictures,
                    wanted: { [weak self] in
                        guard let self, let now = self.result else { return false }
                        guard case .none = now.displayPicture else { return false }
                        return now.display === display && !self.paint.active && self.wantsPictures(of: now)
                    },
                    make: { try await stack.picture(request: $0, of: .display, baseSide: 0)?.picture }
                ) { [weak self] picture in
                    guard let self, var now = self.result else { return }
                    now.displayPicture = DisplayPicture(picture)
                    self.setResult(now)
                }
            } else {
                // Remade by the paint worker: `onPresented` replaces the
                // display with its picture.
                pictureLog.debug("result's picture asked of the paint worker")
                paint.present(stack, renderID: shown.renderID)
                shown.displayPicture = .painting
                setResult(shown)
                return
            }
        }
        if wantsMap, shown.mapPicture == nil, let map = shown.map {
            makePicture(
                mapPictures,
                wanted: { [weak self] in
                    guard let self, let now = self.result else { return false }
                    return now.map === map && now.mapPicture == nil && (self.showsDepthMap || self.paint.active)
                        && self.wantsPictures(of: now)
                },
                make: { try await stack.picture(request: $0, of: .map, baseSide: 0)?.picture }
            ) { [weak self] picture in
                guard let self, var now = self.result else { return }
                now.mapPicture = picture
                self.setResult(now)
            }
        }
    }

    // MARK: What the view lines up and lights

    /// The compares and lit map in the view: those shown keep or remake
    /// their pictures; one kept under the depth map, everything in the
    /// background, and while a stack runs the compare and lit map, let go.
    /// A frame alone with Contribution stays.
    private func fitViewPictures(background: Bool) {
        let wanted = !background && (result.map(wantsPictures(of:)) ?? true)
        var dropped: [String] = []
        let drop = { (image: CompareImage) -> CompareImage in
            guard let picture = image.picture else { return image }
            dropped.append("compared: \(picture.logSize)")
            return image.made(again: nil, by: image.request)
        }
        let dropLit = { (lit: LitMap) -> LitMap in
            guard let picture = lit.picture else { return lit }
            dropped.append("lit map: \(picture.logSize)")
            return lit.made(again: nil, by: lit.request)
        }
        // On screen: a compare's while pictures are wanted, a Contribution
        // frame's but in the background; one under the depth map isn't.
        let onScreen = switch viewState {
        case .compare: wanted
        case .frame(overlay: .contribution): !background
        case .frame(overlay: .none), .result, .map: false
        }
        if !onScreen { alignedLoad = alignedLoad.map(drop) }
        if !wanted, case .lit(var lit) = litMap {
            lit.picture = lit.picture.map(dropLit)
            litMap = .lit(lit)
        }
        for line in dropped { pictureLog.debug("picture let go of, \(line, privacy: .public)") }
        guard !background, let result else { return }
        // Back in front or a stack over: what the view shows is remade by
        // its own load. A stand-in held while another loads is not.
        let stack = result.stack
        switch viewState {
        case .compare:
            guard wanted else { return }
            remakeCompare(stack: stack)
        case .frame(overlay: .contribution):
            remakeCompare(stack: stack)
        case .map(back: _, light: .lit):
            guard wanted else { return }
            mapLighter.remake(
                stack: stack,
                wanted: { [weak self] in
                    guard let self else { return false }
                    return self.showsDepthMap && (self.result.map(self.wantsPictures(of:)) ?? false)
                },
                current: { [weak self] in self?.showsDepthMap == true ? self?.litMap : nil },
                set: { [weak self] in self?.litMap = $0 })
        case .frame(overlay: .none), .result, .map:
            break
        }
    }

    /// Remake the loaded compare image's picture against `stack`, the result
    /// it was lined up with (a newer result loads its own: `refreshCompare`).
    private func remakeCompare(stack: StackResult) {
        guard case .loaded(let image) = alignedLoad, image.picture == nil, image.aligned,
              image.result == ObjectIdentifier(stack)
        else { return }
        let engine = engine
        let source = image.source
        let request = CompareRequest(ticket: alignedLoads.ticket(), source: source, result: image.result)
        setLoad(.loading(request, showing: image))
        pictureLog.debug("compared picture being made again")
        loadAligned(request) { [weak self] in
            guard let self else { return nil }
            let started = ContinuousClock.now
            let picture = try await Request().load { request -> Picture? in
                switch source {
                case .frame(let path, let contribution):
                    let of = PictureOf.comparedFrame(path: path, contribution: contribution)
                    return try await stack.picture(request: request, of: of, baseSide: 0)?.picture
                case .render(let id):
                    guard let engine else { return nil }
                    let other = try await engine.loadRenderPixels(request: request, id: id)
                    return try await stack.picture(request: request, of: .comparedRender(other: other), baseSide: 0)?.picture
                }
            }
            let wanted = switch self.viewState {
            case .compare: self.result.map(self.wantsPictures(of:)) ?? false
            case .frame(overlay: .contribution): true
            case .frame(overlay: .none), .result, .map: false
            }
            return image.made(again: Self.kept(picture, "compared picture", wanted: wanted, since: started), by: request.ticket)
        }
    }

    /// `picture` if still `wanted` and in front (nothing lands in the
    /// background); logged.
    private static func kept(_ picture: Picture?, _ what: String, wanted: Bool, since started: ContinuousClock.Instant) -> Picture? {
        guard let picture else {
            pictureLog.error("\(what, privacy: .public) couldn't be made again")
            return nil
        }
        guard wanted, UIApplication.shared.applicationState != .background else {
            pictureLog.debug("\(what, privacy: .public) made again but no longer wanted: dropped")
            return nil
        }
        pictureLog.debug("\(what, privacy: .public) taken in \(AppModel.milliseconds(since: started)) ms: \(picture.logSize, privacy: .public)")
        return picture
    }

    // MARK: The frame alone

    /// No stack or batch runs, so a frame alone may hold its picture and
    /// load its full size. Otherwise it shows its base and is remade when
    /// the stack ends.
    var frameWantsPicture: Bool { stacking == nil && batchRun == nil }

    private func fitFramePicture(background: Bool) {
        // A full size still loading when a stack starts would land beside it.
        if !frameWantsPicture, let full = sourcePreview?.full, full.asked != nil {
            frameSettling?.cancel()
            setFullFrame(full.abandoned)
        }
        guard let preview = sourcePreview, case .loaded(let frame) = preview.full else { return }
        if background || !frameWantsPicture {
            if let picture = frame.picture {
                pictureLog.debug("frame's picture let go of: \(picture.logSize, privacy: .public)")
                setFullFrame(.loaded(frame.made(again: nil, by: frame.request)))
            }
            return
        }
        guard frame.picture == nil, needsSourcePreview, let engine else { return }
        let file = preview.file
        let decoder = stackSettings.rawDecoder
        let request = FullRequest(ticket: fullFrameLoads.ticket(), call: Request())
        setFullFrame(.loading(request, showing: frame))
        pictureLog.debug("frame's picture being made again")
        loadFullFrame(
            request, of: file,
            make: { [weak self] in
                let started = ContinuousClock.now
                let picture = try await request.call.load {
                    try await engine.framePicture(request: $0, path: file, rawDecoder: decoder, baseSide: 0).picture
                }
                let wanted = self?.frameWantsPicture ?? false
                return frame.made(again: Self.kept(picture, "frame's picture", wanted: wanted, since: started), by: request.ticket)
            },
            // Stopped as the app left: asked again under a new request.
            again: { [weak self] in
                self?.setFullFrame(.loaded(frame))
                self?.fitFramePicture(background: false)
            })
    }

    // MARK: Making a picture again

    /// Remake the result's or map's picture with `make`, one at a time
    /// through `loads`: skipped unless `wanted`, applied as it lands while
    /// still wanted. Nothing lands in the background.
    private func makePicture(
        _ loads: LatestLoad, wanted: @escaping @MainActor () -> Bool,
        make: @escaping (Request) async throws -> Picture?,
        apply: @escaping @MainActor (Picture) -> Void
    ) {
        let what = loads.name
        loads.run {
            guard wanted() else {
                pictureLog.debug("\(what, privacy: .public) skipped: no longer wanted, or made already")
                return
            }
            pictureLog.debug("\(what, privacy: .public) being made again")
            let started = ContinuousClock.now
            let picture = (try? await Request().run(make)) ?? nil
            guard let picture = Self.kept(picture, what, wanted: wanted(), since: started) else { return }
            apply(picture)
        }
    }
}
