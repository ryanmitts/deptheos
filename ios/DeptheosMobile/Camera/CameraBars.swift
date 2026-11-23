@preconcurrency import AVFoundation
import SwiftUI
import UIKit

/// The camera screen's bars: the top bar, the bottom bar and the bracket's
/// controls.
extension CameraView {
    /// Close, exposure, focus peaking and the light; the status has the line
    /// under them to itself.
    var topBar: some View {
        VStack(spacing: 10) {
            GlassEffectContainer(spacing: 10) {
                HStack(spacing: 10) {
                    Button {
                        dismiss()
                    } label: {
                        Image(systemName: "xmark")
                            .font(.body.weight(.semibold))
                            .frame(width: Self.symbolSide, height: Self.symbolSide)
                    }
                    .buttonStyle(.glass)
                    .buttonBorderShape(.circle)
                    .disabled(busy)
                    .accessibilityLabel(
                        Text("Close", comment: "VoiceOver label of the camera screen's close button."))
                    Spacer()
                    if isFailed {
                        // Only the way out.
                    } else {
                        exposureMenu
                        Button {
                            peakingOn.toggle()
                        } label: {
                            Image(systemName: "scope")
                                .font(.body)
                                .foregroundStyle(peakingOn ? .white : .white.opacity(0.75))
                                .frame(width: Self.symbolSide, height: Self.symbolSide)
                        }
                        .buttonStyle(.glass(peakingOn ? .regular.tint(.red) : .regular))
                        .buttonBorderShape(.circle)
                        .accessibilityLabel(
                            Text("Focus peaking", comment: "VoiceOver label of the camera screen's switch for focus peaking."))
                        .accessibilityValue(
                            peakingOn
                                ? Text("On", comment: "VoiceOver value of a camera screen switch (focus peaking, the light) that is on.")
                                : Text("Off", comment: "VoiceOver value of a camera screen switch (focus peaking, the light) that is off."))
                        if camera.hasTorch {
                            // A tap turns it on or off; held, how bright.
                            Menu {
                                Picker(
                                    LocalizedStringResource(
                                        "Brightness", comment: "The camera screen's menu of how bright the light (torch) is."),
                                    selection: $torchLevel
                                ) {
                                    ForEach(Self.torchLevels, id: \.self) { level in
                                        Text(level.formatted(.percent)).tag(level)
                                    }
                                }
                            } label: {
                                Image(systemName: torchOn ? "flashlight.on.fill" : "flashlight.off.fill")
                                    .font(.body)
                                    .foregroundStyle(torchOn ? .white : .white.opacity(0.75))
                                    .frame(width: Self.symbolSide, height: Self.symbolSide)
                            } primaryAction: {
                                torchOn.toggle()
                            }
                            .buttonStyle(.glass(torchOn ? .regular.tint(.yellow) : .regular))
                            .buttonBorderShape(.circle)
                            // Exposure is locked through a burst: the light stays.
                            .disabled(busy)
                            .onChange(of: torchOn) { camera.setTorch(torchOn ? Float(torchLevel) : 0) }
                            .onChange(of: torchLevel) {
                                torchOn = true
                                camera.setTorch(Float(torchLevel))
                            }
                            .accessibilityLabel(
                                Text("Light", comment: "VoiceOver label of the camera screen's switch for the torch."))
                            .accessibilityValue(
                                torchOn
                                    ? Text("On", comment: "VoiceOver value of a camera screen switch (focus peaking, the light) that is on.")
                                    : Text("Off", comment: "VoiceOver value of a camera screen switch (focus peaking, the light) that is off."))
                        }
                    }
                }
            }
            if !isFailed {
                statusPill
                if phase == .ready, session == .running, let exposure {
                    exposureReadout(exposure)
                        .transition(.opacity)
                }
            }
        }
        .animation(reduceMotion ? nil : .snappy(duration: 0.2), value: phase == .ready && session == .running && exposure != nil)
    }

