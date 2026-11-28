import Foundation
@preconcurrency import DeptheosEngine

private let appLog = AppLog(category: "app")

/// Retouching: starting and stopping it, what the brush can paint from, and
/// the titles.
extension AppModel {
    /// Newest first.
    var paintRenders: [RenderInfo] {
        stackRenders.filter { $0.id != paint.renderID }
    }

    /// Picked on screen at once, painted from once the session has read it.
    func paintFrom(render id: String) {
        paint.pickRender(id)
    }

    /// The side of Retouch's sources, as Paint From picks: paint mode's
    /// column and the compact sheet both, so they never disagree.
    func showPaintSources(_ tab: SourcesTab) {
        guard paint.active else { return }
        paint.sourcesTab = tab
        if case .sources(let shown, let height) = panel, shown != tab { setPanel(.sources(tab, height)) }
    }

    /// ← and →: from a render, the render `step` places on (newest first);
    /// from a frame, the frame; from the original, the first or last frame.
    func stepPaintSource(by step: Int) async {
        guard paint.active else { return }
        if let id = paint.sourceRender {
            let renders = paintRenders
            guard let at = renders.firstIndex(where: { $0.id == id }) else { return }
            let next = min(max(at + step, 0), renders.count - 1)
            if next != at { paintFrom(render: renders[next].id) }
            return
        }
        paint.stepFrame(by: step)
    }

    /// The rail places of frames some painted pixel came from.
    func paintedMarks(in frames: [RailFrame]) -> IndexSet {
        let painted = paint.paintedFrom
        return IndexSet(frames.indices.filter { painted.contains(frames[$0].path) })
    }

    /// Goes after a few seconds, unless another took its place.
    func showBriefNotice(_ text: UserText) {
        notice = text
        Task { [weak self] in
            try? await Task.sleep(for: .seconds(4))
            if self?.notice == text { self?.notice = nil }
        }
    }

    /// "canon1 · 12 strokes".
    var retouchLine: UserText {
        guard let name = stackName else { return .app(strokesText) }
        return .line([.verbatim(name), .app(strokesText)])
    }

    /// "12 strokes".
    var strokesText: LocalizedStringResource {
        LocalizedStringResource(
            "\(paint.undoCount) strokes",
            comment: "In paint mode, under the title: how many strokes can be undone. The placeholder is how many; zero reads 'No strokes'.")
    }

    /// "Retouching #4".
    var retouchTitle: LocalizedStringResource {
        let number = history.renders.first { $0.id == paint.renderID }?.number
        return number.map {
            LocalizedStringResource(
                "Retouching #\($0)",
                comment: "The title in paint mode. The placeholder is the number of the render being retouched.")
        } ?? LocalizedStringResource("Retouching", comment: "The title in paint mode, when the render's number isn't known.")
    }

    func startPaint(_ engine: Engine) {
        paint.app = self
        paint.onPresented = { [weak self] id, painted in
            guard let self, let shown = result, shown.renderID == id, !paint.active else { return }
            // Nothing painted (reset, or unshowable, which was said): the
            // result as stacked, remade by `fitPictures`, unless shown.
            if painted == nil, shown.display === shown.plainDisplay { return }
            let image = painted?.image ?? shown.plainDisplay
            // The same render, painted or not, so it keeps its zoom.
            if let replaced = shown.display, let image, let zoom = viewerZoom,
               zoom.image == ObjectIdentifier(replaced) {
                viewerZoom = ViewerZoom(image: ObjectIdentifier(image), visible: zoom.visible)
            }
            // A stack started since: the paint shows, its picture asked for
            // again if the stack ends without a result (`fitPictures`).
            let wanted = wantsPictures(of: shown)
            if !wanted, painted != nil { appLog.debug("paint's picture landed while a stack runs: dropped") }
            setResult(shown.showing(image, picture: DisplayPicture(wanted ? painted?.picture : nil), renderID: id))
        }
        paint.onFailed = { [weak self] message in self?.errorMessage = ErrorMessage(message) }
        paint.onNotice = { [weak self] text in self?.showBriefNotice(text) }
        paint.start(engine)
    }

    /// A result is shown (every one is a render) and nothing is stacking.
    var canPaint: Bool {
        result?.display != nil && !busy
    }

    /// Retouch the result, from the frame selected if it is one of its
    /// frames, on its picture until the tiles come. The view underneath
    /// becomes the result alone (`showResultAlone`), so queued compare and
    /// Contribution loads drop and no preview is asked for
    /// (`needsSourcePreview`). Leaving shows the result alone; Retouch's own
    /// layout is `PaintModel`'s memory.
    func startPainting() {
        guard canPaint, let result else { return }
        stopAdjusting()
        setPanel(.closed)
        paint.open(
            result.stack, renderID: result.renderID, frame: selectedFile, standIn: result.display,
            lease: result.lease)
        guard paint.active else { return }
        // Retouch opens where the viewer was zoomed on this render.
        if let display = result.display, let zoom = viewerZoom, zoom.image == ObjectIdentifier(display) {
            paint.zoom = zoom.visible
        }
        showResultAlone()
    }

    /// Back to the result, with its paint.
    func stopPainting() {
        guard paint.active else { return }
        let zoom = paint.zoom
        // The paint as the canvas shows it, at once, so the result never
        // shows without it; its whole picture follows.
        let painted = paint.paintedImage()
        // Asked before the worker closes the render: open, it flattens the
        // canvas it holds; closed, it would rebuild it from the stack.
        // `onPresented` lands after `close`, so it is applied.
        if let result { paint.present(result.stack, renderID: result.renderID) }
        paint.close()
        guard let result else { return }
        let id = result.renderID
        // Nothing painted or nothing to show it from: the result as stacked,
        // then its paint if any, as the worker's last state may lag. Its
        // picture is remade (`fitPictures`) unless its paint comes first.
        let display = painted ?? result.plainDisplay
        viewerZoom = zoom.flatMap { visible in
            display.map { ViewerZoom(image: ObjectIdentifier($0), visible: visible) }
        }
        // The paint's picture is on its way, so `fitPictures` doesn't ask.
        setResult(result.showing(display, picture: painted == nil ? .none : .painting, renderID: id))
        // The result alone shows the selected frame while held: its preview.
        requestSourcePreview()
    }
}
