import CoreGraphics
import Foundation
import Observation
import SwiftUI
import UniformTypeIdentifiers
import os
@preconcurrency import DeptheosEngine

private let captureLog = AppLog(category: "capture")
nonisolated private let appLog = AppLog(category: "app")
/// Each lifecycle transition, one line.
private let stateLog = AppLog(category: "state")

/// The viewer's zoom, kept in the model so a rebuilt viewer (the Duo
/// opened or closed) picks it up.
struct ViewerZoom {
    let image: ObjectIdentifier
    /// As fractions of the image's size.
    let visible: CGRect
}

extension ListRow: @retroactive Identifiable {
    public var id: String { file }
    var name: String { (file as NSString).lastPathComponent }
    var url: URL { URL(fileURLWithPath: file) }
}

/// What is running, as the desktop's `Running`. A batch runs its stacks
/// one at a time; the stack's own state is its `StackingState`.
///
/// Lifecycle: idle, stacking or batch (its run, with its stack, if any).
/// Changed only by `AppModel.setRunning`, through `startStacking`,
/// `endStacking`, `updateStacking` and `updateBatch`.
///
///     idle ──Stack──▶ stacking(s)
///     stacking(s) ──kept, canceled or failed──▶ idle
///     idle ──Start in the Batch sheet──▶ batch(run, nil)
///     batch(run, nil) ──its queue says stack──▶ batch(run, s)
///     batch(run, s) ──that stack kept, canceled or failed──▶ batch(run, nil)
///     batch(run, nil) ──its summary──▶ idle
enum Running {
    case idle
    case stacking(StackingState)
    case batch(BatchRun, stack: StackingState?)

    var isIdle: Bool {
        if case .idle = self { true } else { false }
    }

    /// The stack running, alone or as its batch's.
    var stack: StackingState? {
        switch self {
        case .idle: nil
        case .stacking(let stack): stack
        case .batch(_, let stack): stack
        }
    }

    var batch: BatchRun? {
        if case .batch(let run, _) = self { run } else { nil }
    }

    /// For the log.
    var name: String {
        switch self {
        case .idle: "idle"
        case .stacking: "a stack"
        case .batch(_, nil): "a batch, between stacks"
        case .batch(_, _?): "a batch, stacking"
        }
    }
}

/// The model, as the desktop's: one list of images, each included or
/// excluded, and one history of every render.
@Observable
final class AppModel {
    /// The engine's start; a second window waits for the first's start.
    ///
    /// Lifecycle: idle, starting (its task), ready or failed (why). Changed
    /// only by `start` and `startOnce`: idle → starting → ready or failed,
    /// once a launch.
    enum EngineState {
        case idle
        case starting(Task<Void, Never>)
        /// The engine and the scopes of folders read in place (ADR-145).
        /// Every device stacks, the Simulator included (ADR-110).
        case ready(Engine, Bookmarks)
        /// Why, in the engine's or the system's words.
        case failed(String)
    }

    /// Longest side of the result kept for display; the full-size pixels
    /// stay in the engine.
    static let displaySide: UInt32 = 4096
    /// Longest side of the live map.
    static let liveSide: UInt32 = 1024

    private(set) var engineState = EngineState.idle
    /// The compact window's sheet over the photo, changed only by
    /// `setPanel` and its kin.
    private(set) var panel = Panel.closed
    /// Which screen (`AppScreen`), changed only by `setScreen`.
    private(set) var screen = AppScreen.library
    /// Changed only by `present` and `dismiss`.
    private(set) var presented = Presented.none
    var showExport: Bool { presented == .export }
    /// The render whose Render Info is open, by id.
    var renderInfo: String? {
        if case .renderInfo(let id) = presented { id } else { nil }
    }
    /// Revert from the menu bar, waiting on its confirmation.
    var revertAsked: RevertInfo? {
        if case .revert(let revert) = presented { revert } else { nil }
    }
    var showCamera: Bool { presented == .camera }