    /// Ready, on a camera with more than one lens, it is the lens menu.
    @ViewBuilder private var statusPill: some View {
        let text = Text(status)
            .font(.callout.monospacedDigit())
            .lineLimit(1)
            .minimumScaleFactor(0.8)
            .contentTransition(.numericText())
        if phase == .ready, camera.lenses.count > 1 {
            Menu {
                Picker(
                    LocalizedStringResource("Lens", comment: "The camera screen's menu of the back cameras (0.5×, 1×, 3×)."),
                    selection: Binding(
                        get: { camera.lens?.id ?? "" },
                        set: { id in
                            lensID = id
                            metering = .whole
                            camera.setLens(id)
                        })
                ) {
                    ForEach(camera.lenses) { lens in
                        Text(verbatim: lens.label).tag(lens.id)
                    }
                }
            } label: {
                HStack(spacing: 5) {
                    text
                    Image(systemName: "chevron.up.chevron.down")
                        .font(.caption2.weight(.semibold))
                        .foregroundStyle(.secondary)
                }
                .padding(.horizontal, 4)
                .frame(minHeight: Self.symbolSide)
            }
            .buttonStyle(.glass)
            .buttonBorderShape(.capsule)
            .accessibilityLabel(Text("Lens", comment: "The camera screen's menu of the back cameras (0.5×, 1×, 3×)."))
            .accessibilityValue(Text(status))
        } else {
            text
                .padding(.horizontal, 14)
                .padding(.vertical, 8)
                .glassEffect(.regular, in: .capsule)
        }
    }

    /// Exposure compensation, and back to metering the whole picture.
    private var exposureMenu: some View {
        let active = exposureBias != 0 || metering != .whole
        return Menu {
            Picker(
                LocalizedStringResource(
                    "Exposure", comment: "The camera screen's menu of exposure compensation, in EV."),
                selection: $exposureBias
            ) {
                ForEach(Self.biases.filter(biasRange.contains), id: \.self) { bias in
                    Text(Self.biasLabel(bias)).tag(bias)
                }
            }
            if metering != .whole {
                Button(LocalizedStringResource(
                    "Meter the Whole Picture",
                    comment: "In the camera screen's Exposure menu after a tap metered a point: meters the whole picture again.")
                ) {
                    metering = .whole
                }
            }
        } label: {
            Image(systemName: "plusminus")
                .font(.body)
                .foregroundStyle(active ? .white : .white.opacity(0.75))
                .frame(width: Self.symbolSide, height: Self.symbolSide)
        }
        .buttonStyle(.glass(active ? .regular.tint(.yellow) : .regular))
        .buttonBorderShape(.circle)
        .disabled(busy)
        .accessibilityLabel(
            Text("Exposure", comment: "The camera screen's menu of exposure compensation, in EV."))
        .accessibilityValue(Text(Self.biasLabel(exposureBias)))
    }

    var biasRange: ClosedRange<Double> {
        let range = camera.exposureBiasRange
        return Double(range.lowerBound)...Double(range.upperBound)
    }

    private static func biasLabel(_ bias: Double) -> LocalizedStringResource {
        LocalizedStringResource(
            "\(bias.formatted(.number.precision(.fractionLength(0...1)).sign(strategy: .always(includingZero: false)))) EV",
            comment: "Exposure compensation in the camera screen's Exposure menu. The placeholder is the signed amount ('+0.5', '0', '−1').")
    }

    /// A sweep, taken by hand, warns of a shutter slow enough to blur.
    private func exposureReadout(_ exposure: CameraController.Exposure) -> some View {
        let slow = mode == .sweep && exposure.duration > Self.slowShutter
        return HStack(spacing: 4) {
            if slow {
                Image(systemName: "exclamationmark.triangle.fill")
            }
            Text(
                "\(Self.shutterLabel(exposure.duration)) s · ISO \(Int(exposure.iso.rounded()))",
                comment: "The camera's exposure readout. The placeholders are the shutter time in seconds ('1/60') and the ISO number.")
        }
        .font(.footnote.monospacedDigit())
        .foregroundStyle(slow ? .yellow : .white)
        .contentTransition(.numericText())
        .padding(.horizontal, 12)
        .padding(.vertical, 5)
        .glassEffect(.regular, in: .capsule)
        .accessibilityElement(children: .combine)
        .accessibilityLabel(
            slow
                ? Text("Slow shutter", comment: "VoiceOver label of the camera's exposure readout when a sweep's shutter time is slow enough to blur.")
                : Text("Exposure", comment: "The camera screen's menu of exposure compensation, in EV."))
    }

    /// A sweep's shutter time above this blurs in the hand.
    private static let slowShutter = 1.0 / 30

    /// As photographers write it: a fraction under a second.
    private static func shutterLabel(_ seconds: Double) -> String {
        seconds > 0 && seconds < 1
            ? "1/\(Int((1 / seconds).rounded()))"
            : seconds.formatted(.number.precision(.fractionLength(0...1)))
    }

