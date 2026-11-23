@preconcurrency import AVFoundation
import AVKit
import MediaAccessibility
import SwiftUI
import UIKit

private let cameraLog = AppLog(category: "camera")

/// The session's live picture, kept level as the phone turns and mirrored
/// while the back camera faces the user (iPhone Duo); it gives the
/// controller the rotation coordinator its photos are turned by.
struct CameraPreview: UIViewRepresentable {
    let camera: CameraController
    let peaking: Bool
    /// The picture darkens at each exposure.
    let blinks: Bool
    /// A `CameraController.LensChoice.id`: the preview follows it.
    let lens: String?
    let metered: Metered?

    /// Where, in the preview's coordinates, and which tap.
    struct Metered: Equatable {
        var point: CGPoint
        var tap: Int
    }
    /// Width over height as shown, each time it changes.
    let onPicture: @MainActor (CGFloat) -> Void

    func makeUIView(context: Context) -> PreviewView {
        let view = PreviewView()
        view.camera = camera
        view.onPicture = onPicture
        view.previewLayer.session = camera.session
        view.previewLayer.videoGravity = .resizeAspect
        camera.onShutter = { [weak view] in view?.blink() }
        if let overlay = Peaking() {
            view.layer.addSublayer(overlay.layer)
            view.peaking = overlay
            camera.peaking = overlay
        }
        view.bind()
        return view
    }

    func updateUIView(_ view: PreviewView, context: Context) {
        view.peaking?.layer.isHidden = !peaking
        view.onPicture = onPicture
        view.blinks = blinks
        // Sent only on change: each send reconfigures the feed's connection.
        if view.peakingSent != peaking {
            view.peakingSent = peaking
            camera.setPeakingEnabled(peaking)
        }
        view.bind()
        view.meter(metered)
    }

    final class PreviewView: UIView {
        override class var layerClass: AnyClass { AVCaptureVideoPreviewLayer.self }
        var previewLayer: AVCaptureVideoPreviewLayer { layer as! AVCaptureVideoPreviewLayer }
        weak var camera: CameraController?
        var peaking: Peaking?
        var onPicture: (@MainActor (CGFloat) -> Void)?
        var blinks = true
        /// `nil` before the first ask.
        var peakingSent: Bool?
        private var toldAspect: CGFloat?
        /// The camera's unique id the coordinators below are for.
        private var bound: String?
        private var metered: Metered?
        /// Marks the point metered, for a moment.
        private let meterMark: CAShapeLayer = {
            let layer = CAShapeLayer()
            layer.path = CGPath(
                roundedRect: CGRect(x: -32, y: -32, width: 64, height: 64), cornerWidth: 6, cornerHeight: 6,
                transform: nil)
            layer.fillColor = nil
            layer.strokeColor = UIColor.systemYellow.cgColor
            layer.lineWidth = 1.5
            layer.opacity = 0
            return layer
        }()

        /// Follow the camera in use: its rotation, for preview and photos,
        /// and which way it faces. Remade on a lens change.
        func bind() {
            guard let camera, let device = camera.device, device.uniqueID != bound else { return }
            bound = device.uniqueID
            mirrored = false
            let coordinator = AVCaptureDevice.RotationCoordinator(device: device, previewLayer: previewLayer)
            camera.rotation = coordinator
            follow(coordinator)
            watchDirection(of: device)
        }

        /// Meter at a new tap on the picture and mark it; none meters the
        /// whole picture.
        func meter(_ tap: Metered?) {
            guard tap != metered else { return }
            metered = tap
            guard let tap else {
                camera?.meter(at: nil)
                return
            }
            let picture = previewLayer.layerRectConverted(
                fromMetadataOutputRect: CGRect(x: 0, y: 0, width: 1, height: 1))
            guard picture.contains(tap.point) else { return }
            camera?.meter(at: previewLayer.captureDevicePointConverted(fromLayerPoint: tap.point))
            if meterMark.superlayer == nil { layer.addSublayer(meterMark) }
            CATransaction.begin()
            CATransaction.setDisableActions(true)
            meterMark.position = tap.point
            CATransaction.commit()
            let fade = CAKeyframeAnimation(keyPath: "opacity")
            fade.values = [1, 1, 0]
            fade.keyTimes = [0, 0.8, 1]
            fade.duration = 1.5
            meterMark.add(fade, forKey: "mark")
            if !UIAccessibility.isReduceMotionEnabled {
                let pop = CABasicAnimation(keyPath: "transform.scale")
                pop.fromValue = 1.28
                pop.toValue = 1.0
                pop.duration = 0.18
                pop.timingFunction = CAMediaTimingFunction(name: .easeOut)
                meterMark.add(pop, forKey: "scale")
            }
        }
        private var observation: NSKeyValueObservation?
        /// `AVCaptureDeviceDirectionCoordinator` (iOS 27.1), kept as long as
        /// the preview.
        private var direction: AnyObject?
        /// The back camera faces the user, as it can on iPhone Duo.
        private var mirrored = false
        /// A shutter blink at each exposure. Core Animation, not SwiftUI
        /// state, so blinks at 10 a second or more all show.
        private let blinkLayer: CALayer = {
            let layer = CALayer()
            layer.backgroundColor = UIColor.black.cgColor
            layer.opacity = 0
            return layer
        }()