    var list = ListInfo(files: [], excluded: [], source: .raw)
    /// The stack the list is the working version of. Clear, Add… to an
    /// empty list and a camera burst start a new one.
    var stackID = newStackId()
    /// The render the list and settings are "Edited" against.
    var base: String?
    var rows: [ListRow] = []
    /// The frames whose file can't be read, and why (`checkFiles`).
    var unavailable: [String: FrameState] = [:]
    /// The roots of the list's frames outside Deptheos, in list order, as
    /// `checkFiles` last found them.
    var listRoots: [RootInfo] = []
    /// The list's roots held open from one `checkFiles` to the next, so
    /// reads meanwhile find their scopes open, even past
    /// `Bookmarks.openLimit`.
    @ObservationIgnored var listLease: Lease = .none
    /// Rows whose file isn't where the list says or can't be reached, so
    /// can't be stacked. One in iCloud can: it is downloaded.
    var missing: Set<String> {
        Set(unavailable.filter { $0.value != .inCloud }.keys)
    }
    /// Each frame's thumbnail in the Frames list, by file. Changed only by
    /// `setFrameThumb`.
    var frameThumbs: [String: FrameThumb] = [:]
    /// Changed only by `setFrameSelecting`.
    private(set) var frameSelecting: FrameSelecting = .off
    /// The frame selected. A new one loads its preview when the view needs
    /// it (`requestSourcePreview`).
    var selectedFile: String? {
        didSet { if selectedFile != oldValue { requestSourcePreview() } }
    }
    /// The preview of a frame as it loads, changed only by
    /// `setPreviewLoad`. Usually the frame selected; during a drag, the last
    /// to land on the way to it.
    var previewLoad = PreviewLoad.none {
        didSet {
            if let call = oldValue.shown?.full.call, previewLoad.shown?.full.call !== call { call.cancel() }
        }
    }
    /// What the viewer shows, as the engine's reducer last answered
    /// (ADR-149).
    ///
    /// Lifecycle: the result, a frame alone, a comparison or the depth
    /// map, each with its overlay and light. Changed only by `view(_:)`
    /// (AppModel+View), through `setView`.
    private(set) var viewState = ViewState.frame(overlay: .none)
    /// Contribution's toggle over the frame on screen, as the view last
    /// answered; none with no frame on screen.
    private(set) var contributionToggle: ContributionToggle?
    /// The view's reducer, the engine's: `view(_:)` hands it each event.
    @ObservationIgnored let viewControl = ViewControl()
    /// The load of what the view lines up with the result: a compare's
    /// target, or a frame's Contribution. Changed only by `setLoad` and
    /// `fitViewPictures`; dropped by `setView` when the view lines up
    /// something else.
    var alignedLoad = CompareLoad.none
    /// The depth map's lit picture. Changed only by `setLitMap` and the
    /// picture fitting; put out by `setView` when the map is left.
    var litMap = MapLighting.whole
    /// Bumped to fit the photo to the view again.
    var fitRequest = 0
    /// Bumped to turn the Frames column (regular width) to its frames.
    var framesRequest = 0
    /// A text field is being edited: the menu bar's single-key shortcuts
    /// are off.
    var editingText = false
    /// The viewer's zoom; `nil` at fit. Not observed: the viewer reads it
    /// only when it is made.
    @ObservationIgnored var viewerZoom: ViewerZoom?
    /// As a fraction of the photo's width.
    var splitFraction: CGFloat = 0.5

    /// Every render and failed batch stack.
    @ObservationIgnored private(set) lazy var history = HistoryModel(app: self)
    /// The render on screen, with its picture: the one stored state of
    /// which render is shown. How the working version stands against it is
    /// the engine's (`showing`), derived and never kept beside it. A render
    /// asked for waits in `opening` and lands here with its picture
    /// (`show`). Changed only by `setResult`.
    ///
    ///     none ──a stack opened (its base), a batch's end──▶ shown(base)
    ///     shown(r) ──a render picked (its tile, the title menu), once it lands──▶ shown(s)
    ///     shown(r) ──Revert to #s──▶ shown(s), now the base
    ///     any ──Stack finishes──▶ shown(the new render)
    ///     shown(r) ──r deleted──▶ shown(base), or none
    ///     shown(r) ──Retouch's paint, Done──▶ shown(r), with its paint
    ///     any ──another stack, frames removed before a first Stack, its stack deleted──▶ none
    private(set) var result: ShownResult?