    /// For the preview to meter and mark.
    var meteredPoint: CameraPreview.Metered? {
        if case .point(let point, let tap) = metering { .init(point: point, tap: tap) } else { nil }
    }

    var restLens: Float {
        mode == .bracket ? Float(bracketNear) : 0
    }

    /// The glass around it makes a finger's width of it.
    private static let symbolSide: CGFloat = 28

    var busy: Bool { phase.isBusy }

    /// The camera is ready and not paused or stopped.
    var canShoot: Bool { phase == .ready && session.message == nil }

    /// Nearest first. A step past the farthest (100%) is dropped, not
    /// retaken there, so the bracket can be shorter than its Frames; the
    /// sliders' ranges (first at most 90%, step at least 0.25%) leave at
    /// least two.
    var bracketPositions: [Float] {
        let (near, step) = (Float(bracketNear), Float(bracketStep))
        return (0..<max(bracketFrames, 2)).map { near + step * Float($0) }.filter { $0 <= 1.0001 }.map { min(1, $0) }
    }

    private static func percent(_ value: Double) -> String {
        value.formatted(.percent.precision(.fractionLength(0...2)))
    }

    private static func delayLabel(_ seconds: Int) -> LocalizedStringResource {
        seconds == 0
            ? LocalizedStringResource(
                "No delay", comment: "A choice in the bracket's Delay menu: the bracket starts at once.")
            : LocalizedStringResource(
                "\(seconds) seconds",
                comment: "A choice in the bracket's Delay menu. The placeholder is the seconds waited (2 or 5).")
    }

    private static func rateLabel(_ rate: Int) -> LocalizedStringResource {
        rate == 0
            ? LocalizedStringResource(
                "Max fps", comment: "Burst rate in the camera screen's menu: as many frames a second as the camera can take.")
            : LocalizedStringResource(
                "\(rate) fps", comment: "Burst rate in the camera screen's menu. The placeholder is frames a second.")
    }

    private var status: UserText {
        switch phase {
        case .starting:
            .app(LocalizedStringResource(
                "Starting camera", comment: "Camera status pill while the camera starts."))
        case .ready where session == .failed, .declined where session == .failed:
            .app(LocalizedStringResource(
                "Camera stopped", comment: "Camera status pill after the camera stopped with an error."))
        case .ready where session.message != nil, .declined where session.message != nil:
            .app(LocalizedStringResource(
                "Camera paused", comment: "Camera status pill while the camera is interrupted (a call, another app, heat)."))
        case .ready, .declined:
            if let mm = camera.minimumFocusMillimeters {
                .app(LocalizedStringResource(
                    "\(camera.lensLabel) · focus from \(Measurement(value: Double(mm), unit: UnitLength.millimeters).formatted(.measurement(width: .abbreviated, usage: .asProvided)))",
                    comment: "Camera status pill when ready. The placeholders are the lens ('0.5× macro') and the closest focus distance with its unit ('20 mm')."))
            } else {
                .app(camera.lensLabel)
            }
        case .counting:
            .app(LocalizedStringResource(
                "Hold still", comment: "Camera status pill while a bracket's delay counts down before its first frame."))
        case .canceling:
            .app(LocalizedStringResource(
                "Canceling", comment: "Camera status pill while a canceled bracket's last photos land."))
        case .partial(_, let taken, let planned?, _):
            .app(LocalizedStringResource(
                "\(taken) of \(planned)",
                comment: "Camera status pill while a bracket is taken. The placeholders are the frames taken so far and how many the bracket takes."))
        case .partial(_, let taken, nil, _):
            .app(framesCount(taken))
        case .capturing(let frames) where mode == .bracket:
            .app(LocalizedStringResource(
                "\(frames) of \(bracketPositions.count)",
                comment: "Camera status pill while a bracket is taken. The placeholders are the frames taken so far and how many the bracket takes."))
        case .capturing(let frames):
            frames == 0
                ? .app(LocalizedStringResource(
                    "No frames", comment: "Camera status pill as a sweep starts, before its first frame is saved."))
                : .app(framesCount(frames))
        case .saving(let frames):
            .app(LocalizedStringResource(
                "Saving \(frames) frames",
                comment: "Camera status pill while a sweep's frames are saved. The placeholder is how many."))
        case .failed:
            .app(LocalizedStringResource(
                "Camera unavailable", comment: "Camera status pill when the camera couldn't start or a burst failed."))
        }
    }

