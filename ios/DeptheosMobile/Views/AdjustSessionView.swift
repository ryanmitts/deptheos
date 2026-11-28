import SwiftUI
@preconcurrency import DeptheosEngine

/// Adjust over Stacking (ADR-171): the photo with the preview over it, the
/// toolbar, and the deck. At compact width the deck sits under the photo;
/// at regular width it is a column beside it. A size-class change keeps
/// the group and tool, which live here and in the engine's view.
struct AdjustSessionView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.horizontalSizeClass) private var horizontalSizeClass
    @Environment(\.verticalSizeClass) private var verticalSizeClass
    /// The heading whose sliders the compact deck shows.
    @State private var group: DeptheosEngine.Group = .tone
    /// A tap on the photo picks the white balance.
    @State private var eyedropping = false
    /// The crop's shape while dragging it.
    @State private var aspect = CropAspect.original

    private var adjust: AdjustModel { model.adjust }
    private var column: Bool { horizontalSizeClass == .regular && verticalSizeClass == .regular }

    var body: some View {
        NavigationStack {
            HStack(spacing: 0) {
                VStack(spacing: 0) {
                    // The photo fits what the deck leaves; it runs under the
                    // bars above.
                    ZStack {
                        Color.viewer.ignoresSafeArea()
                        photo
                            .scrollEdgeEffectHidden(true, for: .all)
                            .ignoresSafeArea(.container, edges: column ? .all : [.top, .horizontal])
                    }
                    if !column {
                        AdjustDeck(group: $group, eyedropping: $eyedropping, aspect: $aspect, compact: true)
                            .background(Color.viewer.ignoresSafeArea())
                    }
                }
                if column {
                    AdjustDeck(group: $group, eyedropping: $eyedropping, aspect: $aspect, compact: false)
                        .frame(width: 340)
                        .background(.background.secondary)
                }
            }
            .navigationTitle(Text(model.adjustTitle))
            .navigationBarTitleDisplayMode(.inline)
            .barsNeverMinimize()
            .toolbar { toolbar }
            .toolbarColorScheme(.dark, for: .navigationBar, .bottomBar)
        }
        .environment(\.colorScheme, .dark)
        .onChange(of: model.adjustView?.tool) { _, tool in
            if tool == .crop { eyedropping = false }
        }
    }

    /// The picture shown: Before while held, else the result with a drag's
    /// preview over it.
    @ViewBuilder
    private var photo: some View {
        let holding = model.adjustView?.before == true
        if let picture = (holding ? adjust.before.map(model.windowed) : nil) ?? model.resultPicture {
            ZoomableImage(
                picture: picture, keepsFrame: true, fitRequest: model.fitRequest,
                onImageTap: eyedropping ? { point in pickWhite(at: point, in: picture.size) } : nil,
                adjustOverlay: holding || model.adjustView?.preview != true ? nil : adjust.overlay,
                onView: { zoom, visible in adjust.viewChanged(zoom: zoom, visible: visible) },
                crop: holding ? nil : cropTool(for: picture.size))
                .accessibilityPhoto(.app(model.adjustTitle))
        }
    }

    /// In the crop tool, on the uncropped picture it was drawn for.
    private func cropTool(for size: CGSize) -> CropTool? {
        guard model.adjustView?.tool == .crop, let result = model.result?.stack, let value = adjust.value else { return nil }
        let stored = result.storedSize
        let frame = CropFrame(width: stored.width, height: stored.height, reframe: value.reframe())
        let shown = frame.area()
        guard CGFloat(PaneFraming(width: stored.width, height: stored.height, reframe: value.reframe(), pane: .uncropped).size().x) == size.width
        else { return nil }
        return CropTool(frame: frame, area: shown, aspect: aspect) { [adjust] reframe, ended in
            adjust.setReframe(reframe)
            if ended { adjust.commit() }
        }
    }

    /// The stored pixel under `point` (fractions of the shown picture).
    private func pickWhite(at point: CGPoint, in size: CGSize) -> Bool {
        guard let result = model.result?.stack, let value = adjust.value else { return false }
        let stored = result.storedSize
        let framing = PaneFraming(width: stored.width, height: stored.height, reframe: value.reframe(), pane: .uncropped)
        let shown = PicturePoint(x: Double(point.x * size.width), y: Double(point.y * size.height))
        guard let pixel = framing.storedPixel(p: shown) else { return false }
        adjust.eyedrop(at: pixel)
        eyedropping = false
        return true
    }

    @ToolbarContentBuilder
    private var toolbar: some ToolbarContent {
        ToolbarItemGroup(placement: .topBarLeading) {
            Button(
                LocalizedStringResource("Undo", comment: "Adjust toolbar: takes back the last adjustment."),
                systemImage: "arrow.uturn.backward"
            ) { adjust.undo() }
                .disabled((adjust.status?.undo ?? 0) == 0)
                .keyboardShortcut("z", modifiers: .command)
                .accessibilityIdentifier("adjust-undo")
            Button(
                LocalizedStringResource("Redo", comment: "Adjust toolbar: puts back the adjustment taken back."),
                systemImage: "arrow.uturn.forward"
            ) { adjust.redo() }
                .disabled((adjust.status?.redo ?? 0) == 0)
                .keyboardShortcut("z", modifiers: [.command, .shift])
                .accessibilityIdentifier("adjust-redo")
        }
        .keptVisible()

        ToolbarItem(placement: .primaryAction) {
            Menu {
                Button(ViewerDeck.zoomToFit, systemImage: ViewerDeck.zoomToFitSymbol) {
                    model.fitRequest += 1
                }
                Button(
                    LocalizedStringResource("Export…", comment: "Toolbar button: opens the Export sheet for the result shown."),
                    systemImage: "square.and.arrow.up"
                ) { model.present(.export) }
                    .disabled(!model.canExport)
                Divider()
                Button(AdjustCommands.copy, systemImage: "doc.on.doc") {
                    Task { await model.copyAdjustments() }
                }
                Button(AdjustCommands.paste, systemImage: "doc.on.clipboard") {
                    model.pasteAdjustments()
                }
                .disabled(model.copiedAdjustments == nil)
                Divider()
                Button(
                    LocalizedStringResource("Reset All", comment: "Adjust menu button: every slider back to its start, as one undoable change."),
                    systemImage: "arrow.counterclockwise"
                ) { adjust.resetAll() }
                    .disabled(adjust.value?.isNeutral() ?? true)
            } label: {
                Label(
                    LocalizedStringResource("More", comment: "Adjust toolbar menu for Zoom to Fit, Export, Copy and Paste Adjustments and Reset All."),
                    systemImage: "ellipsis.circle")
            }
            .accessibilityIdentifier("adjust-more")
        }
        .keptVisible()

        ToolbarItem(placement: .confirmationAction) {
            Button(role: .confirm) {
                model.stopAdjusting()
            } label: {
                Label(
                    LocalizedStringResource("Done", comment: "Toolbar button in Adjust: leaves Adjust, its adjustments kept."),
                    systemImage: "checkmark")
            }
            .buttonStyle(.glassProminent)
            .tint(.accentFill)
            .keyboardShortcut(.escape, modifiers: [])
            .accessibilityIdentifier("adjust-done")
        }
        .keptVisible()
    }
}