    /// Changed only by `setRunning` and its kin.
    private(set) var running = Running.idle
    var stacking: StackingState? { running.stack }
    /// The batch running, or paused until the app comes back.
    var batchRun: BatchRun? { running.batch }
    /// Changed only by `setOpening` (AppModel+Opening).
    private(set) var opening: Opening?
    /// Images and folders picked, on their way into the list; changed only
    /// by `setAdding`.
    private(set) var adding = Adding.none
    /// Try a Sample Stack's copy; changed only by `setSampleCopy`.
    private(set) var sampleCopy = SampleCopy.idle
    /// Why the screen must not lock by itself. The one owner of
    /// `isIdleTimerDisabled`: Auto-Lock is off while any reason holds, so
    /// the camera closing as its stack starts doesn't turn it back on.
    enum Awake: String, Hashable {
        case camera, stack, batch
    }

    /// Changed only by `keepAwake`.
    private(set) var awake: Set<Awake> = []

    func keepAwake(_ reason: Awake, _ on: Bool) {
        let changed = on ? awake.insert(reason).inserted : awake.remove(reason) != nil
        guard changed else { return }
        appLog.debug(
            "awake: \(reason.rawValue, privacy: .public) \(on ? "held" : "let go", privacy: .public), \(self.awake.map(\.rawValue).sorted().joined(separator: ", "), privacy: .public)")
        UIApplication.shared.isIdleTimerDisabled = !awake.isEmpty
    }
    /// How the last batch went, until the next stack. A new one shows the
    /// controls if a tap hid them.
    var notice: UserText? {
        didSet { if notice != nil { showControls() } }
    }
    /// The notice after a delete, with its Undo; changed only by
    /// `setDeleteUndo`.
    private(set) var deleteUndo = DeleteUndo.none
    /// Frames just added to a stack that had frames, offering New Stack
    /// Instead.
    var addedToExisting: AddedToExisting?
    var errorMessage: ErrorMessage?

    let paint = PaintModel()
    /// iOS's memory warnings, heard from launch (`shed`).
    @ObservationIgnored private var memoryPressure: MemoryPressure?
    /// The preferences shared with the desktop.
    let preferences = AppPreferences()

    /// Bumped by whatever replaces what is shown, so overtaken work drops
    /// its results. Adding frames has its own request (`Adding.reading`).
    @ObservationIgnored var viewGeneration = 0
    /// Numbers each read of frames into the list (`Adding.reading`).
    @ObservationIgnored let reads = Tickets()
    /// Read only through `showings` (AppModel+Working).
    @ObservationIgnored let showingsMemo = EngineMemo<ShowingsKey, ShowingsInfo>(name: "showings")
    /// The Library's tiles, read only through `library` (AppModel+Library),
    /// so a redraw that changes none of its key doesn't tile it again.
    ///
    /// Lifecycle: empty, then the engine's last `library::tiles` answer
    /// (ADR-144) with its key. Changed only by `Memo.answer`, from
    /// `library`, whenever its key changes.
    @ObservationIgnored var libraryMemo = Memo<LibraryKey, LibraryShown>()
    /// Read only through `stackCheck`: the engine reads each DNG's header
    /// to say, so a redraw mustn't ask.
    @ObservationIgnored let stackProblemMemo = EngineMemo<StackProblemKey, String?>(name: "stack check")
    /// Read only through `offersRawDecoder`, keyed by the list's files: the
    /// engine reads each DNG's header to say.
    @ObservationIgnored let choosesDecoderMemo = EngineMemo<[String], Bool>(name: "raw decoder offered")
    /// Read only through `renderSetup` (RenderInfoView). Another render's
    /// rows never stand in.
    @ObservationIgnored let renderSetupMemo = EngineMemo<RenderSetupKey, [SetupSectionInfo]>(
        name: "render setup", standsIn: { $0.id == $1.id })
    /// Numbers each `checkFiles`, so an older one drops.
    @ObservationIgnored let fileChecks = Tickets()
    /// What the view lines up with the result, one load at a time
    /// (`CompareRequest`).
    @ObservationIgnored let alignedLoads = LatestLoad(name: "compare load")
    /// Every frame's preview, one at a time (`PreviewRequest`).
    @ObservationIgnored let previewLoads = LatestLoad(name: "preview")
    /// The frame selected at full size once it has settled, the newest
    /// only, apart from the previews so a drag along the rail never waits
    /// for a full decode (`FullRequest`).
    @ObservationIgnored let fullFrameLoads = LatestLoad(name: "full frame")
    /// The result's and the depth map's pictures made again, one at a time.
    @ObservationIgnored let resultPictures = LatestLoad(name: "result's picture")
    @ObservationIgnored let mapPictures = LatestLoad(name: "depth map's picture")
    /// Waits for the selection to settle before its full size loads.
    @ObservationIgnored var frameSettling: Task<Void, Never>?
    /// The depth map lit for a frame, one at a time.
    @ObservationIgnored let mapLighter = MapLighter(name: "lit map")
    /// Loads that leaving the foreground canceled (`leaveForeground`), each
    /// asked again once back (`enteredForeground`), where it checks it is
    /// still wanted.
    @ObservationIgnored private var stoppedLoads: [@MainActor () -> Void] = []
    /// Exports in flight, by id; changed only by `runExport`.
    @ObservationIgnored var exports: [UUID: ExportRun] = [:]