    /// The step, the hint, the mode and stack menu, then the shutter.
    var bottomBar: some View {
        VStack(spacing: 12) {
            stepAndHint
            // Side by side where they fit, else one above the other.
            GlassEffectContainer(spacing: 12) {
                ViewThatFits(in: .horizontal) {
                    HStack(spacing: 12) {
                        modePicker
                        stackMenu
                    }
                    VStack(spacing: 8) {
                        modePicker
                        stackMenu
                    }
                }
            }
            .disabled(busy)
            shutter
        }
        .animation(reduceMotion ? nil : .snappy(duration: 0.22), value: mode)
    }

    /// The same for a narrow column: mode and stack menu stacked beside the
    /// shutter.
    var columnControls: some View {
        VStack(spacing: 10) {
            stepAndHint
            HStack(spacing: 12) {
                GlassEffectContainer(spacing: 8) {
                    VStack(spacing: 8) {
                        modePicker
                        stackMenu
                    }
                }
                .disabled(busy)
                shutter
            }
        }
        .animation(reduceMotion ? nil : .snappy(duration: 0.22), value: mode)
    }

    @ViewBuilder private var stepAndHint: some View {
        if mode == .bracket, camera.canBracket {
            bracketControls
                .opacity(busy ? 0.4 : 1)
                .disabled(busy)
                .transition(.opacity.combined(with: .scale(scale: 0.96, anchor: .bottom)))
        }
        Text(hint)
            .font(.footnote)
            .foregroundStyle(.white)
            .multilineTextAlignment(.center)
            .padding(.horizontal, 12)
            .padding(.vertical, 5)
            .glassEffect(.regular, in: .capsule)
            .opacity(showsHint ? 1 : 0)
            .animation(reduceMotion ? nil : .snappy(duration: 0.2), value: showsHint)
    }

    private var modePicker: some View {
        Picker(
            LocalizedStringResource(
                "Camera mode", comment: "The camera screen's choice between Sweep and Bracket (read by VoiceOver)."),
            selection: $mode
        ) {
            ForEach(CaptureMode.allCases) { mode in
                Text(mode.label).tag(mode)
            }
        }
        .pickerStyle(.segmented)
        .frame(maxWidth: 240)
    }

    private var shutter: some View {
        Shutter(face: shutterFace)
            .opacity(canShoot || busy ? 1 : 0.4)
            .gesture(
                DragGesture(minimumDistance: 0)
                    .updating($touching) { _, touching, _ in touching = true })
            // VoiceOver, Voice Control and switches can't hold: one
            // activation starts a sweep, the next ends it.
            .accessibilityElement()
            .accessibilityAddTraits(.isButton)
            .accessibilityLabel(shutterLabel)
            .accessibilityValue(phase.frames.map { Text(framesCount($0)) } ?? Text(verbatim: ""))
            .accessibilityHint(
                mode == .sweep
                    ? Text(
                        "Starts a sweep; activate again to end it.",
                        comment: "VoiceOver hint of the shutter in Sweep mode.")
                    : Text(
                        "Takes a bracket: one photo at each focus step.",
                        comment: "VoiceOver hint of the shutter in Bracket mode."))
            .accessibilityAction {
                if phase.isCapturing, mode == .sweep { release() } else { press() }
            }
    }

    /// The delay's seconds, the sweep's frames, or a bracket's progress.
    private var shutterFace: Shutter.Face {
        switch phase {
        case .counting(let left): .counting(left)
        case .capturing(let frames) where mode == .bracket:
            .bracket(taken: frames, planned: bracketPositions.count)
        case .capturing(let frames): .sweep(frames: frames)
        case .saving, .canceling, .partial: .saving
        case .starting, .ready, .declined, .failed: .idle
        }
    }

    private var shutterLabel: Text {
        switch phase {
        case .counting where mode == .bracket, .capturing where mode == .bracket:
            Text("Cancel Bracket", comment: "VoiceOver label of the camera's shutter while a bracket counts down or is taken.")
        case .capturing:
            Text("End Sweep", comment: "VoiceOver label of the camera's shutter while a sweep is taken.")
        default:
            Text("Shutter", comment: "VoiceOver label of the camera screen's shutter button.")
        }
    }

