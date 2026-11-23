import CoreGraphics
import Foundation
import os
import SwiftUI
import UIKit
@preconcurrency import DeptheosEngine

nonisolated private let viewLog = AppLog(category: "view")

extension AppModel {
    /// Why `row` was left out of the result on screen, if it was.
    func leftOutReason(for row: ListRow) -> String? {
        guard let leftOut = result?.leftOut, !leftOut.isEmpty else { return nil }
        if let why = leftOut[row.file] { return why }
        return row.files.lazy.compactMap { leftOut[$0] }.first
    }

    var leftOutRows: [ListRow] {
        rows.filter { leftOutReason(for: $0) != nil }
    }

    /// Select the first frame the result on screen left out.
    func showLeftOut() async {
        guard let first = leftOutRows.first else { return }
        // The sheet's frames, or the column's at regular width, where the
        // reason is under its name.
        if panel.isOpen { setPanel(.sources(.frames, .low)) }
        framesRequest += 1
        await select(first.file)
    }
}

extension AppModel {
    /// Shown once there is a result.
    var compareState: ViewCompare? { viewState.compare }

    var comparing: Bool {
        compareState != nil && result?.display != nil && stacking == nil && !paint.active
    }

    var showsFrameAlone: Bool {
        viewState.frame != nil
    }

    var showsDepthMap: Bool {
        viewState.light != nil
    }

    /// The render shown is on screen, alone or as a compare's result side;
    /// its row is checked in the menus and the home control lit.
    var showsRenderShown: Bool {
        guard result?.display != nil, stacking == nil, !paint.active else { return false }
        return viewState.isResultAlone || comparing
    }

    /// The frame shown alone or compared with, whose overlay Contribution
    /// toggles.
    var overlaidFrame: String? {
        switch viewState {
        case .frame: selectedFile
        case .compare: comparing ? compareWith?.framePath : nil
        case .result, .map: nil
        }
    }

    var frameOverlay: Overlay? {
        switch viewState {
        case .frame(let overlay): overlay
        case .compare:
            if case .frame(_, let overlay)? = compareWith, comparing { overlay } else { nil }
        case .result, .map: nil
        }
    }

    /// The view says the frame in view supplied part of the result.
    var canShowContribution: Bool {
        contributionToggle?.canChange == true && stacking == nil && !paint.active
    }

    /// Shown over the frame in view, or, over a frame that supplied
    /// nothing, kept for the next that did.
    var contributionChosen: Bool { contributionToggle?.on == true }

    /// The result kept a frame map and stacked `file`.
    func canShowContribution(of file: String?) -> Bool {
        guard let file, let result, result.display != nil else { return false }
        return result.stacked.contains(file) && result.stack.hasContribution()
    }

    /// The frame selected while it is in the list, else the first
    /// included, else the first.
    var compareFrame: String? {
        if let file = selectedFile, rows.contains(where: { $0.file == file }) { return file }
        return includedRows.first?.file ?? rows.first?.file
    }

    /// A render, or `compareFrame` with the view's overlay.
    var compareWith: CompareTarget? {
        switch compareState?.target {
        case .render(let id)?: .render(id: id)
        case .frame(let overlay)?: compareFrame.map { .frame(path: $0, overlay: overlay) }
        case nil: nil
        }
    }

    /// Contribution over its frame only where it can be.
    func source(of target: CompareTarget) -> CompareSource {
        switch target {
        case .frame(let path, let overlay):
            .frame(path, contribution: overlay == .contribution)
        case .render(let id): .render(id)
        }
    }

    /// What is compared, or while that loads, the one before it.
    var shownCompare: CompareImage? {
        guard compareState != nil, let image = alignedLoad.shown, let result,
              image.result == ObjectIdentifier(result.stack)
        else { return nil }
        return image
    }

    /// The frame alone with Contribution over it once lined up (or an
    /// earlier one while it loads).
    var shownContributionFrame: CompareImage? {
        guard case .frame(overlay: .contribution) = viewState,
              let image = alignedLoad.shown, let result, image.result == ObjectIdentifier(result.stack)
        else { return nil }
        return image
    }

