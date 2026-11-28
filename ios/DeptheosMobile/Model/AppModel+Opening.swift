import Foundation
import SwiftUI
import UIKit
import os
@preconcurrency import DeptheosEngine

/// Each lifecycle transition, one line.
private let stateLog = AppLog(category: "state")

/// What is being opened, for view request `generation`, from the tap until
/// its picture lands. The engine switches stacks meanwhile, so the app is
/// `busy`, and `held` but for a render of the stack on screen (`result`).
///
/// Lifecycle: none, or opening a `Target`. Changed only by `beginOpening`,
/// `expectLanding` and `endOpening`, through `setOpening`. Every stack
/// switch goes through `switchStack`.
///
///     (none) ──beginOpening──▶ opening ──endOpening──▶ (none)
///                                 ├──beginOpening──▶ opening (another)
///                                 └──expectLanding──▶ opening, its landing known
///
/// A request begun while one opens replaces it. A stack's open learns its
/// base once its stack is on screen (`expectLanding`), and from then that
/// render is named and its Library thumbnail stands in for its picture
/// (`Landing`). An open that ends with a render shown loads it under its
/// own request (`showResult(of:continuing:)`), so it is one request from
/// tap to picture, and what it holds is released once, as the picture
/// lands (`show`).
struct Opening {
    enum Target {
        /// A render of another stack: its stack goes on screen, then it.
        case render(String)
        case failure(String)
        /// Revert to a render: its frames and settings, then it shown.
        case revert(String)
        /// A stack's working version, by id; with its landing once known.
        case stack(String, landing: Landing?)
        case newStack(String)
        /// A burst just taken, as a stack of its own.
        case capture(String)
        /// Try a Sample Stack: the sample's copy, as a stack of its own.
        case sample(String)
        /// A render of the stack on screen, its picture alone: the list and
        /// settings stay editable. Export, Retouch, Revert and Stack wait,
        /// so none acts on the picture it replaces.
        case result(String)

        var render: String? {
            switch self {
            case .render(let id), .result(let id): id
            case .failure, .revert, .stack, .newStack, .capture, .sample: nil
            }
        }

        /// The render shown once this lands, if known: what the deck and
        /// title name meanwhile.
        var lands: String? {
            switch self {
            case .render(let id), .result(let id): id
            case .stack(_, let landing): landing?.render
            case .failure, .revert, .newStack, .capture, .sample: nil
            }
        }

        /// The switch makes the stack, so it is the switch's fallback.
        var makesStack: Bool {
            switch self {
            case .newStack, .capture, .sample: true
            case .render, .failure, .revert, .stack, .result: false
            }
        }
    }
    let target: Target
    let generation: Int

    /// A stack open's base and its stand-in until its own picture lands:
    /// its Library thumbnail (512 pixels, cropped as the render), where in
    /// memory. Nothing is loaded for it.
    struct Landing: CustomStringConvertible {
        let render: String
        let standIn: CGImage?

        var description: String {
            standIn == nil ? "landing \(render)" : "landing \(render) on its thumbnail"
        }
    }

    var standIn: CGImage? {
        if case .stack(_, let landing?) = target { landing.standIn } else { nil }
    }
}

/// Opening what the Library and Renders list offer, and the stack switch
/// they share with New Stack and a burst.
extension AppModel {
    /// Load a failed batch stack's images and settings, with no result, to
    /// fix and stack again. The stack left keeps its working version.
    func retry(_ failure: FailureInfo) async {
        guard !busy else { return }
        leaveMode()
        await switchStack(
            to: failure.stack, opening: .failure(failure.id),
            fallback: failureWorking(failure: failure)
        ) { _ in
            history.shownFailure = failure.id
            setScreen(.stacking)
            setPanel(.sources(.frames, .low))
            await loadSourcePreview()
        }
    }