    let stackSettings = StackSettingsModel()

    var settings: StackSettings { stackSettings.settings }

    var engine: Engine? {
        if case .ready(let engine, _) = engineState { engine } else { nil }
    }

    /// Present once the engine has started.
    var bookmarks: Bookmarks? {
        if case .ready(_, let bookmarks) = engineState { bookmarks } else { nil }
    }

    /// The included rows, in stacking order.
    var includedRows: [ListRow] { rows.filter(\.included) }

    /// Whether a shot has both RAW and JPEG, so the pair source matters.
    var hasPairs: Bool { rows.contains(where: \.isPair) }

    var engineDown: UserText {
        if case .failed(let message) = engineState {
            // The engine's words lead with what didn't start.
            .verbatim(message)
        } else {
            .app(LocalizedStringResource(
                "Deptheos is starting its GPU engine.",
                comment: "Why Stack is off while the engine starts, under the Stack button."))
        }
    }

    /// Work that would replace the picture on screen is under way, so the
    /// history, Export, Retouch, Revert and Stack wait. What a render of
    /// this stack loading needn't hold up asks `held` instead.
    var busy: Bool { held || opening != nil }

    /// What holds the stack on screen. Not a render of this stack loading
    /// (`Opening.Target.result`): the frames and settings stay editable,
    /// and another tile can be tapped.
    var held: Bool {
        if libraryHeld { return true }
        guard let opening else { return false }
        if case .result = opening.target { return false }
        return true
    }

    /// Not before the engine's first check of the list has answered.
    var canStack: Bool {
        guard !busy, engine != nil, case .some(nil) = stackCheck else { return false }
        return true
    }

    /// What grays the Library's commands out. Not something opening, which
    /// only blocks touches, so a tile tapped doesn't dim it.
    var libraryHeld: Bool { !running.isIdle || adding.waits || sampleCopy != .idle }

    /// None until the engine's first check of the list has answered.
    var stackHint: UserText? {
        if engine == nil { return engineDown }
        return stackCheck.flatMap { $0 }.map(UserText.verbatim)
    }

    /// The engine's check of the list for Stack, asked on the pool: `nil`
    /// until it answers, then the problem, if any.
    var stackCheck: String?? {
        stackProblemMemo.answer(for: stackProblemKey(rows)) { key in
            do {
                return .some(try await Self.stackProblem(key))
            } catch {
                Self.unanswered("stack check", error)
                return nil
            }
        }
    }

    /// Why the included frames of `considered` can't be stacked, by the
    /// engine's checks, asked now: for Stack, which mustn't go by an
    /// answer standing in.
    func stackProblem(_ considered: [ListRow]) async -> UserText? {
        if engine == nil { return engineDown }
        do {
            return try await Self.stackProblem(stackProblemKey(considered)).map(UserText.verbatim)
        } catch {
            return Self.message(error)
        }
    }

    private func stackProblemKey(_ considered: [ListRow]) -> StackProblemKey {
        let stacked = considered.filter(\.included)
        return StackProblemKey(
            listed: considered.count,
            files: stacked.map(\.file),
            missing: stacked.filter { missing.contains($0.file) }.count,
            decoder: stackSettings.rawDecoder)
    }