    /// The frame whose pixels a frame alone shows. During a drag it can be
    /// one the finger has left, until the one it rests on lands.
    var shownFrameFile: String? {
        shownContributionFrame?.source.framePath ?? sourcePreview.flatMap { $0.image != nil ? $0.file : nil }
    }

    /// "Frame 23 · spider-24.jpg", "#3".
    func label(of target: CompareTarget) -> UserText {
        switch target {
        case .frame(let file, _):
            let name = (file as NSString).lastPathComponent
            return frameNumber(of: file).map { .line([.app(frameLabel($0)), .verbatim(name)]) } ?? .verbatim(name)
        case .render(let id):
            return history.renders.first { $0.id == id }?.numberLabel ?? .app(Self.renderLabel)
        }
    }

    /// A render whose number isn't known.
    nonisolated static let renderLabel = LocalizedStringResource(
        "Render", comment: "A render whose number isn't known, where '#4' would name it.")

    /// "Result · #4".
    var resultLabel: LocalizedStringResource {
        let number = history.renders.first { $0.id == result?.renderID }?.number
        return number.map {
            LocalizedStringResource(
                "Result · #\($0)",
                comment: "The label over the result in the viewer. The placeholder is the render's number.")
        } ?? LocalizedStringResource("Result", comment: "The label over the result in the viewer, when its render's number isn't known.")
    }

    /// The others of its stack, newest first.
    var compareRenders: [RenderInfo] {
        let shown = result?.renderID
        return history.renders.filter { $0.stack == stackID && $0.id != shown }.reversed()
    }

    /// "Frame 23 vs #4", "#3 vs #4" while comparing: the title on iPhone,
    /// under the stack's name.
    var compareTitle: UserText? {
        guard comparing, let target = compareWith else { return nil }
        let number = history.renders.first { $0.id == result?.renderID }?.number
        switch target {
        case .frame(let file, _):
            switch (frameNumber(of: file), number) {
            case (let frame?, let number?):
                return .app(LocalizedStringResource(
                    "Frame \(frame) vs #\(number)",
                    comment: "The viewer's title on iPhone while comparing a frame with the result. The placeholders are the frame's number and the result's render number."))
            case (nil, let number?):
                return .app(LocalizedStringResource(
                    "Frame vs #\(number)",
                    comment: "The viewer's title on iPhone while comparing a frame with the result. The placeholder is the result's render number."))
            case (let frame?, nil):
                return .app(frameLabel(frame))
            case (nil, nil):
                return .app(LocalizedStringResource(
                    "Frame", comment: "The viewer's title on iPhone while comparing a frame whose number isn't known."))
            }
        case .render(let id):
            let compared = history.renders.first { $0.id == id }?.number
            switch (compared, number) {
            case (let compared?, let number?):
                return .app(LocalizedStringResource(
                    "#\(compared) vs #\(number)",
                    comment: "The viewer's title on iPhone while comparing two renders. The placeholders are the compared render's number and the result's."))
            case (nil, let number?):
                return .app(LocalizedStringResource(
                    "Render vs #\(number)",
                    comment: "The viewer's title on iPhone while comparing a render with the result. The placeholder is the result's render number."))
            case (let compared?, nil):
                return .verbatim(AppWords.words.renderName(number: compared, name: nil, picked: false))
            case (nil, nil):
                return .app(Self.renderLabel)
            }
        }
    }

    /// Under the stack's name: "#4", "#3 · Edited" while the working
    /// version differs from its base, "Not stacked yet" with frames and no
    /// render, "Frame 23 vs #4" while comparing. `full`, at regular width,
    /// adds the method and frames ("#4 · Depth · 44 of 46 frames ·
    /// Edited"). While a stack opens, the render it lands on, so the title
    /// doesn't reflow as its picture lands.
    func viewerSubtitle(full: Bool = false) -> UserText {
        let edited: [UserText] = isEdited ? [.app(Self.editedLabel)] : []
        if let compareTitle { return full ? .line([compareTitle] + edited) : compareTitle }
        guard let render = namedRender else {
            let unstacked = stackRenders.isEmpty && !rows.isEmpty
            return .line(unstacked ? [.app(Self.notStackedLabel)] : [])
        }
        var parts: [UserText] = [render.numberLabel]
        if full {
            let included = render.included.filter { $0 }.count
            parts.append(.app(AlgorithmKind(render.settings.algorithm).label))
            parts.append(.app(
                included == render.frames.count
                    ? framesCount(render.frames.count) : framesIncluded(included, of: render.frames.count)))
        }
        return .line(parts + edited)
    }