    /// Open a stack from the Library at its working version (its newest
    /// render's when none is kept), its base shown; the stack left keeps
    /// its own.
    ///
    /// The switch happens before Stacking slides in, so it never shows the
    /// stack it replaces, and the open is one request until the base's
    /// picture lands, so the controls go from waiting to ready once.
    /// Stacking slides in at once on the base's stand-in (`Opening.Landing`);
    /// the sheet of a never-stacked stack opens after the slide (`slideIn`).
    func openStack(_ stack: String) async {
        guard !busy else { return }
        guard stack != stackID else {
            // On screen already: its base, whatever was shown. Its result
            // loads when first asked for, not at launch.
            guard let base = baseRender?.id, result?.renderID != base else {
                slideIn(then: landingPanel)
                return
            }
            guard result == nil else {
                // Another of its renders stays on screen until the base
                // lands.
                slideIn(then: landingPanel)
                await showResult(of: base)
                return
            }
            // Nothing shown yet (the first open after launch): opened as
            // from another stack, so the slide starts on the stand-in.
            let generation = beginOpening(.stack(stack, landing: landing(base)))
            defer { endOpening(generation) }
            await land(on: base, request: generation)
            return
        }
        leaveMode()
        await switchStack(to: stack, opening: .stack(stack, landing: nil), fallback: nil) { request in
            if let base = baseRender?.id {
                expectLanding(landing(base), generation: request)
                await land(on: base, request: request)
            } else {
                // Never stacked: its frames are what to look at.
                slideIn(then: landingPanel)
                await showOwnResult(continuing: request)
            }
        }
    }

    private func landing(_ id: String) -> Opening.Landing {
        Opening.Landing(render: id, standIn: history.thumbnails[id])
    }

    /// Stacking slides in for open `request` on `base`'s stand-in where it
    /// has one, else on its first frame. A base that fails to load leaves
    /// its frame shown.
    private func land(on base: String, request: Int) async {
        if opening?.standIn != nil { showStandIn() }
        slideIn(then: landingPanel)
        await showResult(of: base, continuing: request)
        if viewGeneration == request, result == nil { showPlainFrame() }
    }

    /// Stacking slides over the Library, and `panel` opens once the slide
    /// is over, never in the same transaction: only if the stack is still
    /// on screen and no sheet was opened nor controls hidden meanwhile.
    func slideIn(then panel: Panel) {
        setPanel(.closed)
        let stack = stackID
        withAnimation(.snappy(duration: 0.3), completionCriteria: .logicallyComplete) {
            setScreen(.stacking)
        } completion: { [weak self] in
            guard let self, panel != .closed, screen == .stacking, stackID == stack,
                  self.panel == .closed
            else { return }
            setPanel(panel)
        }
    }

    /// Put `stack` on screen as opening `target`: the engine keeps the
    /// working version of the stack left and gives back `stack`'s (kept,
    /// else its newest render's, else `fallback`), editable, with no result
    /// and its first included frame selected. Then, while the request is
    /// current, `landed` is given it for `showResult` to land under. `false`
    /// when nothing went on screen: the switch failed (shown) or `stack` has
    /// nothing to open.
    ///
    /// A stack the switch makes (`target.makesStack`) goes on screen even
    /// when the one it leaves could not be kept.
    @discardableResult
    func switchStack(
        to stack: String, opening target: Opening.Target, fallback: WorkingInfo?,
        landed: (Int) async -> Void = { _ in }
    ) async -> Bool {
        let generation = beginOpening(target)
        defer { endOpening(generation) }
        let outgoing = working
        let switched = await ask { engine, _ in
            try await engine.switchStack(outgoing: outgoing, to: stack, fallback: fallback)
        }
        guard let incoming = (switched ?? nil) ?? (target.makesStack ? fallback : nil) else {
            return false
        }
        // The engine has switched, and nothing else could while opening.
        setResult(nil)
        edit(incoming)
        // Frames selected in the stack left aren't this one's.
        setFrameSelecting(.off)
        selectedFile = firstFrame
        // The stack left's frame isn't shown under this one's name.
        setPreviewLoad(.none)
        showPlainFrame()
        showControls()
        viewerZoom = nil
        guard viewGeneration == generation else { return true }
        await landed(generation)
        return true
    }