    /// A sweep's rate, the photo format and size, and the algorithm.
    private var stackMenu: some View {
        Menu {
            if mode == .sweep {
                Picker(
                    LocalizedStringResource(
                        "Burst Rate", comment: "The camera screen's menu of how many frames a second a sweep takes."),
                    selection: $framesPerSecond
                ) {
                    ForEach(Self.rates, id: \.self) { rate in
                        Text(Self.rateLabel(rate)).tag(rate)
                    }
                }
            }
            if camera.canCaptureRaw {
                Picker(
                    LocalizedStringResource(
                        "Format", comment: "The camera screen's menu of photo formats (HEIC or RAW)."),
                    selection: $format
                ) {
                    ForEach(PhotoFormat.allCases) { format in
                        Text(format.label).tag(format)
                    }
                }
            }
            if camera.resolutions.count > 1, shownFormat == .heic {
                Picker(
                    LocalizedStringResource(
                        "Photo Size", comment: "The camera screen's menu of photo sizes (12 MP, 48 MP)."),
                    selection: $resolution
                ) {
                    ForEach(PhotoResolution.allCases) { resolution in
                        Text(resolution.label).tag(resolution)
                    }
                }
            }
            Picker(
                LocalizedStringResource(
                    "Stack With", comment: "The camera screen's menu of the stacking method a burst is stacked with."),
                selection: $algorithm
            ) {
                ForEach(AlgorithmKind.allCases) { kind in
                    Text(kind.label).tag(kind)
                }
            }
        } label: {
            Text(UserText.line([.app(shownSize), .app(algorithm.label)]))
                .font(.callout.monospacedDigit())
                .lineLimit(1)
                .fixedSize()
                .padding(.horizontal, 2)
                .frame(height: 32)
        }
        .buttonStyle(.glass)
        .buttonBorderShape(.capsule)
    }

    /// The chosen one if this camera offers it.
    var shownResolution: PhotoResolution {
        shownFormat == .raw || camera.resolutions[resolution] == nil ? .twelve : resolution
    }

    /// For the stack menu's label.
    private var shownSize: LocalizedStringResource {
        shownFormat == .raw
            ? LocalizedStringResource(
                "\(shownResolution.rawValue) MP RAW",
                comment: "The camera screen's stack menu label for RAW photos. The placeholder is the size in megapixels (12).")
            : shownResolution.label
    }

    /// RAW only if this camera has it.
    var shownFormat: PhotoFormat {
        camera.canCaptureRaw ? format : .heic
    }

    /// Ready, or while a bracket the shutter cancels counts down or runs.
    private var showsHint: Bool {
        switch phase {
        case .ready, .counting: true
        case .capturing: mode == .bracket
        case .starting, .saving, .canceling, .partial, .declined, .failed: false
        }
    }

    private var hint: LocalizedStringResource {
        if let message = session.message { return message }
        if phase.isCounting || phase.isCapturing {
            return LocalizedStringResource(
                "Tap the shutter to cancel.",
                comment: "Hint on the camera screen while a bracket counts down or is taken.")
        }
        switch previewing {
        case .first:
            return LocalizedStringResource(
                "Showing the first frame's focus.",
                comment: "Hint on the camera screen while the bracket's first-frame slider is dragged and the lens shows the first frame's focus.")
        case .last:
            return LocalizedStringResource(
                "Showing the last frame's focus.",
                comment: "Hint on the camera screen while the bracket's step slider is dragged and the lens shows the last frame's focus.")
        case nil: break
        }
        switch mode {
        case .sweep:
            return LocalizedStringResource(
                "Hold the shutter and move slowly toward the subject.",
                comment: "Hint on the camera screen in Sweep mode.")
        case .bracket where !camera.canBracket:
            return LocalizedStringResource(
                "This camera can't step its focus. Use Sweep instead.",
                comment: "Hint on the camera screen in Bracket mode on a camera whose lens can't be stepped.")
        case .bracket:
            return LocalizedStringResource(
                "Hold still and tap the shutter.", comment: "Hint on the camera screen in Bracket mode.")
        }
    }