    /// The one compared with, the one lit on the depth map, else the one
    /// selected.
    var steppedFile: String? {
        if comparing, let file = compareWith?.framePath { return file }
        if let file = litFrame { return file }
        return selectedFile
    }

    /// What the result is compared with is a render.
    var stepsRenders: Bool {
        comparing && compareWith?.renderID != nil
    }

    /// Where the arrows are among what they step through, and how many.
    private var stepped: (at: Int?, count: Int) {
        if stepsRenders, let id = compareWith?.renderID {
            let renders = compareRenders
            return (renders.firstIndex { $0.id == id }, renders.count)
        }
        return (steppedFile.flatMap { file in rows.firstIndex { $0.file == file } }, rows.count)
    }

    func canStep(by step: Int) -> Bool {
        let (at, count) = stepped
        guard count > 0 else { return false }
        guard let at else { return true }
        return (0..<count).contains(at + step)
    }

    /// ← and →: the render `step` places on in the Renders list while
    /// comparing with one, else the frame `step` places on, compared or
    /// alone as now.
    func step(by step: Int) async {
        if stepsRenders {
            let renders = compareRenders
            guard !renders.isEmpty else { return }
            let at = stepped.at ?? (step > 0 ? -1 : renders.count)
            compare(withRender: renders[min(max(at + step, 0), renders.count - 1)].id)
            return
        }
        guard !rows.isEmpty else { return }
        let at = stepped.at ?? (step > 0 ? -1 : rows.count)
        let next = min(max(at + step, 0), rows.count - 1)
        await select(rows[next].file)
    }

    // MARK: The focus rail

    /// The rail's frames: the render shown's, each with its share (none
    /// without a frame map), whether it was left out and, while the depth
    /// map shows, its legend color, so the rail is the map's legend.
    /// Before a first render, the list's.
    var railFrames: [RailFrame] {
        guard let result else {
            return rows.map { RailFrame(path: $0.file, share: nil, leftOut: false) }
        }
        let legend = showsMapLegend ? result.legend : [:]
        if let contributions = result.contributions {
            return contributions.map {
                RailFrame(path: $0.path, share: $0.share, leftOut: $0.leftOut, legend: legend[$0.path])
            }
        }
        let paths: [String] = if let render = shownRender {
            zip(render.frames, render.included).filter(\.1).map(\.0)
        } else {
            result.stack.frames()
        }
        return paths.map {
            RailFrame(path: $0, share: nil, leftOut: result.leftOut[$0] != nil, legend: legend[$0])
        }
    }

    /// A finished render's depth map is on screen, in the viewer or
    /// Retouch: the rail takes its legend colors.
    private var showsMapLegend: Bool {
        guard stacking == nil, result?.map != nil else { return false }
        return paint.active ? paint.showsDepthMap : showsDepthMap
    }

    /// The frame on screen by its rail place: none for the result, a
    /// render or the whole map.
    func railSelection(in frames: [RailFrame]) -> Int? {
        let file: String? = switch viewState {
        case .frame: selectedFile
        case .compare: compareWith?.framePath
        case .map(_, let light): light.file
        case .result: nil
        }
        guard let file else { return nil }
        return frames.firstIndex { $0.path == file }
    }

    /// The rail scrubbed to `file`: lit on the depth map, compared with,
    /// else shown alone. Its loads go one at a time, newest only; each that
    /// lands meanwhile shows, labeled as itself. Comparing loads no
    /// preview (`requestSourcePreview`).
    func scrub(to file: String) {
        view(.showFrame(frame: pick(file), putOut: false))
    }

    // MARK: The frame's preview

    /// The view shows the selected frame's preview, or may at a touch: a
    /// frame alone, or the result alone while held. Not a compare, the
    /// depth map or Retouch.
    var needsSourcePreview: Bool {
        guard !paint.active else { return false }
        switch viewState {
        case .frame, .result: return true
        case .compare, .map: return false
        }
    }

    /// The frame selected's preview, or while it loads the one before it.
    var sourcePreview: FramePreview? { previewLoad.shown }