    /// The first included, else the first.
    var firstFrame: String? {
        rows.first(where: \.included)?.file ?? rows.first?.file
    }

    /// Show `render`'s picture; the list and settings stay. A render of
    /// another stack puts that stack on screen first. A render of this
    /// stack asked for while another loads replaces that request; the
    /// render shown asked for again drops it and leaves a frame alone or
    /// the depth map (a compare stays).
    func open(_ render: RenderInfo) async {
        guard render.stack != stackID else {
            guard !held, opening?.target.render != render.id else { return }
            if render.id == result?.renderID {
                // Back to the render shown: what was on its way drops.
                if let opening {
                    viewGeneration += 1
                    endOpening(opening.generation)
                }
                backToResult()
                return
            }
            leaveMode()
            await showResult(of: render.id)
            return
        }
        guard !busy else { return }
        leaveMode()
        let id = render.id
        await switchStack(to: render.stack, opening: .render(id), fallback: nil) { request in
            await showResult(of: id, continuing: request)
            await loadSourcePreview()
        }
    }

    /// A new view request, `busy` until [`endOpening`].
    func beginOpening(_ target: Opening.Target) -> Int {
        addedToExisting = nil
        viewGeneration += 1
        setOpening(Opening(target: target, generation: viewGeneration))
        return viewGeneration
    }

    /// Open `generation` lands on `landing`: from now its render is named
    /// and its stand-in shown.
    func expectLanding(_ landing: Opening.Landing, generation: Int) {
        guard let opening, opening.generation == generation,
              case .stack(let stack, .none) = opening.target
        else { return }
        let target = Opening.Target.stack(stack, landing: landing)
        setOpening(Opening(target: target, generation: generation))
    }

    /// The render on screen, or the one an open lands on while none is:
    /// what the home control and subtitle name, so they read the same
    /// throughout an open.
    var namedRender: RenderInfo? {
        guard let id = result?.renderID ?? opening?.target.lands else { return nil }
        return history.renders.first { $0.id == id }
    }

    /// While a stack's open has its picture on the way; never once a
    /// result is shown.
    var standIn: CGImage? {
        result == nil ? opening?.standIn : nil
    }

    /// Opening request `generation` is over, landed or not.
    func endOpening(_ generation: Int) {
        guard opening?.generation == generation else { return }
        setOpening(nil)
    }

    /// Show render `id` of the stack on screen once its picture lands; the
    /// list and settings stay, as does the picture shown meanwhile. A
    /// compare with this render becomes one with the render shown before.
    ///
    /// `continuing`: the request of an open under way that ends with this
    /// render, so what it holds stays held until the picture is there.
    func showResult(of id: String, continuing request: Int? = nil) async {
        guard let engine else { return }
        let generation: Int
        if let request {
            guard viewGeneration == request, opening?.generation == request else { return }
            generation = request
        } else {
            generation = beginOpening(.result(id))
        }
        defer { if request == nil { endOpening(generation) } }
        // A frame picked while it loads stays on screen.
        let (picked, before) = (selectedFile, result?.renderID)
        let relinks = engine.relinks()
        do {
            let stack = try await Request().run { try await engine.loadRender(request: $0, id: id) }
            await show(
                stack, renderID: id, generation: generation, swapping: before,
                keepsView: { [weak self] in self?.selectedFile != picked }, readAt: relinks)
        } catch {
            if viewGeneration == generation { report(error) }
        }
    }