/// Copy and Paste Adjustments: the menu bar's and Adjust's menu.
enum AdjustCommands {
    static let copy = LocalizedStringResource(
        "Copy Adjustments", comment: "Menu item: copies the adjustments of the render shown, to paste onto another.")
    static let paste = LocalizedStringResource(
        "Paste Adjustments", comment: "Menu item: sets the copied sliders on the render shown, its crop kept.")
}

/// Adjust's controls: the status, the histogram, the tool, and the sliders
/// (the chosen heading's at compact width, every heading in a column).
struct AdjustDeck: View {
    @Environment(AppModel.self) private var model
    @Binding var group: DeptheosEngine.Group
    @Binding var eyedropping: Bool
    @Binding var aspect: CropAspect
    let compact: Bool

    private var adjust: AdjustModel { model.adjust }

    var body: some View {
        VStack(spacing: 10) {
            HStack(alignment: .center, spacing: 12) {
                toolPicker
                Spacer(minLength: 0)
                beforeButton
            }
            if let phase = adjust.status?.phase, phase != .ready {
                phaseLine(phase)
            }
            if model.adjustView?.tool == .crop {
                AdjustCropDeck(aspect: $aspect)
            } else if let panel = adjust.panel, let value = adjust.value {
                if let histogram = adjust.histogram {
                    AdjustHistogramView(histogram: histogram)
                        .frame(height: compact ? 36 : 80)
                }
                if compact {
                    groupPicker(panel)
                    ScrollView {
                        sliders(of: panel.groups.first { $0.group == group } ?? panel.groups.first, value: value)
                    }
                    .frame(maxHeight: 156)
                    .scrollBounceBehavior(.basedOnSize)
                } else {
                    ScrollView {
                        VStack(alignment: .leading, spacing: 18) {
                            ForEach(panel.groups, id: \.group) { info in
                                VStack(alignment: .leading, spacing: 8) {
                                    Text(info.label).font(.headline)
                                    sliders(of: info, value: value)
                                }
                            }
                        }
                        .padding(.bottom, 12)
                    }
                    .scrollIndicators(.automatic)
                }
            }
            if !compact { Spacer(minLength: 0) }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 12)
        .frame(maxWidth: compact ? 560 : .infinity)
        .modifier(DeckBackground(compact: compact))
        .padding(.horizontal, compact ? 8 : 0)
        .disabled(adjust.status?.phase != .ready)
        .onChange(of: adjust.panel?.groups.map(\.group)) { _, groups in
            if let groups, !groups.contains(group), let first = groups.first { group = first }
        }
    }

