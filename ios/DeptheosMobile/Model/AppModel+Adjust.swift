import CoreGraphics
import Foundation
@preconcurrency import DeptheosEngine

private let stateLog = AppLog(category: "state")

/// Adjust on the result shown (ADR-153, ADR-171): the engine's session
/// draws, `AdjustModel` shows what it says, and each settled picture
/// becomes the result's display.
extension AppModel {
    func startAdjust(_ engine: Engine) {
        adjust.app = self
        adjust.onSettled = { [weak self] id, image, picture in
            guard let self, let shown = result, shown.renderID == id else { return }
            // The same render, drawn again, keeps its zoom.
            if let replaced = shown.display, let zoom = viewerZoom, zoom.image == ObjectIdentifier(replaced) {
                viewerZoom = ViewerZoom(image: ObjectIdentifier(image), visible: zoom.visible)
            }
            let size = PanePicture.size(of: picture) ?? shown.fullSize
            setResult(shown.showing(image, picture: .made(picture), renderID: id, size: size))
        }
        adjust.onClosed = { [weak self] id, redraw in
            guard let self, let shown = result, shown.renderID == id, viewing else { return }
            // Its pictures come under the reframe they were made with: one
            // changed, or a screen behind its saved value, is read again.
            let saved = shown.stack.reframe()
            let reframed = adjust.lastReframe.map { $0 != saved } ?? false
            guard redraw || reframed else { return }
            stateLog.debug("adjust: closed, the render read again (redraw \(redraw ? "yes" : "no", privacy: .public), reframed \(reframed ? "yes" : "no", privacy: .public))")
            Task { await self.showResult(of: id) }
        }
        adjust.onRewritten = { [weak self] id in self?.history.thumbnailRewritten(id) }
        adjust.onNotice = { [weak self] text in self?.showBriefNotice(text) }
        adjust.start(engine)
    }

    /// As for Retouch: a result is shown and nothing is stacking.
    var canAdjust: Bool { canPaint }

    /// Adjust the result: the view becomes Adjust's (the result alone).
    func startAdjusting() {
        guard canAdjust, let result else {
            stateLog.debug("adjust: not opened: \(self.result == nil ? "no result" : "busy", privacy: .public)")
            return
        }
        stopPainting()
        setPanel(.closed)
        adjust.open(result.stack, renderID: result.renderID)
        guard adjusting else { return }
        view(.adjust(on: true))
    }

    /// Back to the result, its adjustments written.
    func stopAdjusting() {
        guard adjusting else { return }
        adjust.close()
        view(.adjust(on: false))
    }

    /// Retouch or Adjust left: another render, another stack, a delete.
    func leaveMode() {
        stopPainting()
        stopAdjusting()
    }
}

extension AppModel {
    var adjustTitle: LocalizedStringResource {
        let number = history.renders.first { $0.id == adjust.renderID }?.number
        return number.map {
            LocalizedStringResource(
                "Adjusting #\($0)",
                comment: "The title in Adjust. The placeholder is the number of the render being adjusted.")
        } ?? LocalizedStringResource("Adjusting", comment: "The title in Adjust, when the render's number isn't known.")
    }

    /// Copy Adjustments: the render shown's, after its queued writes.
    func copyAdjustments() async {
        guard let engine, let id = adjust.renderID ?? result?.renderID else { return }
        do {
            copiedAdjustments = try await engine.renderAdjustments(renderId: id)
        } catch {
            report(error)
        }
    }

    /// Paste Adjustments: onto the render adjusted, or the one shown,
    /// its crop and process version kept. Never canceled.
    func pasteAdjustments() {
        guard let copied = copiedAdjustments else { return }
        if adjusting {
            adjust.paste(copied)
            return
        }
        guard let engine, let id = result?.renderID else { return }
        Task {
            do {
                try await engine.pasteAdjustments(renderId: id, copied: copied)
                history.thumbnailRewritten(id)
                await showResult(of: id)
            } catch {
                report(error)
            }
        }
    }
}

extension AppModel {
    /// The reframe the result's pictures show under: Adjust's as it moves,
    /// else the one they came with.
    var shownReframe: Reframe? {
        adjust.value?.reframe() ?? result?.stack.reframe()
    }

    /// The result's display as its pane draws it: the crop alone where the
    /// view shows the result reframed, uncropped elsewhere (ADR-171).
    var resultPicture: PanePicture? {
        result?.displayPane.map(windowed)
    }

    /// `picture`, a turned and flipped picture of the result shown,
    /// showing only its crop where the pane is the result.
    func windowed(_ picture: PanePicture) -> PanePicture {
        guard resultPane == .result, let window = cropMark(fitting: picture.size) else { return picture }
        return picture.windowed(window)
    }

    /// The crop over the uncropped picture as shown, in its pixels, if
    /// there is one and the picture is that size (one drawn under another
    /// turn shows whole until the next lands).
    func cropMark(fitting size: CGSize) -> CGRect? {
        guard let result, let reframe = shownReframe, reframe.crop != nil else { return nil }
        let stored = result.stack.storedSize
        let framing = PaneFraming(width: stored.width, height: stored.height, reframe: reframe, pane: .uncropped)
        let shown = framing.size()
        guard CGFloat(shown.x) == size.width, CGFloat(shown.y) == size.height, let mark = framing.cropMark()
        else { return nil }
        return CGRect(x: Int(mark.x), y: Int(mark.y), width: Int(mark.width), height: Int(mark.height))
    }
}