    /// Show `stack` as render `renderID` unless something else was shown
    /// since `generation`: the one place a picture lands in `result`, and
    /// open `generation` ends in the same step, so nothing shows between
    /// the picture and the controls it enables. `swapping`: the render
    /// compared with now. `keepsView`: leave the view mode. `readAt`: the
    /// engine's relink count (`Engine.relinks`) when `stack` was read; a
    /// stack relinked since is read again, so its frames' paths are
    /// current, and the view's frames follow (`retargetView`).
    func show(
        _ stack: StackResult, renderID: String, generation: Int, swapping: String? = nil,
        keepsView: () -> Bool = { false }, readAt: UInt64? = nil
    ) async {
        // Overtaken: dropped at each step, before anything is made, opened
        // or read again.
        guard viewGeneration == generation else { return overtaken() }
        let relinks = readAt ?? engine?.relinks() ?? 0
        let side = Self.displaySide
        // Full size, tiled for a zoom past it (ADR-164), with their bases;
        // the engine then lets go of its displays. Each picture keeps its
        // pyramid.
        let pictures: ResultPictures
        do {
            pictures = try await Request().run { try await stack.pictures(request: $0, baseSide: side) }
        } catch EngineError.Canceled {
            // Stopped as the app left the foreground: shown once back,
            // unless something else was shown meanwhile.
            guard viewGeneration == generation else { return }
            stateLog.debug("opening: its pictures stopped, shown again in front")
            stopped { [weak self] in
                guard let self, self.viewGeneration == generation else { return }
                Task { await self.showResult(of: renderID) }
            }
            return
        } catch {
            if viewGeneration == generation { report(error) }
            return
        }
        guard viewGeneration == generation else { return overtaken() }
        // Ready to draw: committing it as Stacking slides in decodes
        // nothing.
        let plain = pictures.display?.base
        let display = (try? await Blocking.run { plain?.makeCGImage().map(Self.preparedForDisplay) }) ?? nil
        guard viewGeneration == generation else { return overtaken() }
        let map = pictures.map?.base?.makeCGImage()
        let (plainPicture, mapPicture) = (pictures.display?.picture, pictures.map?.picture)
        let (frames, contributions, colors) = (stack.frames(), stack.frameContributions(), stack.frameColors())
        // Its frames are read to paint from, compare with and for export
        // metadata: held while it is shown.
        let lease = await access(frames)
        let leftOut = Dictionary(
            stack.leftOut().map { ($0.path, $0.reason) },
            uniquingKeysWith: { first, _ in first }
        )
        guard viewGeneration == generation else { return }
        if moved(frames + leftOut.keys, since: relinks), let engine {
            let again = engine.relinks()
            stateLog.debug("opening: its frames moved, reading the render again")
            do {
                let fresh = try await Request().run { try await engine.loadRender(request: $0, id: renderID) }
                await show(
                    fresh, renderID: renderID, generation: generation, swapping: swapping,
                    keepsView: keepsView, readAt: again)
            } catch {
                if viewGeneration == generation { report(error) }
            }
            return
        }
        setResult(ShownResult(
            stack: stack, renderID: renderID, display: display, plainDisplay: display, map: map,
            // Turned as handed over.
            fullSize: PanePicture.size(of: plainPicture)
                ?? CGSize(width: Int(stack.width()), height: Int(stack.height())),
            displayPicture: DisplayPicture(plainPicture), mapPicture: mapPicture,
            legend: ShownResult.legend(colors, frames: frames), leftOut: leftOut, stacked: Set(frames),
            contributions: contributions, lease: lease))
        if let swapping, swapping != renderID { swapCompare(shown: renderID, before: swapping) }
        retargetView()
        backToResult(keepingView: keepsView())
        endOpening(generation)
        // A retouched render shows its paint.
        paint.present(stack, renderID: renderID)
    }

    /// Overtaken by a newer open or the view moving on: nothing is shown.
    private func overtaken() {
        stateLog.debug("opening: a render's open overtaken, dropped")
    }

    /// `image` decoded for the screen on the caller's (`Blocking`) thread,
    /// never the main one, so the viewer's first draw commits nothing
    /// heavy. Itself where it can't be.
    nonisolated static func preparedForDisplay(_ image: CGImage) -> CGImage {
        UIImage(cgImage: image).preparingForDisplay()?.cgImage ?? image
    }
}