    private var toolPicker: some View {
        Picker(
            LocalizedStringResource("Tool", comment: "Adjust: picks between the sliders and Crop."),
            selection: Binding(
                get: { model.adjustView?.tool ?? .sliders },
                set: { model.view(.tool(tool: $0)) })
        ) {
            Text(LocalizedStringResource("Adjust", comment: "Adjust's tool picker: the sliders.")).tag(Tool.sliders)
            Text(LocalizedStringResource("Crop", comment: "Adjust's tool picker: crop, turn and flip.")).tag(Tool.crop)
        }
        .pickerStyle(.segmented)
        .fixedSize()
        .accessibilityIdentifier("adjust-tool")
    }

    /// Held: the render without its adjustments.
    private var beforeButton: some View {
        Text(LocalizedStringResource("Before", comment: "Adjust: held, the photo shows without its adjustments."))
            .font(.subheadline.weight(.medium))
            .padding(.horizontal, 12)
            .padding(.vertical, 6)
            .background(Capsule().fill(model.adjustView?.before == true ? Color.accentFill : Color.white.opacity(0.12)))
            .onLongPressGesture(minimumDuration: 0, maximumDistance: 40, perform: {}) { pressing in
                model.view(.holdBefore(held: pressing))
            }
            .accessibilityAddTraits(.isButton)
            .accessibilityHint(LocalizedStringResource(
                "Touch and hold to see the photo without its adjustments",
                comment: "VoiceOver hint on Adjust's Before button."))
            .accessibilityIdentifier("adjust-before")
            .help(LocalizedStringResource(
                "Hold to see the render without its adjustments", comment: "Pointer tooltip of Adjust's Before button."))
    }

