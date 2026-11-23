@preconcurrency import AVFoundation
import Foundation
import os

private nonisolated let log = AppLog(category: "camera")

nonisolated extension CameraController {
    /// In order of magnification, labeled with it.
    static func backLenses() -> [LensChoice] {
        let devices = AVCaptureDevice.DiscoverySession(
            deviceTypes: [.builtInUltraWideCamera, .builtInWideAngleCamera, .builtInTelephotoCamera],
            mediaType: .video, position: .back
        ).devices
        let wide = devices.first { $0.deviceType == .builtInWideAngleCamera }
        let virtuals = AVCaptureDevice.DiscoverySession(
            deviceTypes: [.builtInTripleCamera, .builtInDualWideCamera, .builtInDualCamera],
            mediaType: .video, position: .back
        ).devices
        return devices
            .map { ($0, magnification(of: $0, against: wide, virtuals: virtuals)) }
            .sorted { $0.1 < $1.1 }
            .map { LensChoice(id: $0.0.deviceType.rawValue, label: magnificationLabel($0.1)) }
    }

    /// `device`'s magnification against the wide: best from a virtual
    /// device's switch-over zoom factors (each over the wide's, the first
    /// being 1), else the ratio of the tangents of the half fields of view,
    /// else by type.
    static func magnification(
        of device: AVCaptureDevice, against wide: AVCaptureDevice?, virtuals: [AVCaptureDevice]
    ) -> Double {
        guard let wide, device.deviceType != wide.deviceType else { return 1 }
        for virtual in virtuals {
            let cameras = virtual.constituentDevices.map(\.deviceType)
            let factors = virtual.virtualDeviceSwitchOverVideoZoomFactors.map(\.doubleValue)
            guard factors.count == cameras.count - 1,
                  let at = cameras.firstIndex(of: device.deviceType),
                  let wideAt = cameras.firstIndex(of: wide.deviceType)
            else { continue }
            let zoom = { (index: Int) in index == 0 ? 1.0 : factors[index - 1] }
            guard zoom(at) > 0, zoom(wideAt) > 0 else { continue }
            return zoom(at) / zoom(wideAt)
        }
        let wideView = Double(wide.activeFormat.videoFieldOfView)
        let view = Double(device.activeFormat.videoFieldOfView)
        if wideView > 0, view > 0 {
            let halfTangent = { (degrees: Double) in tan(degrees * .pi / 360) }
            return halfTangent(wideView) / halfTangent(view)
        }
        log.info("magnification of \(device.deviceType.rawValue, privacy: .public) unknown; going by its type")
        return device.deviceType == .builtInUltraWideCamera ? 0.5 : 2
    }

    /// To a tenth: "0.5×", "1×", "2.5×".
    static func magnificationLabel(_ magnification: Double) -> String {
        let rounded = (magnification * 10).rounded() / 10
        return rounded.formatted(.number.precision(.fractionLength(0...1))) + "×"
    }

    /// Shoot with another back camera between bursts; ignored during one or
    /// for an id unknown or in use. The session is reconfigured at once,
    /// then `onLensChange` runs. A camera that can't be added leaves the old
    /// one.
    func setLens(_ id: String) {
        queue.async {
            guard self.burstIdle else {
                log.debug("lens not changed: a burst is running")
                return
            }
            guard let old = self.camera, id != old.lens.id, old.lenses.contains(where: { $0.id == id }),
                  let device = Self.backCamera(id)
            else {
                log.debug("lens not changed: \(id, privacy: .public) is in use or unknown")
                return
            }
            let watching = self.watch != nil
            self.stopWatching()
            // The torch is the old camera's; it goes off with it.
            if old.device.hasTorch, (try? old.device.lockForConfiguration()) != nil {
                old.device.torchMode = .off
                old.device.unlockForConfiguration()
            }
            let changed = self.switchInput(to: device, from: old)
            if changed {
                self.readRawFormat()
            }
            // A new input makes new connections: the peaking settings go on
            // again.
            self.applyPeakingConnection()
            if watching, let now = self.device {
                self.startWatching(now)
            }
            self.prepare()
            guard changed else { return }
            log.info("lens changed: \(old.lens.id, privacy: .public) → \(id, privacy: .public)")
            guard let onLensChange = self.onLensChange else { return }
            DispatchQueue.main.async { MainActor.assumeIsolated { onLensChange() } }
        }
    }

    /// Swap `old`'s input for `device`'s in one configuration, putting the
    /// old back if it can't be added. Whether it changed. On `queue`.
    private func switchInput(to device: AVCaptureDevice, from old: Camera) -> Bool {
        session.beginConfiguration()
        defer { session.commitConfiguration() }
        session.removeInput(old.input)
        guard let input = try? AVCaptureDeviceInput(device: device), session.canAddInput(input) else {
            log.error("lens \(device.deviceType.rawValue, privacy: .public) couldn't be added; keeping \(old.lens.id, privacy: .public)")
            if session.canAddInput(old.input) {
                session.addInput(old.input)
            } else {
                log.error("the old lens couldn't be put back either")
            }
            return false
        }
        session.addInput(input)
        setZeroShutterLag(zeroShutterLagWanted, configuring: true)
        do {
            try setUpDevice(device)
        } catch {
            log.error("new lens not set up: \(String(describing: error), privacy: .public)")
        }
        adopt(device, input: input, lenses: old.lenses)
        return true
    }
}