    /// The one place `previewLoad` changes, logged.
    func setPreviewLoad(_ load: PreviewLoad) {
        let (old, new) = (previewLoad.logName, load.logName)
        previewLoad = load
        if old != new { viewLog.debug("preview: \(old, privacy: .public) → \(new, privacy: .public)") }
    }

    /// Ask `previewLoads` for the selected frame's preview if the view
    /// needs it and it isn't held or loading (`retry`: again after a
    /// failed read). Called as the selection or view changes.
    ///
    /// Previews load one at a time so they land in order (ImagePreview's
    /// queue is concurrent). One lands only while its stack is on screen
    /// with the frame in its list, and, overtaken during a drag, only if
    /// newer than the one held.
    func requestSourcePreview(retry: Bool = false) {
        // The frame held already: its full size, once settled; another:
        // the one settling was left.
        if sourcePreview?.file == selectedFile { requestFullFrame() } else { frameSettling?.cancel() }
        guard let file = selectedFile else { return }
        guard needsSourcePreview else {
            // Asked for a frame no longer selected, with nothing to show it.
            if let asked = previewLoad.asked, asked.file != file { setPreviewLoad(previewLoad.abandoned) }
            return
        }
        let stack = stackID
        switch previewLoad {
        case .loaded(let held) where held.file == file && (held.image != nil || !retry):
            return
        case .loading(let asked, _) where asked.file == file && asked.stack == stack:
            return
        // Held already, as a drag came back to it: nothing to load.
        case .loading(_, let held?) where held.file == file && held.image != nil:
            setPreviewLoad(.loaded(held))
            requestFullFrame()
            return
        case .none, .loading, .loaded, .failed:
            break
        }
        let request = PreviewRequest(ticket: previewLoads.ticket(), file: file, stack: stack)
        setPreviewLoad(previewLoad.asking(request))
        previewLoads.load(
            request,
            latest: { [weak self] in
                // Only while its stack is on screen with the frame in its list.
                guard let self, self.stackID == stack, self.rows.contains(where: { $0.file == file }) else { return nil }
                return self.previewLoad
            },
            set: { [weak self] in self?.setPreviewLoad($0) },
            answers: { preview, asked in preview.file == asked.file },
            holds: { [weak self] preview, _, _ in preview.image != nil && self?.showsFrameAlone == true },
            make: { [weak self] in await self?.loadPreview(request) },
            landed: { [weak self] _ in self?.requestFullFrame() })
    }

    /// Returns once this load, or a newer one, has landed or dropped.
    func loadSourcePreview() async {
        guard selectedFile != nil else {
            setPreviewLoad(.none)
            return
        }
        requestSourcePreview(retry: true)
        await previewLoads.settled()
    }

    /// `nil` for its image when it can't be read.
    private func loadPreview(_ request: PreviewRequest) async -> FramePreview {
        let file = request.file
        let lease = await access([file])
        defer { withExtendedLifetime(lease) {} }
        var image: CGImage?
        if unavailable[file] == nil {
            let started = ContinuousClock.now
            image = await ImagePreview.load(URL(fileURLWithPath: file), maxPixelSize: 2048, urgent: true)
            let (width, height) = (image?.width ?? 0, image?.height ?? 0)
            viewLog.debug("preview \(request.ticket.number) took \(Self.milliseconds(since: started)) ms, \(width)×\(height)")
        }
        return FramePreview(file: file, image: image, request: request.ticket)
    }

    // MARK: The frame at full size

    /// As it is, not with Contribution over it (that image has its own).
    private var frameAloneWantsFull: Bool {
        switch viewState {
        case .frame(overlay: .none): true
        case .frame(overlay: .contribution): false
        case .result, .compare, .map: false
        }
    }

    /// The one place a preview's full size changes, logged.
    func setFullFrame(_ full: FrameFull) {
        guard var preview = sourcePreview else { return }
        let old = preview.full.logName
        preview.full = full
        setPreviewLoad(previewLoad.map { $0.request == preview.request ? preview : $0 })
        if old != full.logName {
            viewLog.debug("full frame: \(old, privacy: .public) → \(full.logName, privacy: .public)")
        }
    }

