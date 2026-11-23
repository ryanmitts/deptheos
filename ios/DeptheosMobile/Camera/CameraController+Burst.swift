@preconcurrency import AVFoundation
import Foundation
import os

private nonisolated let log = AppLog(category: "camera")

/// Taking a burst (`BurstState`), and the photo output's callbacks, which
/// hop to `queue`.
nonisolated extension CameraController {
    /// Start a burst into `folder`, running until `endBurst()` (or
    /// `maxFrames`); `completion` runs once every frame is on disk. With
    /// `bracket` (lens positions, near to far) it takes one photo per
    /// position and ends itself. Fails with `BurstBusy` during another, and
    /// `BurstInterrupted` while not running. `cancelBurst()` discards it.
    func beginBurst(
        into folder: URL, bracket: [Float]? = nil,
        completion: @escaping @MainActor (Result<BurstOutcome, Error>) -> Void
    ) {
        let current = rotation?.videoRotationAngleForHorizonLevelCapture
        let angle = lock.withLock { () -> CGFloat in
            if let current { _lastAngle = current }
            return _lastAngle
        }
        let framesPerSecond = self.framesPerSecond
        queue.async {
            // The interruption came already: a burst now would wait for photos
            // a paused session never takes.
            guard self.sessionState == .running else {
                log.info("burst not started: the session is \(self.sessionState.label, privacy: .public)")
                Task { @MainActor in completion(.failure(BurstInterrupted())) }
                return
            }
            let (raw, codec, dimensions) = self.nextBurstFormat()
            let kind: Burst.Kind = bracket.map { .bracket($0) }
                ?? .sweep(interval: framesPerSecond.map { 1 / $0 } ?? 0)
            self.burstsBegun += 1
            let burst = Burst(
                id: self.burstsBegun, kind: kind, folder: folder, angle: angle, rawPixelFormat: raw,
                codec: codec, dimensions: dimensions, completion: completion)
            let began = self.withBurst { state -> Bool in
                guard case .idle = state else { return false }
                state = .metering(burst, letGo: false)
                return true
            }
            guard began else {
                log.error("burst asked for while one runs")
                Task { @MainActor in completion(.failure(BurstBusy())) }
                return
            }
            log.info("burst started: \(bracket.map { "bracket of \($0.count)" } ?? "sweep", privacy: .public), \(raw != nil ? "Bayer RAW DNG" : codec == .hevc ? "HEIC" : "JPEG", privacy: .public)")
            // Normally set already, by `setBracketMode`.
            self.setZeroShutterLag(bracket == nil)
            self.meterThenShoot(burst.id, until: .now() + Self.meterTimeout)
        }
    }

    /// Longest wait for auto exposure to settle before locking.
    static let meterTimeout: TimeInterval = 0.5

    /// Lock exposure for burst `id` once auto exposure settles (or at
    /// `deadline`), then take its first photo; nothing if it ended
    /// meanwhile. On `queue`.
    private func meterThenShoot(_ id: Int, until deadline: DispatchTime) {
        guard case .metering(let burst, _) = burstState, burst.id == id else { return }
        let adjusting = device?.isAdjustingExposure ?? false
        if adjusting, DispatchTime.now() < deadline {
            queue.asyncAfter(deadline: .now() + 0.05) { self.meterThenShoot(id, until: deadline) }
            return
        }
        let shoots = withBurst { state -> Bool in
            guard case .metering(let burst, let letGo) = state, burst.id == id else { return false }
            if case .bracket = burst.kind {
                state = .bracket(burst, Bracket())
            } else {
                state = .sweeping(burst, letGo: letGo)
            }
            return true
        }
        guard shoots else { return }
        if adjusting {
            log.info("exposure still adjusting after \(Self.meterTimeout, format: .fixed(precision: 1)) s; locked as it was")
        }
        setLocked(true)
        if case .bracket = burst.kind {
            moveLens(burst, Bracket())
            return
        }
        captureNext()
    }

    /// A bracket ignores it and runs its course.
    func endBurst() {
        queue.async {
            self.withBurst { state in
                switch state {
                case .sweeping(let burst, letGo: false):
                    // Its first photo not asked for yet: it is, then it ends.
                    state = burst.requested > 0
                        ? .draining(burst, .done, until: .now() + Self.doneDrainTimeout) : .sweeping(burst, letGo: true)
                case .metering(let burst, letGo: false):
                    if case .sweep = burst.kind {
                        state = .metering(burst, letGo: true)
                    }
                default:
                    break
                }
            }
            self.finishIfDone()
        }
    }

    /// End a running burst now and discard it: in-flight photos get a moment,
    /// then it completes with `BurstCanceled`. One already ending ends
    /// canceled.
    func cancelBurst() {
        queue.async { self.end(.canceled) }
    }

    /// Exposure and white balance held for a burst, automatic between, the
    /// lens back at rest after. A locked exposure keeps its metered bias.
    private func setLocked(_ locked: Bool) {
        let rest = lock.withLock { _restLens }
        guard let device, (try? device.lockForConfiguration()) != nil else { return }
        defer { device.unlockForConfiguration() }
        if !locked, device.isLockingFocusWithCustomLensPositionSupported {
            device.setFocusModeLocked(lensPosition: rest)
        }
        let exposure: AVCaptureDevice.ExposureMode = locked ? .locked : .continuousAutoExposure
        if device.isExposureModeSupported(exposure) {
            device.exposureMode = exposure
        }
        let balance: AVCaptureDevice.WhiteBalanceMode =
            locked ? .locked : .continuousAutoWhiteBalance
        if device.isWhiteBalanceModeSupported(balance) {
            device.whiteBalanceMode = balance
        }
    }

    /// From the choices made.
    private func nextBurstFormat()
        -> (raw: OSType?, codec: AVVideoCodecType, dimensions: CMVideoDimensions)
    {
        let raw = lock.withLock { _format == .raw } ? rawPixelFormat : nil
        let codec: AVVideoCodecType =
            output.availablePhotoCodecTypes.contains(.hevc) ? .hevc : .jpeg
        // Bayer RAW is the sensor's 12 MP readout; larger would be refused.
        let dimensions = raw != nil ? resolutions[.twelve] ?? burstDimensions : burstDimensions
        return (raw, codec, dimensions)
    }

    /// Bayer RAW in `rawPixelFormat`, or processed in `codec`.
    private static func photoSettings(
        rawPixelFormat: OSType?, codec: AVVideoCodecType, dimensions: CMVideoDimensions
    ) -> AVCapturePhotoSettings {
        let settings = rawPixelFormat.map { AVCapturePhotoSettings(rawPixelFormatType: $0) }
            ?? AVCapturePhotoSettings(format: [AVVideoCodecKey: codec])
        settings.maxPhotoDimensions = dimensions
        settings.photoQualityPrioritization = .speed
        settings.flashMode = .off
        return settings
    }

    /// Set the pipeline up for the next burst, so its first frame doesn't
    /// wait (switching to RAW is slow). On `queue`, between bursts.
    func prepare() {
        guard device != nil, burstIdle else { return }
        let (raw, codec, dimensions) = nextBurstFormat()
        guard dimensions.width > 0 else { return }
        let settings = Self.photoSettings(rawPixelFormat: raw, codec: codec, dimensions: dimensions)
        let started = Date.now
        output.setPreparedPhotoSettingsArray([settings]) { prepared, error in
            let took = Int(Date.now.timeIntervalSince(started) * 1000)
            if let error {
                log.error("photo pipeline not prepared: \(String(describing: error), privacy: .public)")
            } else {
                log.info("photo pipeline prepared for \(raw != nil ? "RAW" : "HEIC", privacy: .public) \(dimensions.width)×\(dimensions.height) in \(took) ms (\(prepared ? "ok" : "not prepared", privacy: .public))")
            }
        }
    }

    /// If the burst still wants one: a sweep held, or a bracket whose lens
    /// settled.
    private func captureNext() {
        enum Next {
            case nothing
            case retry(Int, after: TimeInterval)
            case capture(AVCapturePhotoSettings, angle: CGFloat)
        }
        /// Counted and pending.
        func request(_ burst: inout Burst) -> AVCapturePhotoSettings {
            burst.requested += 1
            let settings = Self.photoSettings(
                rawPixelFormat: burst.rawPixelFormat, codec: burst.codec, dimensions: burst.dimensions)
            burst.pending.insert(settings.uniqueID)
            return settings
        }
        let next: Next = withBurst { state in
            guard output.captureReadiness == .ready else { return .nothing }
            switch state {
            case .sweeping(var burst, let letGo):
                guard burst.inFlight < Self.maxInFlight else { return .nothing }
                // Too soon for the rate cap: ask again when it is time (once;
                // other triggers find `waiting` set).
                if case .sweep(let interval) = burst.kind, let last = burst.lastRequest {
                    let wait = interval - Date.now.timeIntervalSince(last)
                    if wait > 0 {
                        guard !burst.waiting else { return .nothing }
                        burst.waiting = true
                        state = .sweeping(burst, letGo: letGo)
                        return .retry(burst.id, after: wait)
                    }
                }
                burst.lastRequest = .now
                let settings = request(&burst)
                // The last frame: let go before the first, or `maxFrames`.
                state = letGo || burst.requested >= Self.maxFrames
                    ? .draining(burst, .done, until: .now() + Self.doneDrainTimeout) : .sweeping(burst, letGo: letGo)
                return .capture(settings, angle: burst.angle)
            case .bracket(var burst, var bracket):
                guard case .settled(let settledAt) = bracket.lens, burst.inFlight == 0
                else { return .nothing }
                let settings = request(&burst)
                bracket.lens = .exposing(settings.uniqueID, settled: settledAt)
                state = .bracket(burst, bracket)
                return .capture(settings, angle: burst.angle)
            case .idle, .metering, .draining:
                return .nothing
            }
        }
        let settings: AVCapturePhotoSettings, angle: CGFloat
        switch next {
        case .nothing:
            return
        case .retry(let id, let wait):
            queue.asyncAfter(deadline: .now() + wait) {
                self.withBurst { $0.change(id) { $0.waiting = false } }
                self.captureNext()
            }
            return
        case .capture(let asked, let at):
            (settings, angle) = (asked, at)
        }

        if let connection = output.connection(with: .video),
           connection.isVideoRotationAngleSupported(angle) {
            connection.videoRotationAngle = angle
        }
        readiness?.startTrackingCaptureRequest(using: settings)
        output.capturePhoto(with: settings, delegate: self)
    }

    /// A bracket step's photo `id` is written or dropped: on to the next
    /// step, or retake after a drop (up to `maxRetakes`, then on without
    /// it); after the last, drain. The lens moves only now, so a retake
    /// stays in focus order.
    private func stepFinished(_ id: Int64) {
        let next = withBurst { state -> (Burst, Bracket)? in
            guard case .bracket(var burst, var bracket) = state,
                  case .exposing(id, _) = bracket.lens,
                  case .bracket(let positions) = burst.kind
            else { return nil }
            if burst.saved.remove(id) != nil {
                bracket.step += 1
                bracket.tries = 0
            } else if bracket.tries < Self.maxRetakes {
                bracket.tries += 1
                burst.retakes += 1
            } else {
                log.warning("bracket step \(bracket.step + 1) of \(positions.count) left out: its photo failed \(bracket.tries + 1) times")
                burst.missed += 1
                bracket.step += 1
                bracket.tries = 0
            }
            guard bracket.step < positions.count else {
                state = .draining(burst, .done, until: .now() + Self.doneDrainTimeout)
                return nil
            }
            bracket.lens = .moving
            state = .bracket(burst, bracket)
            return (burst, bracket)
        }
        guard let (burst, bracket) = next else { return }
        moveLens(burst, bracket)
    }

    /// Its photo is asked for when it settles. A retake finds it there.
    private func moveLens(_ burst: Burst, _ bracket: Bracket) {
        let (id, step, tries) = (burst.id, bracket.step, bracket.tries)
        guard case .bracket(let positions) = burst.kind, positions.indices.contains(step),
              let device
        else {
            end(.cutShort(SetupError.noCamera), of: id)
            return
        }
        let position = positions[step]
        let settle: @Sendable (CMTime?) -> Void = { syncTime in
            self.queue.async {
                let settledAt = syncTime.flatMap(self.sessionTime)
                let first = self.withBurst { state -> Bool in
                    guard case .bracket(let at, var now) = state, at.id == id,
                          now.step == step, now.tries == tries, case .moving = now.lens
                    else { return false }
                    now.lens = .settled(settledAt)
                    state = .bracket(at, now)
                    return true
                }
                guard first else { return }
                self.captureNext()
            }
        }
        do {
            try device.lockForConfiguration()
        } catch {
            // No lens to move: the bracket is cut short, not stuck.
            log.error("lens not locked at step \(step + 1): \(String(describing: error), privacy: .public)")
            end(.cutShort(error), of: id)
            return
        }
        defer { device.unlockForConfiguration() }
        // A lens already where it is told to go reports no move.
        if device.focusMode == .locked, abs(device.lensPosition - position) < 0.0005 {
            settle(nil)
            return
        }
        log.debug("lens to \(position, format: .fixed(precision: 4)) for step \(step + 1)")
        device.setFocusModeLocked(lensPosition: position) { syncTime in settle(syncTime) }
        // Should the report never come, go on without it.
        queue.asyncAfter(deadline: .now() + 0.5) { settle(nil) }
    }

    /// A lens report's device-clock time on the session clock, where photo
    /// timestamps are; `nil` when either is unknown. On `queue`.
    private func sessionTime(_ deviceTime: CMTime) -> CMTime? {
        guard deviceTime.isValid, let sessionClock = session.synchronizationClock,
              let deviceClock = camera?.input.ports.first(where: { $0.mediaType == .video })?.clock
        else { return nil }
        return CMSyncConvertTime(deviceTime, from: deviceClock, to: sessionClock)
    }

    /// End the running burst (only `id`, if given) for `ending`. In-flight
    /// photos get `ending.drainTimeout` (the earliest deadline stands), then
    /// it goes on without them. On `queue`.
    func end(_ ending: Ending, of id: Int? = nil) {
        let deadline: DispatchTime = .now() + ending.drainTimeout
        let ended = withBurst { state -> Int? in
            guard let running = state.burst, id.map({ $0 == running.id }) ?? true else { return nil }
            switch state {
            case .idle:
                return nil
            case .metering, .sweeping, .bracket:
                state = .draining(running, ending, until: deadline)
            case .draining(_, let was, let until):
                state = .draining(running, was.merged(with: ending), until: min(until, deadline))
            }
            return running.id
        }
        guard let running = ended else { return }
        finishIfDone()
        // Its own deadline: one earlier than the one it drained to stands.
        drain(running, until: deadline)
    }

    /// At `deadline`, if `running` still drains to it with photos in flight,
    /// go on without them. Set by `withBurst` and by `end`. On `queue`.
    func drain(_ running: Int, until deadline: DispatchTime) {
        queue.asyncAfter(deadline: deadline) {
            let stuck = self.withBurst { state -> Bool in
                guard case .draining(var draining, let ending, let until) = state, draining.id == running,
                      until <= .now(), !draining.pending.isEmpty
                else { return false }
                draining.pending.removeAll()
                state = .draining(draining, ending, until: until)
                return true
            }
            guard stuck else { return }
            log.error("burst's last photos never came; finishing without them")
            self.finishIfDone()
        }
    }

    /// Once nothing is in flight or being written: idle, then its
    /// completion, once.
    private func finishIfDone() {
        struct Finished {
            let completion: @MainActor (Result<BurstOutcome, Error>) -> Void
            let result: Result<BurstOutcome, Error>
            let summary: String
        }
        let finished = withBurst { state -> Finished? in
            guard case .draining(let burst, let ending, _) = state,
                  burst.pending.isEmpty, burst.writing.isEmpty
            else { return nil }
            state = .idle
            // A photo landing from now finds no burst.
            let planned: Int? = if case .bracket(let positions) = burst.kind { positions.count } else { nil }
            let result: Result<BurstOutcome, Error> =
                switch ending {
                case .done:
                    if let planned, burst.written < planned {
                        .success(.partial(taken: burst.written, planned: planned, why: nil))
                    } else {
                        .success(.complete)
                    }
                // Every early end, sweeps included: two frames or more are
                // kept for the user to decide.
                case .cutShort(let why):
                    burst.written >= 2
                        ? .success(.partial(taken: burst.written, planned: planned, why: why.localizedDescription))
                        : .failure(BurstCutShort(why: why))
                case .canceled: .failure(BurstCanceled())
                }
            let span = switch (burst.firstExposure, burst.lastExposure) {
            case (let first?, let last?): CMTimeGetSeconds(CMTimeSubtract(last, first))
            default: 0.0
            }
            let rate = burst.written > 1 && span > 0 ? Double(burst.written - 1) / span : 0
            var summary = "\(burst.written) frames of \(burst.requested) asked for, "
                + String(format: "%.1f fps", rate)
            if case .bracket(let positions) = burst.kind {
                summary += ", \(positions.count) steps, \(burst.retakes) retaken, \(burst.missed) left out"
            }
            summary += ", \(ending.label)"
            return Finished(completion: burst.completion, result: result, summary: summary)
        }
        guard let finished else { return }
        queue.async {
            self.setLocked(false)
            self.prepare()
        }
        log.info("burst: \(finished.summary, privacy: .public)")
        let (completion, result) = (finished.completion, finished.result)
        Task { @MainActor in completion(result) }
    }

    // MARK: Photos landing

    /// Read on the photo output's queue and handed to `queue`
    /// (`photoLanded`).
    struct LandedPhoto: @unchecked Sendable {
        /// `AVCapturePhotoSettings.uniqueID`.
        let id: Int64
        /// Unless it failed or has none.
        let file: Data?
        /// Why iOS dropped it, if it did.
        let error: String?
        let timestamp: CMTime
        let dimensions: CMVideoDimensions
    }

    /// Number it and write it on `writes` (`frameWritten` follows), or
    /// count a drop; too many in a row end the burst. On `queue`.
    func photoLanded(_ photo: LandedPhoto) {
        enum Landed {
            case ignored
            /// The burst ends as `ending` after too many in a row.
            case dropped(Int, ending: Ending?)
            case frame(Int, Data, URL)
        }
        let id = photo.id
        let landed = withBurst { state -> Landed in
            // A photo of no burst, or of one past its drain deadline.
            guard let running = state.burst, running.pending.contains(id) else { return .ignored }
            if let error = photo.error {
                // A pushed pipeline now and then refuses a frame ("Cannot
                // Take Photo"); a sweep skips it, a bracket retakes it.
                let ending = state.change(running.id) { $0.dropFrame(error, ending: .cutShort(FramesDropped())) } ?? nil
                return .dropped(running.id, ending: ending)
            }
            // A photo with no file is not a frame: no number, not counted.
            guard let data = photo.file else {
                let ending = state.change(running.id) {
                    $0.dropFrame("no file data", ending: .cutShort(FramesDropped()))
                } ?? nil
                return .dropped(running.id, ending: ending)
            }
            if case .bracket(_, let bracket) = state, case .exposing(id, let settled?) = bracket.lens,
               photo.timestamp.isValid {
                let lead = CMTimeGetSeconds(CMTimeSubtract(photo.timestamp, settled))
                log.debug("bracket frame \(running.numbered + 1): exposed \(Int(lead * 1000)) ms after the lens settled")
            }
            let url = state.change(running.id) { burst -> URL in
                burst.failedInRow = 0
                burst.numbered += 1
                burst.writing.insert(id)
                if burst.delivered == nil {
                    burst.delivered = photo.dimensions
                    log.info("first photo \(photo.dimensions.width)×\(photo.dimensions.height) (asked for \(burst.dimensions.width)×\(burst.dimensions.height))")
                }
                if burst.firstExposure == nil { burst.firstExposure = photo.timestamp }
                burst.lastExposure = photo.timestamp
                return burst.folder.appending(
                    path: String(format: "IMG_%04d.", burst.numbered) + burst.fileExtension)
            }
            return url.map { .frame(running.id, data, $0) } ?? .ignored
        }
        switch landed {
        case .ignored:
            return
        case .dropped(let burst, let ending):
            guard let ending else { return }
            end(ending, of: burst)
        case .frame(let burst, let data, let url):
            writes.async {
                let written = Result { try data.write(to: url, options: .atomic) }
                self.queue.async { self.frameWritten(id, of: burst, written) }
            }
        }
    }

    /// Photo `id` is on disk, or couldn't be written, which cuts the burst
    /// short, as free space below `spaceReserve` does. A bracket step goes
    /// on once its photo has both finished and been written. On `queue`.
    private func frameWritten(_ id: Int64, of burst: Int, _ written: Result<Void, Error>) {
        let (count, captured) = withBurst { state -> (Int?, Bool) in
            state.change(burst) { burst -> (Int?, Bool) in
                burst.writing.remove(id)
                let captured = !burst.pending.contains(id)
                guard case .success = written else { return (nil, captured) }
                // Counted once on disk, so a cut-short burst keeps only frames
                // it has.
                burst.written += 1
                if case .bracket = burst.kind { burst.saved.insert(id) }
                return (burst.written, captured)
            } ?? (nil, false)
        }
        if case .failure(let error) = written {
            log.error("frame not saved: \(String(describing: error), privacy: .public)")
            end(.cutShort(FrameNotSaved(underlying: error)), of: burst)
        }
        if let count {
            if let onFrameCount {
                Task { @MainActor in onFrameCount(count) }
            }
            // A sweep's length is unknown: it stops cleanly when room runs
            // low.
            if let free = AppFiles.freeSpace(), free < Self.spaceReserve {
                log.info("free space below the reserve after frame \(count): the burst is cut short")
                end(.cutShort(OutOfSpace()), of: burst)
            }
        }
        // Its capture finished while it was written: its step goes on now.
        if captured { stepFinished(id) }
        // A burst past its drain deadline waits only for this write.
        finishIfDone()
    }

    /// No longer in flight. A bracket step goes on unless the file is still
    /// being written (then `frameWritten` does). On `queue`.
    func captureFinished(_ id: Int64) {
        let writing = withBurst { state -> Bool in
            guard let running = state.burst else { return false }
            return state.change(running.id) { burst -> Bool in
                burst.pending.remove(id)
                return burst.writing.contains(id)
            } ?? false
        }
        if !writing { stepFinished(id) }
        captureNext()
        finishIfDone()
    }
}

