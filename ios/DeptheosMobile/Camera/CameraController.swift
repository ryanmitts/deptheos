@preconcurrency import AVFoundation
import Foundation
import os

private nonisolated let log = AppLog(category: "camera")

/// The photo size a burst is taken at, as the Camera app names it.
enum PhotoResolution: Int, CaseIterable, Identifiable, Sendable {
    case twelve = 12
    case fortyEight = 48
    var id: Self { self }
    var label: LocalizedStringResource {
        LocalizedStringResource(
            "\(rawValue) MP",
            comment: "A photo size in the camera's menu, in megapixels, as the Camera app names it. The placeholder is the number (12 or 48).")
    }
}

/// What a burst's photos are saved as. The raw value is what `@AppStorage`
/// keeps, so it never changes.
enum PhotoFormat: String, CaseIterable, Identifiable, Sendable {
    /// Apple's processed photo: sharpened, denoised and tone-mapped, 8-bit.
    case heic
    /// The sensor's Bayer data as a DNG: linear, no sharpening, denoising or
    /// tone mapping, so frames match and the focus measure sees only the
    /// lens. About 24 MB a frame, always 12 MP; exports as DNG.
    case raw
    var id: Self { self }
    var label: LocalizedStringResource {
        switch self {
        case .heic:
            LocalizedStringResource(
                "HEIC", comment: "A photo format in the camera's Format menu: Apple's processed photo. A file format's name.")
        case .raw:
            LocalizedStringResource(
                "RAW", comment: "A photo format in the camera's Format menu: the sensor's data, saved as DNG. Always capitalized.")
        }
    }
}