    /// The view no longer shows the full size held.
    func dropFullFrame() {
        frameSettling?.cancel()
        guard let preview = sourcePreview else { return }
        if case .none = preview.full { return }
        setFullFrame(.none)
    }

    /// Load the frame selected at full size once its preview is in, it is
    /// shown alone (or `held`), and the selection has settled. Nothing
    /// when it has it or is loading it.
    func requestFullFrame(held: Bool = false) {
        guard frameWantsPicture, held || frameAloneWantsFull, let preview = sourcePreview,
              preview.file == selectedFile, preview.image != nil, case .none = preview.full, engine != nil
        else { return }
        let (file, request) = (preview.file, preview.request)
        frameSettling?.cancel()
        // A hold asks at once: the finger is on the photo, not the rail.
        let settle: Duration = held ? .zero : .milliseconds(300)
        frameSettling = Task { [weak self] in
            try? await Task.sleep(for: settle)
            // The view may have left the frame, or a stack started, without
            // canceling this.
            guard !Task.isCancelled, let self, self.frameWantsPicture, held || self.frameAloneWantsFull,
                  let now = self.sourcePreview, now.file == file,
                  now.request == request, self.selectedFile == file, case .none = now.full
            else { return }
            let asked = FullRequest(ticket: self.fullFrameLoads.ticket(), call: Request())
            self.setFullFrame(.loading(asked, showing: nil))
            // Stopped as the app left: asked again under a new request.
            self.loadFullFrame(
                asked, of: file, make: { [weak self] in try await self?.decodeFullFrame(file, request: asked) },
                again: { [weak self] in
                    self?.setFullFrame(.none)
                    self?.requestFullFrame()
                })
        }
    }

    /// Load `request` into `file`'s full size while its preview is held.
    func loadFullFrame(
        _ request: FullRequest, of file: String, make: @escaping @MainActor () async throws -> FullFrame?,
        again: (@MainActor () -> Void)? = nil
    ) {
        fullFrameLoads.load(
            request,
            latest: { [weak self] in
                guard let preview = self?.sourcePreview, preview.file == file else { return nil }
                return preview.full
            },
            set: { [weak self] in self?.setFullFrame($0) },
            make: make, again: again)
    }

    /// Decode `file` at full size as the engine stacks it; `nil` when it
    /// can't be, or is no longer wanted.
    private func decodeFullFrame(_ file: String, request: FullRequest) async throws -> FullFrame? {
        guard let engine else { return nil }
        let side = Self.displaySide
        // Camera RAW decoded as the stack's frames are.
        let decoder = stackSettings.rawDecoder
        let started = ContinuousClock.now
        // Canceled once the view moves on, before or during the decode.
        var made: FullFrame?
        do {
            let pictured = try await request.call.run {
                try await engine.framePicture(request: $0, path: file, rawDecoder: decoder, baseSide: side)
            }
            made = pictured.base?.makeCGImage().map {
                FullFrame(
                    request: request.ticket, base: $0, picture: pictured.picture,
                    size: PanePicture.size(of: pictured.picture)!)
            }
        } catch EngineError.Canceled {
            // Stopped as the app left: thrown to its load, which asks again
            // once back.
            viewLog.debug("full frame \(request.ticket.number) stopped after \(Self.milliseconds(since: started)) ms")
            throw EngineError.Canceled
        } catch {
            viewLog.error("full frame failed: \(error.localizedDescription, privacy: .public)")
        }
        let elapsed = Self.milliseconds(since: started)
        if let made {
            viewLog.debug("full frame \(request.ticket.number) took \(elapsed) ms, \(made.base.width)×\(made.base.height)")
        } else {
            viewLog.debug("full frame \(request.ticket.number) failed after \(elapsed) ms")
        }
        return made
    }

    /// For the log.
    nonisolated static func milliseconds(since start: ContinuousClock.Instant) -> Int {
        let elapsed = ContinuousClock.now - start
        return Int(elapsed.components.seconds * 1000 + elapsed.components.attoseconds / 1_000_000_000_000_000)
    }

    // MARK: Changing it

    // MARK: The view's events