    private nonisolated static func stackProblem(_ key: StackProblemKey) async throws -> String? {
        try await Request().run {
            try await DeptheosEngine.stackProblem(
                request: $0, listed: UInt32(key.listed), included: key.files,
                missing: UInt32(key.missing), rawDecoder: key.decoder, words: AppWords.words)
        }
    }

    /// Whether the list has camera RAW other than DNG (the engine's rule):
    /// not until the engine answers.
    var offersRawDecoder: Bool {
        let answer = choosesDecoderMemo.answer(for: rows.map(\.file)) { files in
            do {
                return try await Request().run { try await choosesRawDecoder(request: $0, paths: files) }
            } catch {
                Self.unanswered("raw decoder offered", error)
                return nil
            }
        }
        return answer ?? false
    }

    /// An `EngineMemo` ask with no answer: logged unless canceled.
    nonisolated static func unanswered(_ what: String, _ error: Error) {
        if case EngineError.Canceled = error { return }
        if error is CancellationError { return }
        appLog.error("\(what, privacy: .public): \(message(error).string, privacy: .public)")
    }

    // MARK: What is running

    /// The one place `running` changes, logged. A batch keeps the screen
    /// awake from Start to its summary; a stack alone holds it in
    /// `runStack`.
    func setRunning(_ new: Running) {
        let old = running.name
        running = new
        if old != new.name { stateLog.debug("running: \(old, privacy: .public) → \(new.name, privacy: .public)") }
        keepAwake(.batch, new.batch != nil)
    }

    /// Only with none running.
    func startStacking(_ stack: StackingState) {
        switch running {
        case .idle: setRunning(.stacking(stack))
        case .batch(let run, nil): setRunning(.batch(run, stack: stack))
        case .stacking, .batch(_, _?): assertionFailure("a stack started while one runs")
        }
    }

    /// A batch, if any, goes on.
    func endStacking() {
        switch running {
        case .idle, .batch(_, nil): break
        case .stacking: setRunning(.idle)
        case .batch(let run, _?): setRunning(.batch(run, stack: nil))
        }
    }

    /// Nothing when none runs.
    func updateStacking(_ change: (inout StackingState) -> Void) {
        switch running {
        case .idle, .batch(_, nil): break
        case .stacking(var stack):
            change(&stack)
            setRunning(.stacking(stack))
        case .batch(let run, var stack?):
            change(&stack)
            setRunning(.batch(run, stack: stack))
        }
    }

    /// Nothing when none runs.
    func updateBatch(_ change: (inout BatchRun) -> Void) {
        guard case .batch(var run, let stack) = running else { return }
        change(&run)
        setRunning(.batch(run, stack: stack))
    }

    // MARK: Engine

    /// Start the engine and load the kept list, once however many windows
    /// ask.
    func start() async {
        switch engineState {
        case .idle:
            let task = Task { await startOnce() }
            engineState = .starting(task)
            await task.value
        case .starting(let task):
            await task.value
        case .ready, .failed:
            return
        }
    }

    private func startOnce() async {
        do {
            await AppLog.install()
            try? await Blocking.run(.background) { AppFiles.sweepTemporaryFolders() }
            let directory = try AppFiles.historyDirectory()
            let container = try AppFiles.containerDirectory()
            let engine = try await Engine(
                decoder: AppleFrameDecoder(), historyDirectory: directory.path,
                appDirectory: container.path, language: AppWords.language)
            engineState = .ready(engine, Bookmarks())
            try? await Blocking.run(.background) { AppFiles.leaveRecentlyDeletedOutOfBackup(directory) }
            memoryPressure = MemoryPressure { [weak self] level in self?.shed(level) }
            startPaint(engine)
            // Lasting: a launch the app leaves at once still reads its list.
            let saved = try await Request.lasting().run { try await engine.readWorking(request: $0) }
            (stackID, base) = (saved.stack, saved.base)
            if let kept = saved.settings { stackSettings.use(kept) }
            setList(saved.list, save: false)
            await history.refresh()
            // Off the launch's way, so a slow share holds up nothing.
            Task { await findRootsAgain() }
            selectedFile = firstFrame
            await loadSourcePreview()
            #if DEBUG
            await openTestFrames()
            await probeLocalTone()
            #endif
        } catch {
            engineState = .failed(Self.message(error).string)
        }
    }