    @ViewBuilder
    private func phaseLine(_ phase: AdjustPhase) -> some View {
        switch phase {
        case .opening:
            Label(
                LocalizedStringResource("Opening the render…", comment: "Over Adjust's sliders while the render opens."),
                systemImage: "hourglass")
                .font(.footnote).foregroundStyle(.secondary)
        case .away:
            Label(
                LocalizedStringResource(
                    "Adjust picks up where it was when you're back",
                    comment: "Over Adjust's sliders while the app is away and Adjust has let go of the GPU."),
                systemImage: "pause.circle")
                .font(.footnote).foregroundStyle(.secondary)
        case .failed(let message):
            HStack {
                Text(message).font(.footnote).foregroundStyle(.secondary)
                Spacer()
                Button(LocalizedStringResource("Try Again", comment: "Adjust: opens the render again after a failure.")) {
                    adjust.retry()
                }
            }
            .disabled(false)
        case .ready:
            EmptyView()
        }
    }

    private func groupPicker(_ panel: AdjustPanel) -> some View {
        ScrollView(.horizontal) {
            HStack(spacing: 6) {
                ForEach(panel.groups, id: \.group) { info in
                    Button(info.label) { group = info.group }
                        .buttonStyle(.bordered)
                        .tint(info.group == group ? .accentFill : .secondary)
                        .controlSize(.small)
                        .accessibilityAddTraits(info.group == group ? .isSelected : [])
                }
                if panel.groups.contains(where: { $0.group == .whiteBalance }) {
                    eyedropperButton
                }
            }
        }
        .scrollIndicators(.hidden)
    }

    private var eyedropperButton: some View {
        Button {
            eyedropping.toggle()
        } label: {
            Label(
                LocalizedStringResource("White Balance Eyedropper", comment: "Adjust: a tap on something neutral gray in the photo then sets the white balance."),
                systemImage: "eyedropper")
                .labelStyle(.iconOnly)
        }
        .buttonStyle(.bordered)
        .tint(eyedropping ? .accentFill : .secondary)
        .controlSize(.small)
        .keyboardShortcut("w", modifiers: [])
        .accessibilityIdentifier("adjust-eyedropper")
        .help(LocalizedStringResource(
            "White balance eyedropper: tap something neutral gray in the photo",
            comment: "Pointer tooltip of Adjust's eyedropper."))
    }

    @ViewBuilder
    private func sliders(of info: AdjustGroupInfo?, value: AdjustValue) -> some View {
        if let info {
            VStack(spacing: 6) {
                ForEach(info.sliders, id: \.slider) { slider in
                    AdjustSliderRow(info: slider, value: value)
                }
                if info.group == .whiteBalance, !compact { EmptyView() }
            }
            if info.group == .whiteBalance, !compact {
                HStack { eyedropperButton; Spacer() }
            }
        }
    }
}

/// Glass under the compact deck; the column is plain.
private struct DeckBackground: ViewModifier {
    let compact: Bool

    func body(content: Content) -> some View {
        if compact {
            content.glassEffect(.regular, in: RoundedRectangle(cornerRadius: 22))
        } else {
            content
        }
    }
}

/// One slider: its label, its value as the engine words it, and the track
/// in the engine's positions (the kelvin track's bend is its). A change
/// goes on until let go, then commits; a double tap on the label resets
/// it. VoiceOver reads the value and steps it.
struct AdjustSliderRow: View {
    @Environment(AppModel.self) private var model
    let info: AdjustSliderInfo
    let value: AdjustValue
    /// The finger's place while dragging, so the thumb doesn't wait on the
    /// engine's status.
    @State private var dragging: Double?

    private var adjust: AdjustModel { model.adjust }

