@preconcurrency import AVFoundation
import Foundation
import os

private nonisolated let log = AppLog(category: "camera")

nonisolated extension CameraController {
    /// At the least.
    private static let exposureReportInterval: TimeInterval = 0.25
    /// The video frame rate while the system is under serious pressure.
    private static let pressureFrameRate: Int32 = 20

    /// Meter continuously at `point` (device coordinates, 0 to 1, as the
    /// preview layer converts a touch); `nil` meters the whole frame.
    /// Ignored during a burst.
    func meter(at point: CGPoint?) {
        queue.async {
            guard self.burstIdle else {
                log.debug("exposure point not changed: a burst is running")
                return
            }
            guard let device = self.device, (try? device.lockForConfiguration()) != nil else { return }
            defer { device.unlockForConfiguration() }
            if device.isExposurePointOfInterestSupported {
                // The default point, in the middle, with its default area.
                device.exposurePointOfInterest = point.map {
                    CGPoint(x: min(max($0.x, 0), 1), y: min(max($0.y, 0), 1))
                } ?? CGPoint(x: 0.5, y: 0.5)
            }
            // Setting the mode is what applies the point.
            if device.isExposureModeSupported(.continuousAutoExposure) {
                device.exposureMode = .continuousAutoExposure
            }
        }
    }

    /// Bias by `ev` stops within `exposureBiasRange`, held through a burst
    /// and across lenses. Ignored during a burst.
    func setExposureBias(_ ev: Float) {
        queue.async {
            guard self.burstIdle else {
                log.debug("exposure bias not changed: a burst is running")
                return
            }
            self.lock.withLock { self._exposureBias = ev }
            guard let device = self.device, (try? device.lockForConfiguration()) != nil else { return }
            defer { device.unlockForConfiguration() }
            device.setExposureTargetBias(
                min(max(ev, device.minExposureTargetBias), device.maxExposureTargetBias), completionHandler: nil)
        }
    }

    /// Watch exposure (for `onExposure`) and system pressure, replacing any
    /// camera watched before. On `queue`.
    func startWatching(_ device: AVCaptureDevice) {
        stopWatching()
        let watch = DeviceWatch(device: device)
        self.watch = watch
        let exposureChanged: @Sendable () -> Void = { [weak self] in
            guard let self else { return }
            queue.async { self.exposureChanged(watch) }
        }
        watch.observations = [
            device.observe(\.exposureDuration, options: [.initial]) { _, _ in exposureChanged() },
            device.observe(\.iso) { _, _ in exposureChanged() },
            device.observe(\.systemPressureState, options: [.initial]) { [weak self] _, _ in
                guard let self else { return }
                queue.async { self.pressureChanged(watch) }
            },
        ]
    }

    /// The frame rate back to its own if pressure capped it. On `queue`.
    func stopWatching() {
        guard let watch else { return }
        self.watch = nil
        watch.observations.forEach { $0.invalidate() }
        watch.observations = []
        if watch.capped, (try? watch.device.lockForConfiguration()) != nil {
            watch.device.activeVideoMinFrameDuration = .invalid
            watch.device.unlockForConfiguration()
        }
    }

    /// Report at most every `exposureReportInterval`. On `queue`.
    private func exposureChanged(_ watch: DeviceWatch) {
        guard self.watch === watch, !watch.reportWaiting else { return }
        watch.reportWaiting = true
        let due = watch.sentAt + Self.exposureReportInterval
        queue.asyncAfter(deadline: max(due, .now())) { self.reportExposure(watch) }
    }

    /// If it changed enough to show. On `queue`.
    private func reportExposure(_ watch: DeviceWatch) {
        watch.reportWaiting = false
        guard self.watch === watch else { return }
        let duration = CMTimeGetSeconds(watch.device.exposureDuration)
        guard duration.isFinite, duration > 0 else { return }
        let exposure = Exposure(duration: duration, iso: watch.device.iso)
        guard watch.sent.map({ exposure.differs(from: $0) }) ?? true else { return }
        watch.sent = exposure
        watch.sentAt = .now()
        guard let onExposure else { return }
        DispatchQueue.main.async { MainActor.assumeIsolated { onExposure(exposure) } }
    }

    /// Under serious system pressure (heat, a weak battery) the video the
    /// preview and peaking show is capped at `pressureFrameRate`, as the
    /// camera advises; restored once it falls. On `queue`.
    private func pressureChanged(_ watch: DeviceWatch) {
        guard self.watch === watch else { return }
        let device = watch.device
        let level = device.systemPressureState.level
        let high = level == .serious || level == .critical || level == .shutdown
        guard high != watch.capped else { return }
        let rate = Self.pressureFrameRate
        let cap = CMTime(value: 1, timescale: rate)
        if high {
            let supported = device.activeFormat.videoSupportedFrameRateRanges.contains {
                $0.minFrameRate <= Double(rate) && Double(rate) <= $0.maxFrameRate
            }
            guard supported, !device.isAutoVideoFrameRateEnabled else {
                log.info("system pressure \(level.rawValue, privacy: .public): video rate can't be capped at \(rate) fps")
                return
            }
            // Already that slow: nothing to cap.
            guard CMTimeCompare(device.activeVideoMinFrameDuration, cap) < 0 else { return }
        }
        guard (try? device.lockForConfiguration()) != nil else { return }
        device.activeVideoMinFrameDuration = high ? cap : .invalid
        device.unlockForConfiguration()
        watch.capped = high
        log.info("system pressure \(level.rawValue, privacy: .public): video \(high ? "capped at \(rate) fps" : "back to its full rate", privacy: .public)")
    }

    /// On `queue` only.
    final class DeviceWatch: @unchecked Sendable {
        let device: AVCaptureDevice
        var observations: [NSKeyValueObservation] = []
        /// With when.
        var sent: Exposure?
        var sentAt = DispatchTime(uptimeNanoseconds: 0)
        var reportWaiting = false
        /// Capped for system pressure.
        var capped = false

        init(device: AVCaptureDevice) {
            self.device = device
        }
    }
}

nonisolated extension CameraController.Exposure {
    /// The shutter time or ISO by more than 3%.
    fileprivate func differs(from other: Self) -> Bool {
        abs(duration / other.duration - 1) > 0.03 || abs(Double(iso / max(other.iso, 1)) - 1) > 0.03
    }
}
