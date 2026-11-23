@preconcurrency import AVFoundation
import Foundation
import os

private nonisolated let log = AppLog(category: "camera")

/// Setting up the session and camera: access, the photo pipeline, the
/// peaking feed and the lens at rest.
nonisolated extension CameraController {
    func start() async throws {
        switch AVCaptureDevice.authorizationStatus(for: .video) {
        case .authorized: break
        case .notDetermined:
            guard await AVCaptureDevice.requestAccess(for: .video) else { throw SetupError.denied }
        default: throw SetupError.denied
        }
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            queue.async {
                do {
                    if self.camera == nil {
                        try self.configure()
                        self.readRawFormat()
                    }
                    // Started on purpose: earlier runtime errors don't count.
                    self.restarts = []
                    if !self.session.isRunning {
                        self.session.startRunning()
                    }
                    if self.watch == nil, let device = self.device {
                        self.startWatching(device)
                    }
                    // A live interruption stays, said again to the view.
                    if case .interrupted = self.sessionState {
                        self.become(self.sessionState)
                    } else {
                        self.become(.running)
                    }
                    self.prepare()
                    continuation.resume()
                } catch {
                    continuation.resume(throwing: error)
                }
            }
        }
    }

    /// A running burst ends first, canceled, so its completion still runs.
    func stop() {
        queue.async {
            self.end(.canceled)
            self.stopWatching()
            if self.session.isRunning {
                self.session.stopRunning()
            }
            self.become(.stopped)
        }
    }

    /// The ultra-wide where it is the macro lens, else the wide. On the
    /// non-Pro iPhones 11 to 15 the ultra-wide is fixed-focus: neither close
    /// nor able to bracket.
    private static func chooseCamera() -> AVCaptureDevice? {
        let wide = AVCaptureDevice.default(.builtInWideAngleCamera, for: .video, position: .back)
        guard let ultraWide = AVCaptureDevice.default(.builtInUltraWideCamera, for: .video, position: .back)
        else { return wide }
        return ultraWideIsMacro() ? ultraWide : wide ?? ultraWide
    }

    /// It can step its lens and focuses closer than the wide (or no wide
    /// can).
    private static func ultraWideIsMacro() -> Bool {
        guard let ultraWide = AVCaptureDevice.default(.builtInUltraWideCamera, for: .video, position: .back),
              ultraWide.isLockingFocusWithCustomLensPositionSupported
        else { return false }
        // A distance of 0 or less is unknown.
        guard let wide = AVCaptureDevice.default(.builtInWideAngleCamera, for: .video, position: .back),
              wide.isLockingFocusWithCustomLensPositionSupported,
              wide.minimumFocusDistance > 0
        else { return true }
        log.info("ultra-wide focuses to \(ultraWide.minimumFocusDistance) mm, wide to \(wide.minimumFocusDistance) mm")
        return ultraWide.minimumFocusDistance > 0
            && ultraWide.minimumFocusDistance < wide.minimumFocusDistance
    }

    /// By a device type's raw value.
    static func backCamera(_ id: String) -> AVCaptureDevice? {
        AVCaptureDevice.default(AVCaptureDevice.DeviceType(rawValue: id), for: .video, position: .back)
    }

    private func configure() throws {
        let lenses = Self.backLenses()
        let preferred = preferredLens.flatMap { id in
            lenses.contains { $0.id == id } ? Self.backCamera(id) : nil
        }
        guard let device = preferred ?? Self.chooseCamera() else { throw SetupError.noCamera }
        log.info("lenses: \(lenses.map { "\($0.label) \($0.id)" }.joined(separator: ", "), privacy: .public); using \(device.deviceType.rawValue, privacy: .public)\(preferred != nil ? " (preferred)" : "", privacy: .public)")
        session.beginConfiguration()
        defer { session.commitConfiguration() }
        session.sessionPreset = .photo
        guard let input = try? AVCaptureDeviceInput(device: device), session.canAddInput(input),
              session.canAddOutput(output)
        else { throw SetupError.cannotConfigure }
        session.addInput(input)
        session.addOutput(output)
        video.videoSettings = [kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32BGRA]
        video.alwaysDiscardsLateVideoFrames = true
        video.setSampleBufferDelegate(self, queue: videoQueue)
        if session.canAddOutput(video) {
            session.addOutput(video)
        }
        // Where the camera can be shared while multitasking (Stage Manager),
        // keep shooting beside another app. Set before the session starts.
        if session.isMultitaskingCameraAccessSupported {
            session.isMultitaskingCameraAccessEnabled = true
        }
        log.info("camera while multitasking: \(self.session.isMultitaskingCameraAccessEnabled ? "on" : "not supported", privacy: .public)")

        // The stack needs many frames close together more than each one
        // denoised.
        output.maxPhotoQualityPrioritization = .speed
        setZeroShutterLag(zeroShutterLagWanted, configuring: true)

        try setUpDevice(device)
        adopt(device, input: input, lenses: lenses)
        let readiness = AVCapturePhotoOutputReadinessCoordinator(photoOutput: output)
        readiness.delegate = self
        self.readiness = readiness
        observeSession()
    }

    /// Focus locked at rest, automatic exposure (with the bias) and white
    /// balance between bursts, no zoom, torch off. On `queue`.
    func setUpDevice(_ device: AVCaptureDevice) throws {
        let (rest, bias) = lock.withLock { (_restLens, _exposureBias) }
        try device.lockForConfiguration()
        defer { device.unlockForConfiguration() }
        if device.isLockingFocusWithCustomLensPositionSupported {
            device.setFocusModeLocked(lensPosition: rest)
        } else if device.isFocusModeSupported(.locked) {
            device.focusMode = .locked
        }
        if device.isSmoothAutoFocusSupported {
            device.isSmoothAutoFocusEnabled = false
        }
        device.videoZoomFactor = 1
        if device.isExposureModeSupported(.continuousAutoExposure) {
            device.exposureMode = .continuousAutoExposure
        }
        device.setExposureTargetBias(
            min(max(bias, device.minExposureTargetBias), device.maxExposureTargetBias), completionHandler: nil)
        if device.isWhiteBalanceModeSupported(.continuousAutoWhiteBalance) {
            device.whiteBalanceMode = .continuousAutoWhiteBalance
        }
        if device.hasTorch, device.isTorchModeSupported(.off) {
            device.torchMode = .off
        }
    }

    /// Make `device` the camera in use and size the pipeline for it. Inside
    /// the session's configuration, on `queue`; its Bayer RAW format is read
    /// once committed (`readRawFormat`).
    func adopt(_ device: AVCaptureDevice, input: AVCaptureDeviceInput, lenses: [LensChoice]) {
        let id = device.deviceType.rawValue
        let camera = Camera(
            device: device, input: input,
            lens: lenses.first { $0.id == id } ?? LensChoice(id: id, label: Self.magnificationLabel(1)),
            lenses: lenses,
            macro: device.deviceType == .builtInUltraWideCamera && Self.ultraWideIsMacro(),
            rawPixelFormat: nil,
            resolutions: Self.resolutions(of: device),
            biasRange: device.minExposureTargetBias...device.maxExposureTargetBias)
        lock.withLock { _camera = camera }
        let offered = device.activeFormat.supportedMaxPhotoDimensions
            .map { "\($0.width)×\($0.height)" }.joined(separator: ", ")
        log.info("photo sizes offered: \(offered, privacy: .public); default \(self.output.maxPhotoDimensions.width)×\(self.output.maxPhotoDimensions.height)")
        applyResolution(configuring: true)
    }

    /// Once the configuration is committed. On `queue`.
    func readRawFormat() {
        let raw = output.availableRawPhotoPixelFormatTypes
            .first { AVCapturePhotoOutput.isBayerRAWPixelFormat($0) }
        lock.withLock { _camera?.rawPixelFormat = raw }
        log.info("Bayer RAW: \(raw.map { String(format: "%08x", $0) } ?? "none", privacy: .public)")
    }

    /// Zero shutter lag on for sweeps, off for brackets, with responsive and
    /// fast capture following it: a zero-lag photo can predate its request,
    /// in a bracket from before the lens settled. Changing it rebuilds the
    /// pipeline, which is slow, so it changes between bursts
    /// (`setBracketMode`). On `queue`.
    func setZeroShutterLag(_ on: Bool, configuring: Bool = false) {
        zeroShutterLagWanted = on
        let zero = on && output.isZeroShutterLagSupported
        let unchanged = output.isZeroShutterLagEnabled == zero
            && output.isResponsiveCaptureEnabled == (zero && output.isResponsiveCaptureSupported)
        guard configuring || !unchanged else { return }
        if !configuring { session.beginConfiguration() }
        defer { if !configuring { session.commitConfiguration() } }
        if output.isZeroShutterLagSupported {
            output.isZeroShutterLagEnabled = zero
        }
        let responsive = zero && output.isResponsiveCaptureSupported
        if output.isResponsiveCaptureSupported || !responsive {
            output.isResponsiveCaptureEnabled = responsive
        }
        let fast = responsive && output.isFastCapturePrioritizationSupported
        if output.isFastCapturePrioritizationSupported || !fast {
            output.isFastCapturePrioritizationEnabled = fast
        }
        log.info("zero shutter lag \(zero ? "on" : "off", privacy: .public), responsive capture \(responsive ? "on" : "off", privacy: .public)")
    }

    /// Set the pipeline up for the mode chosen, between bursts: brackets
    /// without zero shutter lag, sweeps with it. Call it on a mode change so
    /// the slow change isn't made as a burst begins.
    func setBracketMode(_ bracket: Bool) {
        queue.async {
            guard self.burstIdle else { return }
            // Before the camera is set up: `configure` applies it.
            guard self.device != nil else {
                self.zeroShutterLagWanted = !bracket
                return
            }
            self.setZeroShutterLag(!bracket)
            self.prepare()
        }
    }

    /// 12 MP: the offered size nearest 12 million pixels; 48 MP: the
    /// largest, when at least 40 million.
    static func resolutions(of device: AVCaptureDevice)
        -> [PhotoResolution: CMVideoDimensions]
    {
        let sizes = device.activeFormat.supportedMaxPhotoDimensions
        let pixels = { (d: CMVideoDimensions) in Double(d.width) * Double(d.height) }
        var result: [PhotoResolution: CMVideoDimensions] = [:]
        result[.twelve] = sizes.min { abs(pixels($0) - 12e6) < abs(pixels($1) - 12e6) }
        if let largest = sizes.max(by: { pixels($0) < pixels($1) }), pixels(largest) >= 40e6 {
            result[.fortyEight] = largest
        }
        return result
    }

    /// Or the nearest one offered.
    var burstDimensions: CMVideoDimensions {
        let chosen = lock.withLock { _resolution }
        return resolutions[chosen] ?? resolutions[.twelve] ?? output.maxPhotoDimensions
    }

    /// On `queue`, never during a burst, which keeps its starting size.
    func applyResolution(configuring: Bool = false) {
        guard burstIdle, device != nil else { return }
        let dimensions = burstDimensions
        guard dimensions.width > 0,
              output.maxPhotoDimensions.width != dimensions.width
                || output.maxPhotoDimensions.height != dimensions.height
        else { return }
        if !configuring { session.beginConfiguration() }
        output.maxPhotoDimensions = dimensions
        if !configuring { session.commitConfiguration() }
        log.info("photo size \(dimensions.width)×\(dimensions.height)")
    }

    /// Its connection is set again only when that changes.
    func setPeakingEnabled(_ enabled: Bool) {
        queue.async {
            guard enabled != self.peakingEnabled else { return }
            self.peakingEnabled = enabled
            self.applyPeakingConnection()
        }
    }

    /// Turned as the preview, so the overlay lines up.
    func setPreviewAngle(_ angle: CGFloat) {
        queue.async {
            self.previewAngle = angle
            self.applyPeakingConnection()
        }
    }

    /// Mirrored as the preview: a back camera facing the user (iPhone Duo,
    /// iOS 27.1) shows a mirror image. The photo output is never mirrored:
    /// frames are what the lens saw.
    func setPreviewMirrored(_ mirrored: Bool) {
        queue.async {
            self.previewMirrored = mirrored
            self.applyPeakingConnection()
        }
    }

    /// On or off, turned and mirrored as the preview. A lens change makes a
    /// new connection, so this runs again. On `queue`.
    func applyPeakingConnection() {
        guard let connection = video.connection(with: .video) else { return }
        connection.isEnabled = peakingEnabled
        if let previewAngle, connection.isVideoRotationAngleSupported(previewAngle) {
            connection.videoRotationAngle = previewAngle
        }
        if let previewMirrored, connection.isVideoMirroringSupported {
            connection.automaticallyAdjustsVideoMirroring = false
            connection.isVideoMirrored = previewMirrored
        }
    }

    /// 0 (closest) to 1 (farthest): where the lens returns after each burst,
    /// `previewLens(nil)` and a new camera; sweeps are taken there. Moves it
    /// now unless a burst runs.
    func setRestLens(_ position: Float) {
        let position = min(max(position, 0), 1)
        lock.withLock { _restLens = position }
        queue.async {
            guard self.burstIdle else { return }
            self.moveLensToRest()
        }
    }

    /// On `queue`, between bursts.
    private func moveLensToRest() {
        let rest = lock.withLock { _restLens }
        guard let device, device.isLockingFocusWithCustomLensPositionSupported,
              (try? device.lockForConfiguration()) != nil
        else { return }
        device.setFocusModeLocked(lensPosition: rest)
        device.unlockForConfiguration()
    }

    /// Preview a bracket's far limit between bursts (0 closest, 1
    /// farthest); `nil` puts it back at rest.
    func previewLens(_ position: Float?) {
        queue.async {
            guard self.burstIdle else { return }
            guard let position else {
                self.moveLensToRest()
                return
            }
            guard let device = self.device,
                  device.isLockingFocusWithCustomLensPositionSupported,
                  (try? device.lockForConfiguration()) != nil
            else { return }
            device.setFocusModeLocked(lensPosition: min(max(position, 0), 1))
            device.unlockForConfiguration()
        }
    }

    /// Whether the lens can be stepped for a bracket.
    var canBracket: Bool {
        device?.isLockingFocusWithCustomLensPositionSupported ?? false
    }

    /// For the status pill: its magnification, "macro" after it for the
    /// ultra-wide as macro lens.
    var lensLabel: LocalizedStringResource {
        guard let camera else {
            return LocalizedStringResource(
                "1×", comment: "The lens in the camera screen's status pill before the camera is set up: the main wide lens.")
        }
        return camera.macro
            ? LocalizedStringResource(
                "\(camera.lens.label) macro",
                comment: "The lens in the camera screen's status pill: the ultra-wide, used as the macro lens. The placeholder is its magnification, such as 0.5×.")
            : LocalizedStringResource(
                "\(camera.lens.label)",
                comment: "The lens in the camera screen's status pill. The placeholder is its magnification, such as 1× or 3×; usually left as it is.")
    }

    /// For the camera screen's label.
    var minimumFocusMillimeters: Int? {
        guard let device, device.minimumFocusDistance > 0 else { return nil }
        return device.minimumFocusDistance
    }
}

nonisolated extension CameraController: AVCaptureVideoDataOutputSampleBufferDelegate {
    func captureOutput(
        _ output: AVCaptureOutput, didOutput sampleBuffer: CMSampleBuffer,
        from connection: AVCaptureConnection
    ) {
        guard let pixels = CMSampleBufferGetImageBuffer(sampleBuffer) else { return }
        peaking?.draw(pixels)
    }
}