    var body: some View {
        let current = value.get(slider: info.slider)
        VStack(spacing: 2) {
            HStack {
                Text(info.label)
                    .font(.subheadline)
                    .onTapGesture(count: 2) { adjust.reset(info.slider) }
                Spacer()
                Text(value.text(slider: info.slider))
                    .font(.subheadline.monospacedDigit())
                    .foregroundStyle(value.isDefault(slider: info.slider) ? .secondary : .primary)
            }
            Slider(
                value: Binding(
                    get: { dragging ?? value.position(slider: info.slider, value: current) },
                    set: { p in
                        dragging = p
                        adjust.set(info.slider, to: value.valueAt(slider: info.slider, p: p))
                    }),
                in: 0...1
            ) { editing in
                if !editing {
                    dragging = nil
                    adjust.commit()
                }
            }
            .accessibilityHidden(true)
        }
        .help(info.help)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(Text(info.label))
        .accessibilityValue(Text(value.text(slider: info.slider)))
        .accessibilityHint(Text(info.help))
        .accessibilityAdjustableAction { direction in
            let step = direction == .increment ? info.step : -info.step
            let next = min(max(current + step, info.min), info.max)
            adjust.set(info.slider, to: next)
            adjust.commit()
        }
        .accessibilityAction(named: LocalizedStringResource("Reset", comment: "VoiceOver action on an Adjust slider: back to its start.")) {
            adjust.reset(info.slider)
        }
        .accessibilityIdentifier("adjust-slider-\(String(describing: info.slider))")
    }
}

/// The histogram of the picture under its crop: luminance filled, the
/// channels over it, and a mark at each end that clips.
struct AdjustHistogramView: View {
    let histogram: AdjustHistogram

    var body: some View {
        Canvas { context, size in
            let peak = max(histogram.luminance.dropFirst().dropLast().max() ?? 1, 1)
            func path(_ bins: [UInt32]) -> Path {
                var path = Path()
                path.move(to: CGPoint(x: 0, y: size.height))
                for (i, n) in bins.enumerated() {
                    let x = size.width * CGFloat(i) / CGFloat(max(bins.count - 1, 1))
                    let y = size.height * (1 - min(CGFloat(n) / CGFloat(peak), 1))
                    path.addLine(to: CGPoint(x: x, y: y))
                }
                path.addLine(to: CGPoint(x: size.width, y: size.height))
                path.closeSubpath()
                return path
            }
            context.fill(path(histogram.luminance), with: .color(.white.opacity(0.25)))
            for (bins, color) in [(histogram.red, Color.red), (histogram.green, .green), (histogram.blue, .blue)] {
                context.stroke(path(bins), with: .color(color.opacity(0.7)), lineWidth: 1)
            }
            let mark = CGSize(width: 8, height: 8)
            if histogram.shadowsClipped {
                context.fill(Path(CGRect(origin: .zero, size: mark)), with: .color(.cyan))
            }
            if histogram.highlightsClipped {
                context.fill(Path(CGRect(origin: CGPoint(x: size.width - mark.width, y: 0), size: mark)), with: .color(.orange))
            }
        }
        .background(RoundedRectangle(cornerRadius: 6).fill(.black.opacity(0.3)))
        .accessibilityElement()
        .accessibilityLabel(Text(LocalizedStringResource("Histogram", comment: "VoiceOver label of Adjust's histogram.")))
        .accessibilityValue(Text(clippingText))
    }

    private var clippingText: LocalizedStringResource {
        switch (histogram.shadowsClipped, histogram.highlightsClipped) {
        case (true, true):
            LocalizedStringResource("Shadows and highlights clipped", comment: "VoiceOver value of Adjust's histogram.")
        case (true, false):
            LocalizedStringResource("Shadows clipped", comment: "VoiceOver value of Adjust's histogram.")
        case (false, true):
            LocalizedStringResource("Highlights clipped", comment: "VoiceOver value of Adjust's histogram.")
        case (false, false):
            LocalizedStringResource("Nothing clipped", comment: "VoiceOver value of Adjust's histogram.")
        }
    }
}