    // MARK: The list

    /// Show `list` and, with `save`, keep it for the next launch.
    func setList(_ list: ListInfo, save: Bool = true) {
        self.list = list
        history.shownFailure = nil
        rows = listRows(list: list)
        let files = Set(rows.map(\.file))
        // As last seen, until `checkFiles` has looked again.
        unavailable = unavailable.filter { files.contains($0.key) }
        for file in frameThumbs.keys where !files.contains(file) {
            setFrameThumb(file, nil)
        }
        // The selection leaves with the files, and ends with the list.
        if case .on(let selected) = frameSelecting {
            setFrameSelecting(files.isEmpty ? .off : .on(selected.intersection(files)))
        }
        if let selected = selectedFile, !files.contains(selected) {
            selectedFile = nil
            setPreviewLoad(.none)
        }
        // A frame passed on the way to the one selected, now gone.
        if let shown = sourcePreview?.file, !files.contains(shown) {
            setPreviewLoad(.none)
            requestSourcePreview()
        }
        checkFiles()
        if save { saveList() }
    }

    var working: WorkingInfo {
        WorkingInfo(stack: stackID, base: base, list: list, settings: settings)
    }

    /// New Stack: an empty stack goes on screen, with the settings as they
    /// are; the one left keeps its working version. Only this, a camera
    /// burst and a batch's stacks start a stack.
    func newStack() async {
        guard !busy else { return }
        // A stack with nothing in it is on screen already.
        let stacked = history.renders.contains { $0.stack == stackID }
        if working.list.files.isEmpty && !stacked { return }
        let fresh = newWorking(settings: working.settings)
        addedToExisting = nil
        await switchStack(to: fresh.stack, opening: .newStack(fresh.stack), fallback: fresh)
    }

    func newWorking(_ files: [String] = [], settings: StackSettings?) -> WorkingInfo {
        WorkingInfo(
            stack: newStackId(), base: nil,
            list: ListInfo(files: files, excluded: [], source: list.source), settings: settings)
    }

    /// Keep the working version for the next launch: the store writes the
    /// newest once it stands, and at once as the app leaves.
    func saveList() {
        guard let engine else { return }
        let working = working
        do {
            try engine.keepWorking(working: working)
        } catch {
            captureLog.error("could not save the list: \(error.localizedDescription, privacy: .public)")
        }
    }

    /// Use RAW / Use JPEG for the shots that have both.
    func setPairSource(_ source: Source) async {
        guard !held, source != list.source else { return }
        let shown = rows.first { $0.file == selectedFile }
        setList(listSetSource(list: list, source: source))
        if let shown, let now = rows.first(where: { $0.stem == shown.stem && $0.files == shown.files }) {
            selectedFile = now.file
        }
        await loadSourcePreview()
    }

    /// The folder of the first missing image: what Locate… asks for.
    var firstMissingFolder: String? {
        rows.first { missing.contains($0.file) }.map { ($0.file as NSString).deletingLastPathComponent }
    }

    func select(_ file: String) async {
        // While retouching, a frame tapped is what the brush paints from.
        if paint.active, paint.frames.contains(file) {
            selectedFile = file
            paint.pick(file)
            return
        }
        selectedFile = file
        // Over a depth map that can be lit, a frame tapped is lit; while
        // comparing, it is compared with; else it is shown alone.
        view(.showFrame(frame: pick(file), putOut: false))
        if showsFrameAlone { await loadSourcePreview() }
    }

    /// The frame number of `file` in the list.
    func frameNumber(of file: String?) -> Int? {
        rows.firstIndex { $0.file == file }.map { $0 + 1 }
    }

    /// For the navigation bar; empty when no render is shown.
    var shownTitle: String {
        history.title(of: result?.renderID) ?? ""
    }

    /// Show `error`; a canceled call (its task went away) is no failure.
    func report(_ error: Error) {
        if case EngineError.Canceled = error { return }
        if error is CancellationError { return }
        errorMessage = ErrorMessage(Self.message(error))
    }