    /// The one way the view changes (ADR-149): the engine's view takes
    /// `event`, its answer becomes `viewState`, and what it says to do is
    /// done; then what it lines up is loaded (`refreshCompare`).
    func view(_ event: ViewEvent) {
        let answer = viewControl.on(event: event)
        for case .show(let path) in answer.actions {
            selectedFile = path
        }
        setView(answer.now, contribution: answer.contribution)
        for action in answer.actions {
            switch action {
            case .light(let frame, let token): light(frame, token: token)
            case .unlight: setLitMap(.whole)
            case .show, .reload: break
            }
        }
        refreshCompare()
    }

    /// `file` with whether it supplied part of the result, so it takes
    /// Contribution and lights the map.
    func pick(_ file: String) -> FramePick {
        FramePick(path: file, supplied: canShowContribution(of: file))
    }

    /// What the view needs to know of the result shown, if one is.
    private var resultFacts: Facts? {
        result.map { Facts(map: $0.map != nil, lights: $0.stack.hasContribution()) }
    }

    /// Result (1), Off, Compare turned off, a one-render stack's home
    /// control, Retouch opened: the result alone.
    func showResultAlone() {
        view(.look(look: .result))
    }

    /// A render landed, a stack started or ended, the render shown picked
    /// again: back from a frame or the depth map to the result; a compare
    /// stays. `keepingView` (a frame picked while it opened): the view
    /// stays, and the map shows this render's whole.
    func backToResult(keepingView keep: Bool = false) {
        view(.shown(result: resultFacts, keep: keep, supplied: canShowContribution(of: compareFrame)))
    }

    /// A stack opening shows the stand-in of the render it lands on, alone.
    func showStandIn() {
        view(.shown(result: Facts(map: false, lights: false), keep: false, supplied: false))
    }

    /// No result to show: the frame selected, plain.
    func showPlainFrame() {
        view(.shown(result: nil, keep: false, supplied: false))
    }

    /// A layout chosen (the view menu, 2, 3, 4, \), what it is with kept.
    /// Flicker chosen again flips.
    func setLayout(_ kind: CompareLayout.Kind) {
        guard compareState != nil || compareFrame != nil else { return }
        view(.look(look: kind.look))
    }

    /// Compare on, in the last layout, or off to the result alone.
    func toggleCompare() {
        guard compareState != nil || compareFrame != nil else { return }
        view(.toggleCompare)
    }

    func flip() {
        guard case .flicker? = compareState?.layout else { return }
        view(.look(look: .flicker))
    }

    /// Compare with frame `file`, selected, keeping the overlay: in the
    /// compare on screen, else a new one in the last layout.
    func compare(withFrame file: String) {
        view(.compareFrame(frame: pick(file)))
    }

    /// The sides as they are.
    func compare(withRender id: String) {
        view(.compareRender(id: id))
    }

    /// The render compared with became the one shown: compare with
    /// `before` instead, sides swapped, so each picture stays in place.
    func swapCompare(shown: String, before: String) {
        view(.traded(shown: shown, before: before))
    }

    /// Contribution (C): what the result took from the frame in view, the
    /// rest dimmed.
    func toggleContribution() {
        guard canShowContribution else { return }
        view(.toggleContribution)
    }

    /// Depth Map (M): on, lit for the frame in view when it has
    /// Contribution on and the map can be lit, else whole; or off, a frame
    /// lit landing selected with Contribution on, a frame left out plain.
    func toggleDepthMap() {
        guard result?.map != nil, stacking == nil else { return }
        view(.toggleMap(onScreen: overlaidFrame.map(pick)))
    }

    // MARK: The depth map lit

    /// The depth map in the viewer can be lit frame by frame: the render
    /// kept a frame map. Retouch's map is lit in `PaintView.depthMap`.
    var canLightMap: Bool {
        guard showsDepthMap, stacking == nil, let result, result.map != nil else { return false }
        return result.stack.hasContribution()
    }

    /// While the map is on screen lit with `token` (any light, without one).
    private func mapLighting(token: UInt64? = nil) -> MapLighting? {
        guard let light = viewState.light, token == nil || light.token == token else { return nil }
        return litMap
    }

    var litFrame: String? {
        viewState.light?.file
    }

    /// The frame lit's picture, or while it loads the one before it; `nil`
    /// for the map whole.
    var shownLitMap: LitMap? {
        guard let image = mapLighting()?.light?.picture.shown, let result,
              image.result == ObjectIdentifier(result.stack)
        else { return nil }
        return image
    }