        /// Only for a bracket (`blinks`): a sweep would flash 5 to 15 times a
        /// second, so its count climbs instead. Not with Reduce Motion or
        /// Dim Flashing Lights, asked each frame so a change counts at once.
        func blink() {
            guard blinks, !UIAccessibility.isReduceMotionEnabled, !MADimFlashingLightsEnabled() else { return }
            let animation = CABasicAnimation(keyPath: "opacity")
            animation.fromValue = 0.5
            animation.toValue = 0
            animation.duration = 0.07
            blinkLayer.add(animation, forKey: "blink")
        }

        override func layoutSubviews() {
            super.layoutSubviews()
            if blinkLayer.superlayer == nil {
                layer.addSublayer(blinkLayer)
            }
            CATransaction.begin()
            CATransaction.setDisableActions(true)
            blinkLayer.frame = bounds
            CATransaction.commit()
            placeOverlay()
        }

        /// The overlay exactly over the picture shown, and the picture's
        /// shape told to the layout on change, so the layout fits the
        /// picture as it is, not a guess from the screen.
        private func placeOverlay() {
            let picture = previewLayer.layerRectConverted(
                fromMetadataOutputRect: CGRect(x: 0, y: 0, width: 1, height: 1))
            if let overlay = peaking?.layer {
                CATransaction.begin()
                CATransaction.setDisableActions(true)
                overlay.frame = picture
                CATransaction.commit()
            }
            guard picture.width > 1, picture.height > 1, picture.width.isFinite, picture.height.isFinite
            else { return }
            let aspect = ((picture.width / picture.height) * 100).rounded() / 100
            guard aspect != toldAspect else { return }
            toldAspect = aspect
            let angle = previewLayer.connection?.videoRotationAngle ?? 0
            cameraLog.info(
                "camera picture: \(Int(picture.width))×\(Int(picture.height)) in \(Int(self.bounds.width))×\(Int(self.bounds.height)), turned \(Int(angle))°")
            // After this layout pass: it changes the layout's state.
            Task { @MainActor [weak self] in self?.onPicture?(aspect) }
        }

        func follow(_ coordinator: AVCaptureDevice.RotationCoordinator) {
            apply(coordinator.videoRotationAngleForHorizonLevelPreview)
            observation = coordinator.observe(
                \.videoRotationAngleForHorizonLevelPreview, options: [.new]
            ) { [weak self] _, change in
                guard let angle = change.newValue else { return }
                Task { @MainActor in self?.apply(angle) }
            }
        }

        private func apply(_ angle: CGFloat) {
            guard let connection = previewLayer.connection,
                  connection.isVideoRotationAngleSupported(angle)
            else { return }
            connection.videoRotationAngle = angle
            camera?.setPreviewAngle(angle)
            setNeedsLayout()
        }

        /// Mirror the picture and peaking while `device` (a back camera)
        /// faces the user, as an iPhone Duo's can. Before iOS 27.1, and
        /// until the coordinator first reports, the picture is as the lens
        /// sees it.
        func watchDirection(of device: AVCaptureDevice) {
            guard #available(iOS 27.1, *) else { return }
            let id = device.uniqueID
            // The handler runs on the main queue.
            direction = AVCaptureDeviceDirectionCoordinator(
                view: self, deviceTypes: [device.deviceType]
            ) { [weak self] directions in
                let facing = directions.forwardFacingDeviceDescriptors.contains { $0.uniqueID == id }
                MainActor.assumeIsolated { self?.mirror(facing) }
            }
        }

        private func mirror(_ facing: Bool) {
            guard facing != mirrored, let connection = previewLayer.connection,
                  connection.isVideoMirroringSupported
            else { return }
            mirrored = facing
            cameraLog.info("back camera \(facing ? "faces the user: preview mirrored" : "faces away: preview as the lens sees it", privacy: .public)")
            connection.automaticallyAdjustsVideoMirroring = false
            connection.isVideoMirrored = facing
            camera?.setPreviewMirrored(facing)
            setNeedsLayout()
        }
    }
}