    /// Frames and delay, then start and step, each slider showing its
    /// bracket end through the lens (and the peaking) while dragged.
    private var bracketControls: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                Menu {
                    Picker(
                        LocalizedStringResource(
                            "Frames", comment: "The camera screen's menu of how many frames a bracket takes."),
                        selection: $bracketFrames
                    ) {
                        ForEach(Self.bracketCounts, id: \.self) { count in
                            Text(
                                "\(count) frames",
                                comment: "A choice in the bracket's Frames menu, and its label. The placeholder is the number of frames (5 to 20).")
                                .tag(count)
                        }
                    }
                } label: {
                    // Fewer than chosen when the last steps would focus past
                    // the farthest.
                    HStack(spacing: 4) {
                        Text(
                            "\(bracketPositions.count) frames",
                            comment: "A choice in the bracket's Frames menu, and its label. The placeholder is the number of frames (5 to 20).")
                            .contentTransition(.numericText())
                        Image(systemName: "chevron.up.chevron.down")
                            .font(.caption2.weight(.semibold))
                            .foregroundStyle(.secondary)
                    }
                    .font(.caption.weight(.semibold).monospacedDigit())
                    .lineLimit(1)
                    .fixedSize()
                    .padding(.horizontal, 10)
                    .padding(.vertical, 6)
                    .foregroundStyle(.white)
                    .background(.white.opacity(0.12), in: Capsule())
                }
                .buttonStyle(.plain)
                Menu {
                    Picker(
                        LocalizedStringResource(
                            "Delay", comment: "The camera screen's menu of how long a bracket waits after the shutter before its first frame."),
                        selection: $bracketDelay
                    ) {
                        ForEach(Self.delays, id: \.self) { seconds in
                            Text(Self.delayLabel(seconds)).tag(seconds)
                        }
                    }
                } label: {
                    Image(systemName: bracketDelay == 0 ? "timer" : "\(bracketDelay).circle")
                        .font(.callout.weight(.medium))
                        .foregroundStyle(bracketDelay > 0 ? .white : .white.opacity(0.75))
                        .frame(width: 30, height: 28)
                        .background(
                            bracketDelay > 0 ? AnyShapeStyle(Color.accentColor) : AnyShapeStyle(.white.opacity(0.12)),
                            in: Capsule())
                }
                .buttonStyle(.plain)
                .accessibilityLabel(
                    Text("Delay", comment: "The camera screen's menu of how long a bracket waits after the shutter before its first frame."))
                .accessibilityValue(Text(Self.delayLabel(bracketDelay)))
                Spacer(minLength: 0)
                Text(
                    "Focus \(Self.percent(Double(bracketPositions.first ?? 0))) to \(Self.percent(Double(bracketPositions.last ?? 0)))",
                    comment: "On the bracket's controls: where the first and the last frame focus, each a percentage of the lens's range from closest focus ('10%').")
                    .font(.footnote.monospacedDigit())
                    .foregroundStyle(.white)
                    .contentTransition(.numericText())
                    .lineLimit(1)
                    .minimumScaleFactor(0.85)
            }
            bracketSlider(
                Text("First", comment: "Label of the bracket's slider for where its first frame focuses."),
                value: Self.percent(bracketNear)
            ) {
                // The lens rests at the first frame's focus, so it follows by
                // itself.
                Slider(value: $bracketNear, in: Self.nearRange, step: 0.01) { editing in
                    previewing = editing ? .first : nil
                }
                .accessibilityLabel(
                    Text("First frame's focus", comment: "VoiceOver label of the bracket's slider for where its first frame focuses."))
                .accessibilityValue(Text(Self.percent(bracketNear)))
            }
            bracketSlider(
                Text("Step", comment: "Label of the bracket's slider for the lens travel between frames."),
                value: Self.percent(bracketStep)
            ) {
                // Dragging shows the last frame's focus; letting go returns.
                Slider(value: $bracketStep, in: Self.stepRange, step: 0.0025) { editing in
                    previewing = editing ? .last : nil
                    camera.previewLens(editing ? bracketPositions.last : nil)
                }
                .onChange(of: bracketStep) {
                    if previewing == .last {
                        camera.previewLens(bracketPositions.last)
                    }
                }
                .accessibilityLabel(
                    Text("Focus step", comment: "VoiceOver label of the bracket's step slider: the lens travel between frames."))
                .accessibilityValue(Text(Self.percent(bracketStep)))
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
        .glassEffect(.regular, in: RoundedRectangle(cornerRadius: 22))
        .frame(maxWidth: 420)
    }

    private func bracketSlider(_ name: Text, value: String, @ViewBuilder slider: () -> some View) -> some View {
        HStack(spacing: 8) {
            name
                .frame(width: 40, alignment: .leading)
            slider()
            Text(verbatim: value)
                .frame(width: 52, alignment: .trailing)
        }
        .font(.footnote.monospacedDigit())
        .foregroundStyle(.white)
        .lineLimit(1)
        .minimumScaleFactor(0.85)
    }
}