    /// The lit picture's frame, or a left-out frame's, which shows the map
    /// whole; else `nil`.
    var shownLitFile: String? {
        guard let light = viewState.light else { return nil }
        if case .marked(let file) = light { return file }
        return shownLitMap?.file
    }

    /// "Depth Map", or "Depth Map · Frame 18": the label over the map.
    func depthMapLabel(lit file: String?) -> UserText {
        if let number = frameNumber(of: file) {
            .line([.app(ViewerDeck.depthMap), .app(frameLabel(number))])
        } else {
            .app(ViewerDeck.depthMap)
        }
    }

    /// Light the map for `file`, as the view asked with `token`, through
    /// `mapLighter`, the lit picture before held until it lands. A picture
    /// that can't be made puts the light out (`ViewEvent.lightFailed`).
    private func light(_ file: String, token: UInt64) {
        guard let result else { return }
        mapLighter.light(
            file, stacked: true, stack: result.stack,
            current: { [weak self] in self?.mapLighting(token: token) },
            set: { [weak self] light in self?.setLitMap(light) },
            failed: { [weak self] _ in self?.view(.lightFailed(token: token)) })
    }

    /// The one place `litMap` changes but for `setView`'s letting it go.
    private func setLitMap(_ light: MapLighting) {
        guard showsDepthMap else { return }
        let (old, new) = (litMap.logName, light.logName)
        litMap = light
        if old != new { viewLog.debug("lit map: \(old, privacy: .public) → \(new, privacy: .public)") }
    }

    /// The lit tick tapped again, the rail dragged past its first frame,
    /// Show Whole Map: the map whole, nothing to load.
    func unlightMap() {
        view(.unlight)
    }

    /// A tap on the depth map at `point` (fractions of the picture): lights
    /// the frame the result took that point from, or on the frame lit, the
    /// whole map. `false` when it can't, so the tap acts as on the photo.
    func tapMap(at point: CGPoint) -> Bool {
        guard canLightMap, let result, (0...1).contains(point.x), (0...1).contains(point.y) else { return false }
        let stack = result.stack
        let (x, y) = (Float(point.x) * Float(stack.width()), Float(point.y) * Float(stack.height()))
        guard let file = stack.frameAt(x: x, y: y) else { return false }
        view(.showFrame(frame: pick(file), putOut: true))
        return true
    }

    /// The frame selected where it is now, after its folder was relinked
    /// (`AppModel.relinked`), so the view stays on the same frame.
    func retargetView() {
        guard let result, let file = selectedFile,
              !result.stacked.contains(file), result.leftOut[file] == nil
        else { return }
        let now = pathNow(file)
        if now != file { selectedFile = now }
    }

    /// Load what the view lines up with the result unless shown, loading
    /// or failed already. One at a time, newest only (`LatestLoad`); each
    /// applies only while the view holds its request, or, overtaken, as the
    /// image held meanwhile (`CompareLoad`).
    func refreshCompare() {
        guard stacking == nil, !paint.active, let result, result.display != nil else { return }
        let id = ObjectIdentifier(result.stack)
        let source: CompareSource
        let load: CompareLoad
        switch viewState {
        case .compare:
            guard let target = compareWith else { return }
            (source, load) = (self.source(of: target), alignedLoad)
        case .frame(overlay: .contribution):
            guard let file = selectedFile else { return }
            (source, load) = (.frame(file, contribution: true), alignedLoad)
        case .frame(overlay: .none), .result, .map:
            return
        }
        // An unaligned frame can't take Contribution: it stays as it is.
        func shows(_ image: CompareImage) -> Bool {
            image.result == id
                && (image.source == source || (!image.aligned && image.source.framePath == source.framePath))
        }
        switch load {
        case .loading(let request, _) where request.asks(source, result: id):
            return
        case .loaded(let image) where shows(image):
            return
        // Held already, as a drag came back to it: nothing to load.
        case .loading(_, let held?) where shows(held) && held.complete:
            setLoad(.loaded(held))
            return
        case .failed(let request) where request.asks(source, result: id):
            return
        case .none, .loading, .loaded, .failed:
            break
        }
        let request = CompareRequest(ticket: alignedLoads.ticket(), source: source, result: id)
        setLoad(load.asking(request) { $0.result == id })
        let stack = result.stack
        loadAligned(request) { [weak self] in try await self?.lineUp(request, against: stack) }
    }