    /// A load still wanted was canceled by leaving the foreground: `again`
    /// asks for it once back, or at once if back already.
    func stopped(_ again: @escaping @MainActor () -> Void) {
        if UIApplication.shared.applicationState == .background {
            stoppedLoads.append(again)
        } else {
            Task { again() }
        }
    }

    /// Back in front: what leaving stopped is asked again.
    func askStoppedAgain() {
        let again = stoppedLoads
        stoppedLoads = []
        if !again.isEmpty { stateLog.debug("foreground: \(again.count) stopped loads asked again") }
        for ask in again { ask() }
        for loads in [previewLoads, fullFrameLoads, alignedLoads, resultPictures, mapPictures] {
            loads.askAgain()
        }
    }

    /// `call` under `request`, canceled with this task: its value, or `nil`
    /// with no engine or on failure, which is shown.
    func ask<T>(under request: Request = Request(), _ call: (Engine, Request) async throws -> T) async -> T? {
        guard let engine else { return nil }
        do {
            return try await request.run { try await call(engine, $0) }
        } catch {
            report(error)
            return nil
        }
    }

    /// An error as sentences: the engine's worded by the engine, the
    /// system's by the system.
    nonisolated static func message(_ error: Error) -> UserText {
        switch error as? EngineError {
        case .Failed(let message): .verbatim(message)
        case .Canceled:
            .app(LocalizedStringResource(
                "Canceled.", comment: "An alert's text: the work was stopped with Cancel."))
        case nil: .verbatim(error.localizedDescription)
        }
    }
}

extension Request {
    /// `call` with this request, canceled with the task awaiting it: a
    /// Swift cancellation doesn't reach the engine on its own (ADR-143).
    /// Nonisolated, it runs on its caller's actor.
    nonisolated func run<T>(_ call: (Request) async throws -> T) async throws -> T {
        try await withTaskCancellationHandler {
            try await call(self)
        } onCancel: {
            self.cancel()
        }
    }
}

/// The error alert: `title` says what happened ("The stack didn't finish"),
/// the app's name otherwise; `text` says why and what to do.
struct ErrorMessage: Equatable {
    var title = UserText.verbatim("Deptheos")
    var text: UserText

    init(_ text: UserText) {
        self.text = text
    }

    init(title: LocalizedStringResource, text: UserText) {
        self.title = .app(title)
        self.text = text
    }
}

/// The compact window's sheet over the photo.
extension AppModel {
    /// New Stack, from the Library or the menu bar.
    func startNewStack() async {
        await newStack()
        slideIn(then: landingPanel)
    }

    /// Add Folder…, Add from Photos… or Add from Files… in the empty
    /// Library: New Stack, then `add` fills it.
    func addToNewStack(_ add: () async -> Void) async {
        guard !busy else { return }
        await startNewStack()
        await add()
    }

    /// The one place `result` changes. A change of render is logged with
    /// how the working version stands against it.
    func setResult(_ shown: ShownResult?) {
        let before = result?.renderID
        result = shown
        if shown?.renderID != before {
            appLog.debug("shown: \(self.showing.log, privacy: .public)")
        }
        fitPictures()
    }

    /// The one place `frameSelecting` changes.
    func setFrameSelecting(_ new: FrameSelecting) {
        guard new != frameSelecting else { return }
        if new.isOn != frameSelecting.isOn { stateLog.debug("selecting frames: \(new.isOn ? "on" : "off", privacy: .public)") }
        frameSelecting = new
    }

    /// Open, change or close the compact window's sheet. A sheet opened
    /// shows the controls again (`photoOnly` is a state of its own).
    func setPanel(_ panel: Panel) {
        guard panel != self.panel else { return }
        appLog.debug("panel: \(String(describing: panel), privacy: .public)")
        self.panel = panel
    }

    /// The one place `viewState` changes, logged.
    func setView(_ view: ViewState, contribution: ContributionToggle?) {
        // What the view lined up goes with it, the lit picture with the map.
        if view.linesUp != viewState.linesUp { alignedLoad = .none }
        if view.light == nil { litMap = .whole }
        viewState = view
        contributionToggle = contribution
        requestSourcePreview()
        // The frame at full size only while it is shown alone, or may be.
        if needsSourcePreview { requestFullFrame() } else { dropFullFrame() }
        // Only the pictures on screen are held.
        fitPictures()
    }