/// One back camera (by default the ultra-wide where it is the macro lens,
/// else the 1× wide; `setLens` picks another), focus locked where the lens
/// rests (`setRestLens`), taking bursts into a new folder. The physical
/// camera, not the virtual multi-camera, so iOS never switches lenses
/// mid-burst.
///
/// A sweep keeps focus fixed while the shutter is held and the phone moves
/// the focal plane through the subject. A bracket keeps the phone still
/// and steps the lens from closest focus out, one photo per settled step.
/// Exposure and white balance lock for a burst, and every frame keeps the
/// first's orientation.
///
/// A video feed drives focus peaking (`Peaking`).
///
/// Session work runs on `queue`, where the burst state lives alone
/// (`BurstState`); photo callbacks hop to it, and files are written on
/// `writes`. What the main thread also reads or sets is under `lock`.
nonisolated final class CameraController: NSObject, @unchecked Sendable {
    enum SetupError: LocalizedError {
        case denied
        case noCamera
        case cannotConfigure

        var errorDescription: String? {
            switch self {
            case .denied:
                String(
                    localized: "Camera access is off. Turn it on for Deptheos in Settings.",
                    comment: "Shown on the camera screen when Deptheos isn't allowed to use the camera.")
            case .noCamera:
                String(
                    localized: "This device has no back camera.",
                    comment: "Shown on the camera screen on a device without a back camera.")
            case .cannotConfigure:
                String(
                    localized: "The camera couldn't be set up. Close the camera and open it again.",
                    comment: "Shown on the camera screen when the camera couldn't be set up.")
            }
        }
    }

    /// How a kept burst came out.
    enum BurstOutcome: Equatable, Sendable {
        /// Every frame it set out to take.
        case complete
        /// `taken` frames on disk short of the plan: a bracket of `planned`
        /// steps with a step whose photo failed every try (no `why`), or any
        /// burst cut short with two frames or more (`why`). Stacked or
        /// discarded as the user says.
        case partial(taken: Int, planned: Int?, why: String?)
    }

    /// Interrupted (a call, another app) or stopped by an error: a burst
    /// asked for meanwhile isn't taken, one running is cut short.
    struct BurstInterrupted: LocalizedError {
        var errorDescription: String? {
            String(
                localized: "The burst was interrupted.",
                comment: "Error when a camera burst was cut short by a call, another app or the camera stopping.")
        }
    }

    /// `maxFailedInRow` drops in a row cut the burst short.
    struct FramesDropped: LocalizedError {
        var errorDescription: String? {
            String(
                localized: "The burst stopped because the camera dropped several frames in a row.",
                comment: "Error when a camera burst was cut short because several of its frames in a row couldn't be taken.")
        }
    }

    /// Free space fell below `spaceReserve`: cut short, leaving room to
    /// stack it.
    struct OutOfSpace: LocalizedError {
        var errorDescription: String? {
            String(
                localized: "The burst stopped because this device is almost full.",
                comment: "Error when a camera burst was cut short because the device's free space ran low.")
        }
    }

    /// Cut short with fewer than two frames: discarded.
    struct BurstCutShort: LocalizedError {
        let why: Error

        var errorDescription: String? {
            String(
                localized: "\(why.localizedDescription) Too few frames were taken to stack.",
                comment: "Error when a camera burst was cut short with fewer than two frames, which are discarded. The placeholder is the sentence saying what cut it short.")
        }
    }

    /// Ended by `cancelBurst()` or the camera stopping: discarded.
    struct BurstCanceled: LocalizedError {
        var errorDescription: String? {
            String(
                localized: "The burst was canceled.",
                comment: "Error when a camera burst was canceled before it finished; its frames are discarded.")
        }
    }

    /// Most often the device is full.
    struct FrameNotSaved: LocalizedError {
        let underlying: Error

        var errorDescription: String? {
            let full = (underlying as? CocoaError)?.code == .fileWriteOutOfSpace
                || (underlying as NSError).code == Int(ENOSPC)
            return full
                ? String(
                    localized: "The burst stopped because this device is full. Free up space, then take the burst again.",
                    comment: "Shown on the camera screen when a burst's frame couldn't be written because the device is full.")
                : String(
                    localized: "The burst stopped because a frame couldn't be saved. \(underlying.localizedDescription)",
                    comment: "Shown on the camera screen when a burst's frame couldn't be written. The placeholder is the system's own sentence about why.")
        }
    }

    /// Asked for while one runs; not taken.
    struct BurstBusy: LocalizedError {
        var errorDescription: String? {
            String(
                localized: "A burst is already running.",
                comment: "Error when the shutter is pressed while a burst is still running.")
        }
    }

    /// Past it the burst ends as if the shutter was let go.
    static let maxFrames = 120
    /// Dropped frames in a row that cut a burst short (`FramesDropped`).
    static let maxFailedInRow = 5
    /// Once a written frame leaves less, the burst is cut short
    /// (`OutOfSpace`), with room to stack it.
    static let spaceReserve: Int64 = 500_000_000
    /// Asking faster than the pipeline finishes backs it up until it
    /// refuses every request.
    static let maxInFlight = 3
    /// Retakes of a dropped bracket step before it goes on without it.
    static let maxRetakes = 2
    /// Past these within `restartWindow` seconds the session stays failed.
    static let maxRestarts = 3
    static let restartWindow: TimeInterval = 10

    let session = AVCaptureSession()
    let output = AVCapturePhotoOutput()
    let video = AVCaptureVideoDataOutput()
    let videoQueue = DispatchQueue(label: "Deptheos camera video")
    /// Set by the preview.
    var peaking: Peaking? {
        get { lock.withLock { _peaking } }
        set { lock.withLock { _peaking = newValue } }
    }
    private var _peaking: Peaking?
    var readiness: AVCapturePhotoOutputReadinessCoordinator?
    let queue = DispatchQueue(label: "Deptheos camera")

    /// A back camera the user can pick, as its button shows it.
    struct LensChoice: Identifiable, Equatable, Sendable {
        /// The device type's raw value: stable across launches.
        let id: String
        /// Against the 1× wide: "0.5×", "1×", "3×".
        let label: String
    }

    /// The camera in use and what follows from it: set up on `queue` (first
    /// `start`, `setLens`), read on the main thread too, so under `lock`.
    /// `nil` until set up; replaced whole when the lens changes.
    struct Camera {
        let device: AVCaptureDevice
        /// Its lens reports are on this input's clock.
        let input: AVCaptureDeviceInput
        let lens: LensChoice
        /// In order of magnification.
        let lenses: [LensChoice]
        /// The ultra-wide as the macro lens: it focuses closer than the
        /// wide.
        let macro: Bool
        /// Read once the configuration is committed; `nil` when none.
        var rawPixelFormat: OSType?
        let resolutions: [PhotoResolution: CMVideoDimensions]
        /// In EV.
        let biasRange: ClosedRange<Float>
    }
    var _camera: Camera?
    var camera: Camera? { lock.withLock { _camera } }

    var device: AVCaptureDevice? { camera?.device }

    /// In order of magnification; empty until set up.
    var lenses: [LensChoice] { camera?.lenses ?? [] }
    /// `nil` until set up.
    var lens: LensChoice? { camera?.lens }
    /// Set before `start()`: used in place of the automatic choice; `nil`
    /// or an id this device lacks keeps it.
    var preferredLens: String? {
        get { lock.withLock { _preferredLens } }
        set { lock.withLock { _preferredLens = newValue } }
    }
    private var _preferredLens: String?

    /// On the main actor after `setLens`, so the preview can rebuild.
    var onLensChange: (@MainActor () -> Void)? {
        get { lock.withLock { _onLensChange } }
        set { lock.withLock { _onLensChange = newValue } }
    }
    private var _onLensChange: (@MainActor () -> Void)?

    /// Set by the preview, which knows which way up the phone is. Briefly
    /// `nil` during a lens change; a burst begun then keeps the last angle.
    var rotation: AVCaptureDevice.RotationCoordinator? {
        get { lock.withLock { _rotation } }
        set { lock.withLock { _rotation = newValue } }
    }
    private var _rotation: AVCaptureDevice.RotationCoordinator?
    /// For a burst begun with no `rotation`.
    var _lastAngle: CGFloat = 90

    /// 0 (closest) to 1; sweeps are taken there. Under `lock`.
    var _restLens: Float = 0
    /// In EV, kept across bursts and lenses (each camera clamps it). Under
    /// `lock`.
    var _exposureBias: Float = 0
    /// Reapplied when a lens change rebuilds the feed. On `queue` only.
    var peakingEnabled = true
    var previewAngle: CGFloat?
    var previewMirrored: Bool?
    /// Sweeps want zero shutter lag, brackets don't (`setBracketMode`);
    /// reapplied to a new camera. On `queue` only.
    var zeroShutterLagWanted = true
    /// Exposure and system-pressure observers; `nil` while stopped. On
    /// `queue` only.
    var watch: DeviceWatch?

    let lock = NSLock()
    /// On `queue` only, changed through `withBurst`.
    var burstState = BurstState.idle
    /// Numbers each `Burst.id`. On `queue` only.
    var burstsBegun = 0
    /// Frames are written here in numbered order, off `queue`.
    let writes = DispatchQueue(label: "Deptheos camera writes", qos: .userInitiated)

    /// For bursts started from now; `nil` is as fast as the camera can.
    var framesPerSecond: Double? {
        get { lock.withLock { _framesPerSecond } }
        set { lock.withLock { _framesPerSecond = newValue } }
    }
    private var _framesPerSecond: Double?

    /// Applied between bursts. RAW falls back to HEIC without Bayer RAW.
    func setFormat(_ format: PhotoFormat) {
        queue.async {
            self.lock.withLock { self._format = format }
            self.prepare()
        }
    }
    var _format = PhotoFormat.heic

    /// `nil` when the camera has none.
    var rawPixelFormat: OSType? { camera?.rawPixelFormat }
    var canCaptureRaw: Bool { rawPixelFormat != nil }

    var resolutions: [PhotoResolution: CMVideoDimensions] { camera?.resolutions ?? [:] }

    /// In EV; `0...0` until set up.
    var exposureBiasRange: ClosedRange<Float> { camera?.biasRange ?? 0...0 }

    /// As auto exposure has them.
    struct Exposure: Equatable, Sendable {
        /// Seconds.
        var duration: Double
        var iso: Float
    }

    /// On the main actor as auto exposure changes, at most about four times
    /// a second and only for a change worth showing.
    var onExposure: (@MainActor (Exposure) -> Void)? {
        get { lock.withLock { _onExposure } }
        set { lock.withLock { _onExposure = newValue } }
    }
    private var _onExposure: (@MainActor (Exposure) -> Void)?

    /// Applied between bursts.
    func setResolution(_ resolution: PhotoResolution) {
        queue.async {
            self.lock.withLock { self._resolution = resolution }
            self.applyResolution()
            self.prepare()
        }
    }
    var _resolution = PhotoResolution.twelve

    /// On the main actor with the frames written so far.
    var onFrameCount: (@MainActor (Int) -> Void)? {
        get { lock.withLock { _onFrameCount } }
        set { lock.withLock { _onFrameCount = newValue } }
    }
    private var _onFrameCount: (@MainActor (Int) -> Void)?
    /// On the main thread as each exposure starts: the shutter blink.
    var onShutter: (@MainActor () -> Void)? {
        get { lock.withLock { _onShutter } }
        set { lock.withLock { _onShutter = newValue } }
    }
    private var _onShutter: (@MainActor () -> Void)?

    /// On the main actor with each session state change; the camera
    /// shoots only while running.
    var onSessionChange: (@MainActor (SessionState) -> Void)? {
        get { lock.withLock { _onSessionChange } }
        set { lock.withLock { _onSessionChange = newValue } }
    }
    private var _onSessionChange: (@MainActor (SessionState) -> Void)?
    var observers: [NSObjectProtocol] = []

    /// Where the session is.
    ///
    /// Lifecycle: stopped, running, interrupted (why, as said on screen) or
    /// failed. Changed only on `queue` by `become`
    /// (`CameraController+Interruptions.swift`), which tells the view
    /// through `onSessionChange`. `start` runs it, `stop` stops it, an
    /// interruption and its end move between running and interrupted, and
    /// an unrecovered runtime error (or too many) fails it. Set up in
    /// `CameraController+Setup.swift`.
    enum SessionState: Equatable {
        case stopped
        case running
        /// A call, another app, screen sharing (no camera while
        /// multitasking) or heat; why, as said on screen.
        case interrupted(LocalizedStringResource)
        /// A runtime error, not restarted.
        case failed

        /// Why the camera can't shoot; `nil` while it can or is stopped on
        /// purpose.
        var message: LocalizedStringResource? {
            switch self {
            case .stopped, .running: nil
            case .interrupted(let message): message
            case .failed:
                LocalizedStringResource(
                    "The camera stopped. Close it and open it again.",
                    comment: "Hint on the camera screen after the camera stopped with an error it didn't recover from.")
            }
        }

        /// For the log.
        var label: String {
            switch self {
            case .stopped: "stopped"
            case .running: "running"
            case .interrupted(let message): "interrupted (\(message.key))"
            case .failed: "failed"
            }
        }
    }
    /// Changed only on `queue` through `become`, so changes reach the main
    /// actor in order.
    var sessionState = SessionState.stopped
    /// The last `restartWindow` seconds' restarts. On `queue` only.
    var restarts: [Date] = []

    deinit {
        observers.forEach(NotificationCenter.default.removeObserver)
    }

    /// Kept on while shooting.
    var hasTorch: Bool { device?.hasTorch ?? false }

    /// Full brightness or off.
    func setTorch(_ on: Bool) {
        setTorch(on ? 1 : 0)
    }

    /// 0 (off) to 1 (the most it can give; less when hot). Ignored during a
    /// burst, whose exposure is locked for its light.
    func setTorch(_ level: Float) {
        queue.async {
            guard self.burstIdle else {
                log.debug("torch not changed: a burst is running")
                return
            }
            guard let device = self.device, device.hasTorch,
                  (try? device.lockForConfiguration()) != nil
            else { return }
            defer { device.unlockForConfiguration() }
            guard level > 0 else {
                device.torchMode = .off
                return
            }
            do {
                try device.setTorchModeOn(
                    level: level >= 1 ? AVCaptureDevice.maxAvailableTorchLevel : level)
            } catch {
                // That brightness isn't available now (heat): the most that is.
                log.info("torch at \(level, format: .fixed(precision: 2)) unavailable: \(String(describing: error), privacy: .public)")
                try? device.setTorchModeOn(level: AVCaptureDevice.maxAvailableTorchLevel)
            }
        }
    }

    /// Where a burst is.
    ///
    /// Lifecycle: idle, metering, sweeping, bracket (at a step) or draining
    /// (with its `Ending`), each holding its `Burst`. Changed only on
    /// `queue` through `withBurst`, by the methods in
    /// `CameraController+Burst.swift`; photo callbacks hop to `queue` and
    /// match the burst's `id`. It ends as a `BurstOutcome`.
    ///
    /// idle → metering (`beginBurst`): auto exposure gets up to
    /// `meterTimeout` to settle before it locks. A sweep let go meanwhile
    /// ends after its first photo (`letGo`). metering → sweeping, or
    /// bracket at step 0, once settled.
    ///
    /// sweeping → draining (`endBurst`, or `maxFrames`). Let go before its
    /// first photo was asked for, it stays sweeping until then: a tap takes
    /// one photo, never none.
    ///
    /// bracket: at each step the lens moves and settles, then the photo is
    /// asked for; once it is written or dropped the bracket steps on, or
    /// retakes a drop up to `maxRetakes` times. The lens moves only after
    /// the step's photo settles, so files are numbered near to far. After
    /// the last step it drains.
    ///
    /// Any running burst → draining early (`end`): cut short or canceled
    /// (`cancelBurst`, `stop`); a weightier later reason replaces the one
    /// there.
    ///
    /// draining → idle once nothing is in flight or being written, or at
    /// its deadline (`drainTimeout`, `doneDrainTimeout`); the completion
    /// then runs once.
    enum BurstState {
        case idle
        /// Waiting for auto exposure to settle; `letGo` once a sweep is let
        /// go meanwhile.
        case metering(Burst, letGo: Bool)
        /// `letGo` once let go before the first photo was asked for.
        case sweeping(Burst, letGo: Bool)
        case bracket(Burst, Bracket)
        /// Ends once the photos in flight land, or at `until` without them.
        case draining(Burst, Ending, until: DispatchTime)

        var isDraining: Bool {
            if case .draining = self { true } else { false }
        }

        var burst: Burst? {
            switch self {
            case .idle: nil
            case .metering(let burst, _), .sweeping(let burst, _), .bracket(let burst, _),
                 .draining(let burst, _, _): burst
            }
        }

        /// Change the running burst if it is `id`; `nil` when it isn't.
        @discardableResult
        mutating func change<T>(_ id: Int, _ body: (inout Burst) -> T) -> T? {
            switch self {
            case .idle:
                return nil
            case .metering(var burst, let letGo):
                guard burst.id == id else { return nil }
                let result = body(&burst)
                self = .metering(burst, letGo: letGo)
                return result
            case .sweeping(var burst, let letGo):
                guard burst.id == id else { return nil }
                let result = body(&burst)
                self = .sweeping(burst, letGo: letGo)
                return result
            case .bracket(var burst, let bracket):
                guard burst.id == id else { return nil }
                let result = body(&burst)
                self = .bracket(burst, bracket)
                return result
            case .draining(var burst, let ending, let until):
                guard burst.id == id else { return nil }
                let result = body(&burst)
                self = .draining(burst, ending, until: until)
                return result
            }
        }

        /// For the log.
        var label: String {
            switch self {
            case .idle: "idle"
            case .metering(_, let letGo): letGo ? "metering, shutter let go" : "metering"
            case .sweeping(_, let letGo): letGo ? "sweeping, shutter let go" : "sweeping"
            case .bracket(_, let bracket):
                "bracket step \(bracket.step + 1), try \(bracket.tries + 1), lens \(bracket.lens.label)"
            case .draining(_, let ending, _):
                "draining (\(ending.label))"
            }
        }
    }

    /// Why a draining burst ends, so what it completes with.
    enum Ending {
        /// Its last photo asked for: let go, `maxFrames`, or a bracket's
        /// last step. Kept.
        case done
        /// Cut short for `why` (`BurstInterrupted`, `FramesDropped`,
        /// `FrameNotSaved`, `OutOfSpace`, or a lens that won't move). Kept
        /// with two frames or more for the user to decide; else discarded
        /// and said (`BurstCutShort`).
        case cutShort(Error)
        /// Canceled: discarded.
        case canceled

        /// A cancel outweighs a cut short, which outweighs an ordinary end;
        /// the first of two alike stands.
        private var weight: Int {
            switch self {
            case .done: 0
            case .cutShort: 1
            case .canceled: 2
            }
        }

        func merged(with other: Ending) -> Ending {
            other.weight > weight ? other : self
        }

        /// How long what is in flight gets to land.
        var drainTimeout: TimeInterval {
            if case .done = self { CameraController.doneDrainTimeout } else { CameraController.drainTimeout }
        }

        var label: String {
            switch self {
            case .done: "done"
            case .cutShort(let error): "cut short: \(error)"
            case .canceled: "canceled"
            }
        }
    }

    /// A bracket's place: the step, which try at it, and its lens.
    struct Bracket {
        var step = 0
        /// 0 for the step's first photo, one more per retake.
        var tries = 0
        var lens = Lens.moving
    }

    /// A bracket's lens at its step.
    enum Lens {
        case moving
        /// At this session-clock time, its photo not yet asked for.
        case settled(CMTime?)
        /// Its photo (request id) in flight, with when the lens settled, for
        /// the log's check that it was exposed after.
        case exposing(Int64, settled: CMTime?)

        var label: String {
            switch self {
            case .moving: "moving"
            case .settled(let at): at == nil ? "settled (no report)" : "settled"
            case .exposing: "exposing"
            }
        }
    }

    /// On `queue` only. Each change is logged.
    @discardableResult
    func withBurst<T>(_ body: (inout BurstState) -> T) -> T {
        dispatchPrecondition(condition: .onQueue(queue))
        let (before, wasDraining) = (burstState.label, burstState.isDraining)
        let result = body(&burstState)
        let after = burstState.label
        if after != before {
            log.debug("burst: \(before, privacy: .public) → \(after, privacy: .public)")
        }
        // Every drain has a deadline, an ordinary end's too.
        if !wasDraining, case .draining(let burst, _, let until) = burstState {
            drain(burst.id, until: until)
        }
        return result
    }

    /// On `queue`.
    var burstIdle: Bool {
        dispatchPrecondition(condition: .onQueue(queue))
        if case .idle = burstState { return true } else { return false }
    }

    /// One burst's settings and counts, inside its `BurstState`, so changed
    /// only on `queue`.
    struct Burst {
        enum Kind {
            /// At most one photo every `interval` seconds while held.
            case sweep(interval: TimeInterval)
            /// One photo per lens position, near to far.
            case bracket([Float])
        }

        /// Of `burstsBegun`: what a late callback checks against.
        let id: Int
        let kind: Kind
        let folder: URL
        let angle: CGFloat
        /// For a RAW burst; `nil` takes processed photos with `codec`.
        let rawPixelFormat: OSType?
        let codec: AVVideoCodecType
        let fileExtension: String
        /// The size asked for.
        let dimensions: CMVideoDimensions
        /// The size the first photo came out at.
        var delivered: CMVideoDimensions?
        /// For the burst's rate.
        var firstExposure: CMTime?
        var lastExposure: CMTime?
        /// A sweep's last request, for its rate cap, and whether one waits.
        var lastRequest: Date?
        var waiting = false
        let completion: @MainActor (Result<BurstOutcome, Error>) -> Void
        var requested = 0
        /// In flight, by request id. A photo not in it (past a drain's
        /// deadline) is ignored.
        var pending: Set<Int64> = []
        var inFlight: Int { pending.count }
        /// Numbered and being written, by request id: the burst doesn't
        /// finish, nor a bracket step go on, until they are on disk or
        /// failed.
        var writing: Set<Int64> = []
        /// A bracket's photos on disk its step hasn't seen, by request id.
        var saved: Set<Int64> = []
        /// Written or being written.
        var numbered = 0
        /// Frames on disk.
        var written = 0
        /// A burst rides out a few in a row.
        var failedInRow = 0
        /// Bracket steps retaken, and given up on.
        var retakes = 0
        var missed = 0

        init(
            id: Int, kind: Kind, folder: URL, angle: CGFloat, rawPixelFormat: OSType?, codec: AVVideoCodecType,
            dimensions: CMVideoDimensions,
            completion: @escaping @MainActor (Result<BurstOutcome, Error>) -> Void
        ) {
            self.id = id
            self.kind = kind
            self.folder = folder
            self.dimensions = dimensions
            self.angle = angle
            self.rawPixelFormat = rawPixelFormat
            self.codec = codec
            fileExtension = rawPixelFormat != nil ? "dng" : codec == .hevc ? "heic" : "jpg"
            self.completion = completion
        }

        /// A dropped frame: after a few in a row the burst ends as `ending`,
        /// which this returns.
        mutating func dropFrame(_ reason: String, ending: @autoclosure () -> Ending) -> Ending? {
            failedInRow += 1
            log.error("dropped a frame (\(self.failedInRow) in a row): \(reason, privacy: .public)")
            return failedInRow >= CameraController.maxFailedInRow ? ending() : nil
        }
    }
}