    /// The one place `alignedLoad` changes but for `setView`'s letting it go.
    func setLoad(_ load: CompareLoad) {
        guard viewState.linesUp != nil else { return }
        let (old, new) = (alignedLoad.logName, load.logName)
        alignedLoad = load
        if old != new { viewLog.debug("aligned load: \(old, privacy: .public) → \(new, privacy: .public)") }
    }

    /// A compare's, or a frame alone's Contribution one.
    private var viewLoad: CompareLoad? {
        switch viewState {
        case .compare, .frame(overlay: .contribution): alignedLoad
        case .frame(overlay: .none), .result, .map: nil
        }
    }

    /// Load `request` while the view still waits for it. One overtaken is
    /// held while the newer loads if both are for the same result and of
    /// the same kind; one the view asks for now lands as it.
    func loadAligned(_ request: CompareRequest, make: @escaping @MainActor () async throws -> CompareImage?) {
        alignedLoads.load(
            request,
            latest: { [weak self] in self?.viewLoad },
            set: { [weak self] in self?.setLoad($0) },
            answers: { image, asked in asked.asks(image.source, result: image.result) },
            holds: { _, overtaken, asked in
                asked.result == overtaken.result && asked.source.isKind(of: overtaken.source)
            },
            make: make,
            landed: { [weak self] made in if made == nil { self?.compareFailed() } })
    }

    /// Line up what `request` asks for with `stack`; `nil` when it can't be.
    private func lineUp(_ request: CompareRequest, against stack: StackResult) async throws -> CompareImage? {
        let side = Self.displaySide
        let engine = engine
        let source = request.source
        let started = ContinuousClock.now
        // The picture and its base, made together: one pyramid.
        var (image, picture, aligned): (CGImage?, Picture?, Bool) = (nil, nil, false)
        do {
            let pictured = try await Request().run { request -> Pictured? in
                switch source {
                case .frame(let file, let contribution):
                    let of = PictureOf.comparedFrame(path: file, contribution: contribution)
                    return try await stack.picture(request: request, of: of, baseSide: side)
                case .render(let id):
                    guard let engine else { return nil }
                    let other = try await engine.loadRenderPixels(request: request, id: id)
                    return try await stack.picture(request: request, of: .comparedRender(other: other), baseSide: side)
                }
            }
            if let pictured { (image, picture, aligned) = (pictured.base?.makeCGImage(), pictured.picture, true) }
        } catch EngineError.Canceled {
            // Stopped as the app left: its load asks again once back.
            throw EngineError.Canceled
        } catch {
            viewLog.error("compare failed: \(error.localizedDescription, privacy: .public)")
        }
        // A frame that couldn't be lined up is shown as it is.
        if image == nil, let file = source.framePath {
            image = await ImagePreview.load(URL(fileURLWithPath: file), maxPixelSize: Int(side), urgent: true)
            picture = nil
            aligned = false
        }
        let kind = switch source {
        case .frame(_, contribution: true): "frame with Contribution"
        case .frame: "frame"
        case .render: "render"
        }
        viewLog.debug("""
            compare load \(request.ticket.number) (\(kind, privacy: .public)\(aligned ? "" : ", not aligned", privacy: .public)) \
            took \(Self.milliseconds(since: started)) ms, \(image?.width ?? 0)×\(image?.height ?? 0)
            """)
        let contribution = if case .frame(_, contribution: true) = source { aligned } else { false }
        return image.map {
            CompareImage(
                request: request.ticket, source: source, result: request.result, image: $0, picture: picture,
                aligned: aligned, contribution: contribution)
        }
    }

    /// Said, and a render compared with gives way to the frame.
    private func compareFailed() {
        guard let now = compareState else { return }
        errorMessage = ErrorMessage(.app(LocalizedStringResource(
            "The frame or render you picked couldn't be loaded to compare with this result.",
            comment: "Alert text: what the result was to be compared with couldn't be loaded.")))
        // A render that couldn't be loaded: the frame instead.
        if case .render = now.target, let frame = compareFrame {
            compare(withFrame: frame)
        }
    }
}