nonisolated extension CameraController: AVCapturePhotoOutputReadinessCoordinatorDelegate {
    func readinessCoordinator(
        _ coordinator: AVCapturePhotoOutputReadinessCoordinator,
        captureReadinessDidChange captureReadiness: AVCapturePhotoOutput.CaptureReadiness
    ) {
        // The pipeline can take another photo: the burst asks for one.
        guard captureReadiness == .ready else { return }
        queue.async { self.captureNext() }
    }
}

/// Each callback reads what it needs on the output's queue and hops to
/// `queue`.
nonisolated extension CameraController: AVCapturePhotoCaptureDelegate {
    func photoOutput(
        _ output: AVCapturePhotoOutput, willCapturePhotoFor resolvedSettings: AVCaptureResolvedPhotoSettings
    ) {
        guard let onShutter else { return }
        DispatchQueue.main.async { MainActor.assumeIsolated { onShutter() } }
    }

    func photoOutput(
        _ output: AVCapturePhotoOutput, didCapturePhotoFor resolvedSettings: AVCaptureResolvedPhotoSettings
    ) {
        // The sensor has the frame: a sweep can ask for the next now; a
        // bracket waits for it to finish (`stepFinished`).
        queue.async { self.captureNext() }
    }

    func photoOutput(
        _ output: AVCapturePhotoOutput, didFinishProcessingPhoto photo: AVCapturePhoto, error: Error?
    ) {
        let landed = LandedPhoto(
            id: photo.resolvedSettings.uniqueID,
            file: error == nil ? photo.fileDataRepresentation() : nil,
            error: error.map { String(describing: $0) },
            timestamp: photo.timestamp,
            dimensions: photo.isRawPhoto
                ? photo.resolvedSettings.rawPhotoDimensions : photo.resolvedSettings.photoDimensions)
        queue.async { self.photoLanded(landed) }
    }

    func photoOutput(
        _ output: AVCapturePhotoOutput, didFinishCaptureFor resolvedSettings: AVCaptureResolvedPhotoSettings,
        error: Error?
    ) {
        let id = resolvedSettings.uniqueID
        queue.async { self.captureFinished(id) }
    }
}
