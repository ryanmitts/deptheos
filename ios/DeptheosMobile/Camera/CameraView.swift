@preconcurrency import AVFoundation
import AVKit
import SwiftUI
import UIKit
import os

/// Each lifecycle transition, one line.
private let stateLog = AppLog(category: "state")

/// Full-screen camera. Sweep: hold the shutter while moving toward the
/// subject, let go to stack. Bracket: tap, and the lens steps out from
/// closest focus, a photo a step. A burst becomes a new folder and list,
/// stacked as soon as its last photo is saved. Focus peaking marks what is
/// sharp.
struct CameraView: View {
    /// Offered in the Library, the frames list, the sources' bar and the
    /// menu bar.
    static let takeBurst = LocalizedStringResource("Take Burst", comment: "Opens the camera to take a burst.")
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) var dismiss
    @Environment(\.openURL) private var openURL
    @State var camera = CameraController()
    /// Changed only by `setPhase`.
    @State private(set) var phase = Phase.starting
    /// Each touch presses once, however far it moves, and lets go when it
    /// ends or is canceled.
    @GestureState var touching = false
    /// As the controller last said (`CameraController.SessionState`).
    @State var session = CameraController.SessionState.stopped
    /// Off each time the camera opens.
    @State var torchOn = false
    /// A subject a hand's width away burns out at full.
    @AppStorage("CameraTorchLevel") var torchLevel = 1.0
    static let torchLevels = [0.25, 0.5, 1.0]
    /// Seconds a bracket waits for the phone to stop shaking from the
    /// touch; 0 starts at once.
    @AppStorage("BracketDelay") var bracketDelay = 2
    static let delays = [0, 2, 5]
    /// Frames a second; 0 is as fast as the camera can.
    @AppStorage("BurstFramesPerSecond") var framesPerSecond = 10
    static let rates = [5, 10, 15, 0]
    @AppStorage("CameraMode") var mode = CaptureMode.sweep
    @AppStorage("BracketFrames") var bracketFrames = 10
    static let bracketCounts = [5, 10, 15, 20]
    /// Lens travel between bracket frames (0 closest to 1 farthest).
    @AppStorage("BracketStep") var bracketStep = 0.02
    static let stepRange = 0.0025...0.05
    /// A bracket's first focus (0 is closest): the lens rests there in
    /// Bracket, so what is composed on is the first frame.
    @AppStorage("BracketNear") var bracketNear = 0.0
    static let nearRange = 0.0...0.9
    /// The bracket end a dragged slider shows through the lens.
    @State var previewing: BracketEnd?
    enum BracketEnd { case first, last }
    /// In EV; reset each time the camera opens.
    @State var exposureBias = 0.0
    static let biases = [-2.0, -1.5, -1, -0.5, 0, 0.5, 1, 1.5, 2]
    @State var metering = Metering.whole
    enum Metering: Equatable {
        case whole
        /// In the preview's coordinates; `tap` tells one tap from the next.
        case point(CGPoint, tap: Int)
    }
    /// As the camera last said.
    @State var exposure: CameraController.Exposure?
    /// A `LensChoice.id`; empty leaves it to the camera.
    @AppStorage("CameraLens") var lensID = ""
    /// As the camera last said: the preview follows it.
    @State var lens: CameraController.LensChoice?
    @AppStorage("PhotoResolution") var resolution = PhotoResolution.twelve
    /// RAW where the camera has it, unless chosen otherwise: its frames
    /// differ only in focus, where each HEIC is toned and sharpened alone
    /// (ADR-162).
    @AppStorage("PhotoFormat") var format = PhotoFormat.raw
    @AppStorage("CaptureAlgorithm") var algorithm = AlgorithmKind.pyramid
    @AppStorage("CameraPeaking") var peakingOn = true

    /// Where the camera screen is.
    ///
    /// Lifecycle: starting, ready, counting (a bracket's delay), capturing,
    /// saving, canceling, partial (stack what there is, or discard it),
    /// declined (why) or failed. Changed only by `setPhase`.
    ///
    ///     starting ──started──▶ ready
    ///     ready ──shutter (a bracket with a delay)──▶ counting ──the delay ran out──▶ capturing
    ///     ready ──shutter──▶ capturing ──let go, or it ended──▶ saving
    ///     counting ──shutter again──▶ ready
    ///     capturing ──shutter again (a bracket)──▶ canceling ──its photos in flight landed──▶ ready
    ///     saving ──stacked──▶ (the screen closes)
    ///     saving ──not taken, or a sweep too short to stack──▶ ready
    ///     saving ──a bracket without all its steps, or any burst cut short with 2 frames or more──▶ partial ──Stack Them──▶ saving
    ///     partial ──Discard──▶ ready
    ///     saving ──not stacked (the app busy), or cut short with fewer than 2 frames──▶ declined ──OK──▶ ready
    ///     ready ──no room for the bracket──▶ declined
    ///     starting ──the camera couldn't start──▶ failed
    ///
    /// Declined is an alert over the camera, which stays open; only a
    /// camera that can't start is `failed`.
    enum Phase: Equatable {
        case starting
        case ready
        /// Seconds left before its first frame.
        case counting(left: Int)
        /// `frames` written so far.
        case capturing(frames: Int)
        /// The last frames are written, then it is stacked.
        case saving(frames: Int)
        /// Its photos in flight are landing.
        case canceling
        /// Short of its steps, or cut short (`why`) with two frames or more:
        /// stack or discard. `planned` for a bracket.
        case partial(folder: URL, taken: Int, planned: Int?, why: String?)
        /// For the reason the alert gives.
        case declined(ErrorMessage)
        case failed(String)

        var frames: Int? {
            switch self {
            case .capturing(let frames), .saving(let frames): frames
            case .starting, .ready, .counting, .canceling, .partial, .declined, .failed: nil
            }
        }

        var declined: ErrorMessage? {
            if case .declined(let message) = self { message } else { nil }
        }

        var isCapturing: Bool {
            if case .capturing = self { true } else { false }
        }

        var isCounting: Bool {
            if case .counting = self { true } else { false }
        }

        var isPartial: Bool {
            if case .partial = self { true } else { false }
        }

        /// A burst is under way; the settings can't change under it.
        var isBusy: Bool {
            switch self {
            case .counting, .capturing, .saving, .canceling, .partial: true
            case .starting, .ready, .declined, .failed: false
            }
        }

        /// For the log.
        var name: String {
            switch self {
            case .starting: "starting"
            case .ready: "ready"
            case .counting: "counting"
            case .capturing: "capturing"
            case .saving: "saving"
            case .canceling: "canceling"
            case .partial: "partial"
            case .declined: "declined"
            case .failed: "failed"
            }
        }
    }

    /// The one place `phase` changes, logged, and said to VoiceOver where
    /// only the screen would show it.
    private func setPhase(_ next: Phase) {
        let last = phase
        phase = next
        guard last.name != next.name else { return }
        stateLog.debug("camera screen: \(last.name, privacy: .public) → \(next.name, privacy: .public)")
        switch next {
        case .saving(let frames):
            AccessibilityNotification.Announcement(String(localized: LocalizedStringResource(
                "Saving \(frames) frames",
                comment: "Camera status pill while a sweep's frames are saved. The placeholder is how many."))).post()
        case .canceling:
            AccessibilityNotification.Announcement(String(localized: LocalizedStringResource(
                "Bracket canceled", comment: "Said by VoiceOver when a bracket is canceled from the shutter."))).post()
        case .failed:
            // Nothing to show or shoot with: the camera's light goes out.
            camera.stop()
        case .starting, .ready, .counting, .capturing, .partial, .declined: break
        }
    }

    /// As last measured inside the safe area.
    @State private var shape = Shape()
    @Environment(\.verticalSizeClass) private var verticalSizeClass
    @Environment(\.horizontalSizeClass) private var horizontalSizeClass
    @Environment(\.accessibilityReduceMotion) var reduceMotion
    /// Haptic triggers for `.sensoryFeedback`.
    @State private var lightTaps = 0
    @State private var mediumTaps = 0
    @State private var successTaps = 0
    @State private var warningTaps = 0
    @State private var errorTaps = 0
    /// The viewfinder's place, in screen coordinates. The one preview is
    /// put there, so a layout change moves it rather than remaking it.
    @State private var viewfinderFrame = CGRect.zero

    /// Width over height as the preview shows it, once said: the picture
    /// can run across a screen (the Duo's inner display) as well as along.
    @State private var pictureAspect: CGFloat?

    private var layout: ScreenLayout {
        .of(
            shape, picture: pictureAspect,
            phoneUpright: horizontalSizeClass == .compact && verticalSizeClass == .regular)
    }

    var body: some View {
        ZStack {
            Color.black.ignoresSafeArea()
            if phase != .starting, !isFailed {
                // A bracket blinks at each frame; a sweep would flash many
                // times a second, so its count says it instead.
                CameraPreview(
                    camera: camera, peaking: peakingOn, blinks: mode == .bracket, lens: lens?.id,
                    metered: meteredPoint
                ) {
                    pictureAspect = $0
                }
                    .frame(width: viewfinderFrame.width, height: viewfinderFrame.height)
                    .position(x: viewfinderFrame.midX, y: viewfinderFrame.midY)
                    .transition(.opacity)
            }
            if case .failed(let message) = phase {
                VStack {
                    topBar
                        .dynamicTypeSize(...DynamicTypeSize.accessibility1)
                    Spacer()
                    unavailable(message)
                    Spacer()
                }
                .padding()
            } else {
                // Over the photo: it grows with text only so far. Exactly
                // the screen, so controls that don't fit beside a fold
                // can't widen what `shape` measures (ADR-169).
                controls
                    .dynamicTypeSize(...DynamicTypeSize.accessibility1)
                    .frame(minWidth: 0, maxWidth: .infinity, minHeight: 0, maxHeight: .infinity)
            }
        }
        .coordinateSpace(.named(Self.screen))
        .background {
            Color.clear
                .measure("camera shape", for: Shape.self) { proxy in
                    Shape(size: proxy.size, insets: proxy.safeAreaInsets, fold: Self.fold(in: proxy)).rounded
                } action: { shape = $0 }
        }
        // Dark glass and light symbols over the picture in either mode.
        .environment(\.colorScheme, .dark)
        .statusBarHidden()
        .persistentSystemOverlays(.hidden)
        .animation(reduceMotion ? nil : .easeOut(duration: 0.2), value: phase == .starting)
        .sensoryFeedback(.impact(weight: .light), trigger: lightTaps)
        .sensoryFeedback(.impact(weight: .medium), trigger: mediumTaps)
        .sensoryFeedback(.success, trigger: successTaps)
        .sensoryFeedback(.warning, trigger: warningTaps)
        .sensoryFeedback(.error, trigger: errorTaps)
        .sensoryFeedback(.selection, trigger: mode)
        .sensoryFeedback(.impact(weight: .light), trigger: peakingOn)
        .sensoryFeedback(.impact(weight: .light), trigger: torchOn)
        .onChange(of: resolution) { camera.setResolution(resolution) }
        .onChange(of: format) { camera.setFormat(format) }
        // A bracket's photos are taken as asked, never from just before
        // (zero shutter lag off); told ahead, since the change takes time.
        .onChange(of: mode, initial: true) { camera.setBracketMode(mode == .bracket) }
        // Bracket rests at the first frame's focus; a sweep is taken at
        // closest focus, moving in.
        .onChange(of: restLens, initial: true) { camera.setRestLens(restLens) }
        .onChange(of: exposureBias) { camera.setExposureBias(Float(exposureBias)) }
        .onChange(of: layout.name, initial: true) { _, name in
            stateLog.debug("camera layout: \(name, privacy: .public)")
        }
        // Camera Control and the volume buttons work the shutter as a touch
        // does. The photos make their own sound.
        .onCameraCaptureEvent(
            isEnabled: phase == .ready || phase.isCapturing || phase.isCounting, defaultSoundDisabled: true
        ) { event in
            switch event.phase {
            case .began: press()
            case .ended, .cancelled: release()
            @unknown default: break
            }
        }
        .task { await start() }
        .task(id: phase.isCounting) { await countDown() }
        // The screen stays awake while it is composed on, as on a tripod.
        .onAppear { model.keepAwake(.camera, true) }
        .onDisappear {
            model.keepAwake(.camera, false)
            camera.onFrameCount = nil
            camera.onSessionChange = nil
            camera.onExposure = nil
            camera.onLensChange = nil
            camera.onShutter = nil
            camera.peaking = nil
            camera.rotation = nil
            camera.stop()
        }
        // Watched here, not on the shutter: a layout change under the
        // finger remakes the shutter, and the touch still lets go.
        .onChange(of: touching) { _, touching in
            if touching { press() } else { release() }
        }
        .alert(
            Text(phase.declined?.title ?? .verbatim("Deptheos")),
            isPresented: Binding(
                get: { phase.declined != nil },
                set: { if !$0, phase.declined != nil { setPhase(.ready) } }),
            presenting: phase.declined
        ) { _ in
            Button(LocalizedStringResource("OK", comment: "Button that closes an error alert.")) {}
        } message: { message in
            Text(message.text)
        }
        .alert(
            partialTitle,
            isPresented: Binding(get: { phase.isPartial }, set: { _ in })
        ) {
            Button(LocalizedStringResource(
                "Stack Them", comment: "Button of the camera's Cut Short alert: stacks the frames that were taken.")
            ) {
                if case .partial(let folder, let taken, _, _) = phase {
                    setPhase(.saving(frames: taken))
                    Task { await open(folder) }
                }
            }
            Button(
                LocalizedStringResource(
                    "Discard", comment: "Button of the camera's Cut Short alert: deletes the frames that were taken."),
                role: .destructive
            ) {
                if case .partial(let folder, _, _, _) = phase {
                    model.discardCapture(folder)
                    setPhase(.ready)
                }
            }
        } message: {
            if case .partial(_, let taken, let planned, let why) = phase {
                Text(Self.partialMessage(taken: taken, planned: planned, why: why))
            }
        }
    }

    var isFailed: Bool {
        if case .failed = phase { true } else { false }
    }

    /// A bracket's or a sweep's.
    private var partialTitle: Text {
        if case .partial(_, _, nil, _) = phase {
            Text("Sweep Cut Short", comment: "Title of the camera's alert when a sweep stopped before it was let go, with frames to stack.")
        } else {
            Text("Bracket Cut Short", comment: "Title of the camera's alert when a bracket ended without all its frames.")
        }
    }

    /// Why it stopped, if something cut it short, then what was taken.
    private static func partialMessage(taken: Int, planned: Int?, why: String?) -> String {
        let taken = if let planned {
            String(localized: LocalizedStringResource(
                "\(taken) of \(planned) frames were taken, so part of the focus range is missing.",
                comment: "Message of the camera's Bracket Cut Short alert. The placeholders are the frames taken and the frames the bracket was to take."))
        } else {
            String(localized: LocalizedStringResource(
                "\(taken) frames were taken.",
                comment: "Message of the camera's Sweep Cut Short alert, after why it stopped. The placeholder is how many frames were taken."))
        }
        return [why, taken].compactMap(\.self).joined(separator: " ")
    }

    private nonisolated static let screen = "camera screen"

    /// The fold dividing the screen, in the proxy's coordinates; none
    /// before iOS 27.1.
    private nonisolated static func fold(in proxy: GeometryProxy) -> Fold? {
        if #available(iOS 27.1, *) {
            let folds = proxy.reservedRegions(kind: .division)
            return (folds.first(where: \.isActive) ?? folds.first).map {
                Fold(frame: $0.frame, active: $0.isActive)
            }
        }
        return nil
    }

    /// Why, and the way to Settings when access is missing.
    private func unavailable(_ message: String) -> some View {
        ContentUnavailableView {
            Label(
                LocalizedStringResource(
                    "Camera Unavailable", comment: "Camera screen title when the camera couldn't start or a burst failed."),
                systemImage: "camera.badge.ellipsis")
        } description: {
            // The error's own sentence, already in the app's language.
            Text(verbatim: message)
        } actions: {
            if accessDenied {
                Button(LocalizedStringResource(
                    "Open Settings",
                    comment: "Camera screen button when camera access is off: opens Deptheos's page in the Settings app.")
                ) {
                    if let url = URL(string: UIApplication.openSettingsURLString) { openURL(url) }
                }
                .buttonStyle(.glassProminent)
                .tint(.accentFill)
            }
        }
    }

    /// Where the 4:3 picture sits inside `viewfinder`, matching `.resizeAspect`.
    private var pictureRect: CGRect {
        let size = viewfinderFrame.size
        guard size.width > 0, size.height > 0, let aspect = pictureAspect, aspect > 0 else {
            return CGRect(origin: .zero, size: size)
        }
        let fitted = size.width / size.height > aspect
            ? CGSize(width: size.height * aspect, height: size.height)
            : CGSize(width: size.width, height: size.width / aspect)
        return CGRect(
            x: (size.width - fitted.width) / 2,
            y: (size.height - fitted.height) / 2,
            width: fitted.width, height: fitted.height)
    }

    /// Empty and measured, the preview put over it, showing the whole 4:3
    /// picture inside.
    private var viewfinder: some View {
        Color.clear
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .measure("camera viewfinder", for: CGRect.self) { $0.frame(in: .named(Self.screen)) } action: {
                viewfinderFrame = $0
            }
            // A tap on the picture meters exposure there.
            .contentShape(.rect)
            .onTapGesture { point in
                guard phase == .ready, pictureRect.contains(point) else { return }
                let tap = if case .point(_, let last) = metering { last + 1 } else { 1 }
                metering = .point(point, tap: tap)
                lightTaps += 1
            }
            .accessibilityHidden(true)
    }

    /// Around the viewfinder's place, as `layout` says.
    @ViewBuilder private var controls: some View {
        switch layout {
        case .overlaid(pinned: nil):
            // An upright iPhone: the picture the same size in every mode, the
            // bars over it.
            ZStack {
                viewfinder
                    .ignoresSafeArea()
                bars
                    .padding(ScreenLayout.edgePadding)
            }
        case .overlaid(pinned: let height?):
            ZStack(alignment: .top) {
                viewfinder
                    .frame(height: height)
                bars
                    .padding(ScreenLayout.edgePadding)
            }
        case .below(let picture):
            // The controls keep their height; the picture gives way if larger
            // text makes them taller.
            VStack(spacing: 0) {
                viewfinder
                    .frame(maxHeight: picture)
                    .overlay(alignment: .top) {
                        topBar
                            .padding([.horizontal, .top])
                    }
                    .layoutPriority(1)
                Spacer(minLength: 12)
                bottomBar
                    .padding([.horizontal, .bottom])
                    .fixedSize(horizontal: false, vertical: true)
                    .layoutPriority(2)
            }
        case .beside(let column, let tight):
            HStack(spacing: ScreenLayout.edgePadding) {
                viewfinder
                VStack(spacing: 10) {
                    topBar
                    Spacer(minLength: 0)
                    if tight { columnControls } else { bottomBar }
                }
                .frame(width: column)
                .padding([.vertical, .trailing], ScreenLayout.edgePadding)
            }
        case .book(let leading, let trailing):
            HStack(spacing: 0) {
                viewfinder
                    .frame(width: leading)
                Color.clear
                    .frame(width: trailing - leading)
                VStack(spacing: 12) {
                    topBar
                    Spacer(minLength: 0)
                    bottomBar
                }
                .padding()
                .frame(maxWidth: .infinity)
            }
        case .table(let top, let bottom):
            VStack(spacing: 0) {
                viewfinder
                    .frame(height: top)
                Color.clear
                    .frame(height: bottom - top)
                VStack(spacing: 12) {
                    topBar
                    Spacer(minLength: 0)
                    bottomBar
                }
                .padding()
                .frame(maxHeight: .infinity)
            }
        }
    }

    private var bars: some View {
        VStack {
            topBar
            Spacer()
            bottomBar
        }
    }

    /// Settings is where camera access changes.
    private var accessDenied: Bool {
        switch AVCaptureDevice.authorizationStatus(for: .video) {
        case .denied, .restricted: true
        default: false
        }
    }

    private func start() async {
        camera.onFrameCount = { count in
            switch phase {
            case .capturing: setPhase(.capturing(frames: count))
            case .saving: setPhase(.saving(frames: count))
            case .starting, .ready, .counting, .canceling, .partial, .declined, .failed: break
            }
        }
        camera.onSessionChange = { state in
            session = state
            // A pause or a stop turns the light off with the camera.
            if state != .running { torchOn = false }
            // And ends a bracket's countdown: the camera couldn't shoot at
            // its end.
            if state != .running, case .counting = phase { setPhase(.ready) }
        }
        camera.onExposure = { exposure = $0 }
        camera.onLensChange = {
            // Its bias range, torch and focus range are its own.
            lens = camera.lens
            torchOn = false
            exposureBias = min(max(exposureBias, biasRange.lowerBound), biasRange.upperBound)
        }
        camera.setResolution(resolution)
        camera.setFormat(format)
        camera.preferredLens = lensID.isEmpty ? nil : lensID
        do {
            try await camera.start()
            // The screen was closed while the camera started.
            guard !Task.isCancelled else { return }
            lens = camera.lens
            setPhase(.ready)
            // Automation: `-AutoBurst <seconds>` holds the shutter that long.
            let seconds = Automation.number("AutoBurst")
            if seconds > 0 {
                try? await Task.sleep(for: .seconds(1))
                press()
                try? await Task.sleep(for: .seconds(seconds))
                release()
            }
        } catch {
            guard !Task.isCancelled else { return }
            setPhase(.failed(error.localizedDescription))
        }
    }

    /// By a touch, Camera Control or a volume button: starts a burst, or
    /// cancels the bracket counting down or under way.
    func press() {
        switch phase {
        case .counting:
            lightTaps += 1
            setPhase(.ready)
            return
        case .capturing where mode == .bracket:
            lightTaps += 1
            setPhase(.canceling)
            camera.cancelBurst()
            return
        default: break
        }
        guard canShoot, mode == .sweep || camera.canBracket else { return }
        if let short = shortOfSpace() {
            setPhase(.declined(short))
            return
        }
        if mode == .bracket, bracketDelay > 0 {
            lightTaps += 1
            setPhase(.counting(left: bracketDelay))
        } else {
            // A hand-held sweep wants the touch answered; a bracket gets none,
            // which would shake frame 1.
            if mode == .sweep { mediumTaps += 1 }
            begin()
        }
    }

    /// Ended with the phase (`task(id:)`) if the shutter cancels it.
    private func countDown() async {
        while case .counting(let left) = phase {
            guard (try? await Task.sleep(for: .seconds(1))) != nil else { return }
            guard case .counting = phase else { return }
            if left > 1 { setPhase(.counting(left: left - 1)) } else { begin() }
        }
    }

    /// None while the camera isn't running: the burst would wait on a
    /// paused session with nothing to end it.
    private func begin() {
        guard session == .running else {
            setPhase(.ready)
            return
        }
        let folder: URL
        do {
            folder = try model.newCaptureFolder()
        } catch {
            setPhase(.declined(Self.notSaved(error)))
            return
        }
        setPhase(.capturing(frames: 0))
        camera.framesPerSecond = framesPerSecond > 0 ? Double(framesPerSecond) : nil
        let bracket = mode == .bracket ? bracketPositions : nil
        camera.beginBurst(into: folder, bracket: bracket) { result in
            Task { await finished(result, folder: folder) }
        }
    }

    /// The room a bracket needs against what is free; nil when it fits or
    /// is unknown. A sweep isn't counted: it stops cleanly below
    /// `CameraController.spaceReserve`.
    private func shortOfSpace() -> ErrorMessage? {
        guard mode == .bracket, let free = AppFiles.freeSpace() else { return nil }
        let megabytes: Int64 =
            switch (shownFormat, shownResolution) {
            case (.raw, _): 26
            case (.heic, .twelve): 4
            case (.heic, _): 12
            }
        let frames = bracketPositions.count
        // With the reserve a burst leaves, past which it would stop.
        let need = Int64(frames) * megabytes * 1_000_000 + CameraController.spaceReserve
        guard free < need else { return nil }
        return ErrorMessage(
            title: LocalizedStringResource(
                "Not Enough Space", comment: "Title of the camera's alert when the device has too little room for a burst."),
            text: .app(LocalizedStringResource(
                "This bracket needs about \(need.formatted(.byteCount(style: .file))) and \(free.formatted(.byteCount(style: .file))) is free.",
                comment: "Message of the camera's Not Enough Space alert. The placeholders are sizes with their units ('260 MB').")))
    }

    private static func notSaved(_ error: Error) -> ErrorMessage {
        ErrorMessage(
            title: LocalizedStringResource(
                "Burst Not Saved", comment: "Title of the camera's alert when a burst failed and nothing was stacked."),
            text: .verbatim(error.localizedDescription))
    }

    func release() {
        // A bracket runs its course; letting go does not stop it.
        guard case .capturing(let frames) = phase, mode == .sweep else { return }
        setPhase(.saving(frames: frames))
        lightTaps += 1
        camera.endBurst()
    }

    private func finished(_ result: Result<CameraController.BurstOutcome, Error>, folder: URL) async {
        let canceled = phase == .canceling
        let frames = phase.frames ?? 0
        switch result {
        case .success where canceled:
            // It ended as it was canceled.
            model.discardCapture(folder)
            setPhase(.ready)
        case .success(.partial(let taken, let planned, let why)):
            warningTaps += 1
            setPhase(.partial(folder: folder, taken: taken, planned: planned, why: why))
        case .success(.complete) where mode == .sweep && Self.files(in: folder) < 2:
            // A tap where a hold was meant: nothing to stack; the hint says
            // how.
            model.discardCapture(folder)
            setPhase(.ready)
        case .success(.complete):
            // A burst that ended by itself says so; one let go already has.
            if phase.isCapturing { successTaps += 1 }
            setPhase(.saving(frames: frames))
            await open(folder)
        case .failure(let error)
            where error is CameraController.BurstInterrupted || error is CameraController.BurstBusy
            || error is CameraController.BurstCanceled:
            // Not taken (paused, busy) or canceled; it shoots again once it
            // can.
            model.discardCapture(folder)
            setPhase(.ready)
        case .failure(let error):
            // Too few frames to stack: say why, and shoot again.
            model.discardCapture(folder)
            errorTaps += 1
            setPhase(.declined(Self.notSaved(error)))
        }
    }

    private static func files(in folder: URL) -> Int {
        (try? FileManager.default.contentsOfDirectory(atPath: folder.path).count) ?? 0
    }

    /// The screen closes on the new stack.
    private func open(_ folder: URL) async {
        switch await model.openCapture(folder) {
        case .opened:
            // The model outlives this screen: the stack runs in the viewer,
            // keeping the screen awake itself; the camera lets go as it
            // closes.
            let (model, algorithm) = (model, algorithm)
            Task { await model.stackCapture(algorithm: algorithm) }
            dismiss()
        case .declined(let message):
            setPhase(.declined(message))
        case .failed:
            setPhase(.ready)
        }
    }
}

/// The raw value is what `@AppStorage` keeps, so it never changes.
enum CaptureMode: String, CaseIterable, Identifiable {
    case sweep
    case bracket
    var id: Self { self }
    var label: LocalizedStringResource {
        switch self {
        case .sweep:
            LocalizedStringResource(
                "Sweep", comment: "A camera mode: hold the shutter while moving toward the subject. A name, capitalized.")
        case .bracket:
            LocalizedStringResource(
                "Bracket", comment: "A camera mode: the phone stays still and the lens steps its focus. A name, capitalized.")
        }
    }
}