    /// The one place `screen` changes. The Library closes the sheet.
    func setScreen(_ new: AppScreen) {
        if screen != new { appLog.debug("screen: \(String(describing: new), privacy: .public)") }
        screen = new
        if new == .library { setPanel(.closed) }
    }

    /// The one place `opening` changes, logged.
    func setOpening(_ new: Opening?) {
        opening = new
        stateLog.debug("opening: \(new.map { String(describing: $0.target) } ?? "none", privacy: .public)")
    }

    /// The one place `adding` changes, logged.
    func setAdding(_ new: Adding) {
        adding = new
        appLog.debug("adding: \(self.adding.name, privacy: .public)")
    }

    /// The one place `deleteUndo` changes, logged.
    func setDeleteUndo(_ new: DeleteUndo) {
        guard new != deleteUndo else { return }
        stateLog.debug("delete undo: \(self.deleteUndo.name, privacy: .public) → \(new.name, privacy: .public)")
        deleteUndo = new
    }

    /// The one place `sampleCopy` changes.
    func setSampleCopy(_ state: SampleCopy) {
        guard state != sampleCopy else { return }
        stateLog.debug("sample: \(self.sampleCopy.name, privacy: .public) → \(state.name, privacy: .public)")
        sampleCopy = state
    }

    /// The one place `presented` changes. Export and the camera close the
    /// sheet first; Render Info at compact width is pushed onto it
    /// instead (`StackingView`).
    func present(_ new: Presented) {
        guard new != presented else { return }
        stateLog.debug("presented: \(self.presented.name, privacy: .public) → \(new.name, privacy: .public)")
        presented = new
        switch new {
        case .export, .camera: setPanel(.closed)
        case .none, .renderInfo, .revert: break
        }
    }

    /// `shown` closed, if it is still the one up.
    func dismiss(_ shown: Presented) {
        if presented == shown { present(.none) }
    }

    /// The bar's Frames, Stack Settings or Brush button: closes its sheet
    /// if open, else shows it, at the open sheet's height if any.
    func togglePanel(_ side: Panel.Side) {
        if panel.side == side {
            setPanel(.closed)
        } else {
            setPanel(.open(side, at: panel.height ?? .low))
        }
    }

    /// A tap on the photo hides or shows its controls; never under
    /// VoiceOver, which would leave nothing to find, or with a sheet open.
    func toggleControls(voiceOver: Bool) {
        switch panel {
        case .closed where !voiceOver: setPanel(.photoOnly)
        case .photoOnly: setPanel(.closed)
        case .closed, .sources, .settings: break
        }
    }

    /// The controls over the photo show again, if a tap hid them.
    func showControls() {
        if panel == .photoOnly { setPanel(.closed) }
    }

    /// Something to look at was picked: a tall sheet drops low.
    func revealViewer() {
        if panel.height == .tall { setPanel(panel.at(.low)) }
    }

    /// The sheet a stack opens with on iPhone: its frames until first
    /// stacked (the whole screen while there are none), else none.
    var landingPanel: Panel {
        if baseRender != nil { return .closed }
        return .sources(.frames, rows.isEmpty ? .tall : .low)
    }
}

/// Selecting frames in the Frames list. The selected leave as they leave
/// the list (`setList`).
///
/// Lifecycle: off, or on with the files selected. Changed only by
/// `AppModel.setFrameSelecting`.
///
///     off ──Select──▶ on([]) ──rows tapped──▶ on(files)
///     on ──Done, Stack These, the Renders side, the list emptied, another stack opened──▶ off
enum FrameSelecting: Equatable {
    case off
    case on(Set<String>)

    var isOn: Bool { self != .off }

    /// The files selected; none while off.
    var selected: Set<String> {
        if case .on(let files) = self { files } else { [] }
    }
}

/// What `AppModel.stackProblem` depends on.
nonisolated struct StackProblemKey: Equatable {
    let listed: Int
    let files: [String]
    let missing: Int
    let decoder: RawBackend
}
