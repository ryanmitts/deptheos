import SwiftUI
@preconcurrency import DeptheosEngine

/// Retouch the result: paint mode on.
struct RetouchButton: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        Button(
            LocalizedStringResource("Retouch", comment: "Toolbar button: opens paint mode on the result."),
            systemImage: "paintbrush.pointed"
        ) { model.startPainting() }
            .disabled(!model.canPaint)
            .accessibilityIdentifier("retouch")
            .help(LocalizedStringResource(
                "Paint over the result from a frame or another render", comment: "Pointer tooltip of the Retouch button."))
    }
}

/// Adjust the result: its sliders, crop, turn and flip.
struct AdjustButton: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        Button(
            LocalizedStringResource("Adjust", comment: "Toolbar button: opens Adjust on the result, its sliders and crop."),
            systemImage: "slider.horizontal.3"
        ) { model.startAdjusting() }
            .disabled(!model.canAdjust)
            .accessibilityIdentifier("adjust")
            .help(LocalizedStringResource(
                "Adjust the result's light, color, crop and turn", comment: "Pointer tooltip of the Adjust button."))
    }
}

/// Whether a finger paints, on iPad: in Retouch's options menu and
/// Settings ▸ Retouch. It starts as the system's Only Draw with Apple
/// Pencil, so it is shown where painting happens.
struct OnlyPencilToggle: View {
    @Bindable var paint: PaintModel

    static var offered: Bool { UIDevice.current.userInterfaceIdiom == .pad }

    var body: some View {
        Toggle(
            LocalizedStringResource(
                "Only Pencil paints",
                comment: "Switch in Retouch's Brush panel and Settings ▸ Retouch (iPad): a finger pans and zooms rather than paints."),
            isOn: $paint.onlyPencilPaints)
    }

    /// What a finger does, as the switch is set.
    static func footer(_ on: Bool) -> LocalizedStringResource {
        on
            ? LocalizedStringResource(
                "Fingers pan and zoom; touch and hold shows the source. Starts as Only Draw with Apple Pencil in the iPad's Settings.",
                comment: "Retouch's Brush panel (iPad): under Only Pencil paints, on. Only Draw with Apple Pencil is the system setting's name in Settings ▸ Apple Pencil.")
            : LocalizedStringResource(
                "A finger paints too; two fingers pan and zoom.",
                comment: "Retouch's Brush panel (iPad): under Only Pencil paints, off.")
    }
}

/// Size, hardness, opacity and flow, each with its value: rows in a list,
/// or stacked.
struct BrushSliders: View {
    static let size = LocalizedStringResource("Size", comment: "Brush slider, a name: the brush's size.")

    @Bindable var paint: PaintModel

    var body: some View {
        let ranges = paint.ranges
        BrushSetting(name: BrushSliders.size, value: .app(paint.radiusLabel)) {
            BrushSizeSlider(paint: paint)
        }
        BrushSetting(
            name: LocalizedStringResource("Hardness", comment: "Brush slider, a name: how sharp the brush's edge is."),
            value: .verbatim(paint.hardness.formatted(.number.precision(.fractionLength(2))))) {
            Slider(value: $paint.hardness, in: ranges.hardness.min...ranges.hardness.max)
        }
        BrushSetting(
            name: LocalizedStringResource("Opacity", comment: "Brush slider, a name: how much one stroke paints at most."),
            value: percent(paint.opacity)) {
            Slider(value: $paint.opacity, in: ranges.opacity.min...ranges.opacity.max)
        }
        BrushSetting(
            name: LocalizedStringResource("Flow", comment: "Brush slider, a name: how much each dab of a stroke paints."),
            value: percent(paint.flow)) {
            Slider(value: $paint.flow, in: ranges.flow.min...ranges.flow.max)
        }
    }
}

/// How close the source's colors must be to the result's for the brush to
/// paint.
struct ToleranceSlider: View {
    static let onlyWhereColorsMatch = LocalizedStringResource(
        "Only where colors match", comment: "Brush switch: paint only where the source's colors are close to the result's.")

    @Bindable var paint: PaintModel

    var body: some View {
        let range = paint.ranges.tolerance
        BrushSetting(
            name: LocalizedStringResource(
                "Within", comment: "Brush slider, a name, under Only where colors match: how close the colors must be."),
            value: percent(paint.tolerance)) {
            Slider(value: $paint.tolerance, in: range.min...range.max)
        }
    }
}

/// On a log scale: a drag changes it by the same ratio at any size.
struct BrushSizeSlider: View {
    @Bindable var paint: PaintModel

    var body: some View {
        let range = paint.ranges.radius
        Slider(
            value: Binding(get: { log2(paint.radius) }, set: { paint.radius = exp2($0) }),
            in: log2(range.min)...log2(range.max))
    }
}

private struct BrushSetting<Control: View>: View {
    let name: LocalizedStringResource
    let value: UserText
    @ViewBuilder let slider: () -> Control

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack {
                Text(name)
                Spacer()
                Text(value).monospacedDigit().foregroundStyle(.secondary)
            }
            slider()
        }
    }
}

/// "40%", in the user's locale.
private func percent(_ value: Float) -> UserText {
    .verbatim(value.formatted(.percent.precision(.fractionLength(0))))
}

/// Dedicated full-screen Photos-Edit–style Retouch session overlaid over
/// `StackingView`. Owns its own dark `NavigationStack` and live-saved top bar
/// (`[ Undo | Redo ]` leading, title/subtitle center, `•••` + prominent `✓`
/// Done trailing), hides the split sidebar even on iPad, and frames the photo
/// cleanly inside the clear area above/between its two-tier tray (portrait) or
/// split side rails (landscape).
struct RetouchSessionView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.horizontalSizeClass) private var horizontalSizeClass
    @Environment(\.verticalSizeClass) private var verticalSizeClass
    @Environment(\.layoutDirection) private var layoutDirection
    @State private var tab: RetouchTab = .source
    @State private var previewingBrush = false
    @State private var safeArea = EdgeInsets()
    @State private var trayInsets = UIEdgeInsets.zero
    @State private var reserved = ReservedAreas()
    @State private var scrubbing = false
    @State private var confirmReset = false

    private var compact: Bool { horizontalSizeClass == .compact }
    private var splitRails: Bool { verticalSizeClass == .compact }
    private var paint: PaintModel { model.paint }

    var body: some View {
        NavigationStack {
            ZStack {
                Color.viewer.ignoresSafeArea()
                RetouchPhoto(
                    below: .row(trayInsets.bottom),
                    extraChromeInsets: UIEdgeInsets(
                        top: 0, left: trayInsets.left, bottom: 0, right: trayInsets.right),
                    previewingBrush: previewingBrush,
                    chrome: chrome,
                    labelInsets: safeArea,
                    showsLabels: !scrubbing)
                    .scrollEdgeEffectHidden(true, for: .all)
                    .ignoresSafeArea(.container, edges: .all)
            }
            .overlay {
                PaintOverlay(
                    tab: $tab,
                    phone: compact,
                    splitRails: splitRails,
                    foldLeading: foldSides?.leading,
                    clear: occluded,
                    onPreviewBrush: { previewingBrush = $0 },
                    onTrayInsets: { trayInsets = $0 }
                )
                .environment(\.automationLoupe, Automation.flag("RailLoupe"))
            }
            .background {
                Color.clear
                    .measure("retouch safe area", for: EdgeInsets.self) { [space = Self.space, rtl = layoutDirection == .rightToLeft] in
                        Self.readSafeArea($0, space: space, rtl: rtl)
                    } action: { safeArea = $0 }
                    .ignoresSafeArea(.container, edges: .all)
            }
            .coordinateSpace(.named(Self.space))
            .measure("retouch session reserved", for: ReservedAreas.self) { ReservedAreas.read($0) } action: {
                reserved = $0
            }
            .navigationTitle(Text(model.retouchTitle))
            .navigationSubtitle(compact ? Text(model.strokesText) : Text(model.retouchLine))
            .navigationBarTitleDisplayMode(.inline)
            .barsNeverMinimize()
            .toolbar { retouchToolbar }
            .toolbarColorScheme(.dark, for: .navigationBar, .bottomBar)
            .onPreferenceChange(RailScrubbing.self) { value in
                withAnimation(.easeOut(duration: 0.15)) { scrubbing = value }
            }
        }
        .environment(\.colorScheme, .dark)
        .modifier(VerticalBarEdgeReader())
    }

    private static let space = "retouch-session"

    nonisolated private static func readSafeArea(_ proxy: GeometryProxy, space: String, rtl: Bool) -> EdgeInsets {
        guard let inner = proxy.bounds(of: .named(space)) else { return EdgeInsets() }
        let left = max(inner.minX, 0)
        let right = max(proxy.size.width - inner.maxX, 0)
        return EdgeInsets(
            top: max(inner.minY, 0), leading: rtl ? right : left,
            bottom: max(proxy.size.height - inner.maxY, 0), trailing: rtl ? left : right
        ).rounded
    }

    private var chrome: UIEdgeInsets {
        let rtl = layoutDirection == .rightToLeft
        return UIEdgeInsets(
            top: safeArea.top,
            left: (rtl ? safeArea.trailing : safeArea.leading) + trayInsets.left,
            bottom: safeArea.bottom + trayInsets.bottom,
            right: (rtl ? safeArea.leading : safeArea.trailing) + trayInsets.right)
    }

    private var sidePadding: CGFloat { compact ? 16 : 12 }

    private var foldSides: (leading: CGFloat, gutter: CGFloat)? {
        guard let fold = reserved.verticalFold(minGutter: 0) else { return nil }
        let leading = fold.leading - sidePadding - 8
        guard leading > 0 else { return nil }
        return (leading, fold.gutter + 16)
    }

    private var occluded: EdgeInsets {
        let depth = reserved.occludedInsets(within: 64)
        return EdgeInsets(
            top: max(depth.top - 8, 0), leading: max(depth.leading - sidePadding, 0),
            bottom: max(depth.bottom - 8, 0), trailing: max(depth.trailing - sidePadding, 0))
    }

    @ToolbarContentBuilder
    private var retouchToolbar: some ToolbarContent {
        ToolbarItemGroup(placement: .topBarLeading) {
            Button(
                LocalizedStringResource("Undo", comment: "Paint mode toolbar: takes back the last stroke."),
                systemImage: "arrow.uturn.backward"
            ) { paint.undo() }
                .disabled(paint.undoCount == 0)
                .keyboardShortcut("z", modifiers: .command)
            Button(
                LocalizedStringResource("Redo", comment: "Paint mode toolbar: does the stroke taken back again."),
                systemImage: "arrow.uturn.forward"
            ) { paint.redo() }
                .disabled(paint.redoCount == 0)
                .keyboardShortcut("z", modifiers: [.command, .shift])
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
                if OnlyPencilToggle.offered {
                    Divider()
                    Toggle(
                        isOn: Binding(get: { paint.onlyPencilPaints }, set: { paint.onlyPencilPaints = $0 })
                    ) {
                        Label(
                            LocalizedStringResource(
                                "Only Pencil paints",
                                comment: "Switch in Retouch's options menu (iPad): a finger pans and zooms rather than paints."),
                            systemImage: "applepencil")
                        Text(paint.onlyPencilPaints
                             ? LocalizedStringResource(
                                "Fingers pan and zoom; hold to peek",
                                comment: "Subtitle under Only Pencil paints in Retouch's menu when on.")
                             : LocalizedStringResource(
                                "A finger paints; two fingers pan",
                                comment: "Subtitle under Only Pencil paints in Retouch's menu when off."))
                    }
                }
                Divider()
                Button(
                    LocalizedStringResource(
                        "Reset Retouching…", comment: "Retouch menu button: asks, then takes the result back to as stacked."),
                    systemImage: "arrow.counterclockwise",
                    role: .destructive
                ) { confirmReset = true }
                    .disabled(!paint.painted)
            } label: {
                Label(
                    LocalizedStringResource("More", comment: "Retouch toolbar menu for Zoom to Fit, Export, Pencil options and Reset Retouching."),
                    systemImage: "ellipsis.circle")
            }
            .confirmationDialog(
                Text("Reset the retouching?", comment: "Title of the dialog after Reset Retouching…."),
                isPresented: $confirmReset, titleVisibility: .visible
            ) {
                Button(LocalizedStringResource("Reset", comment: "Destructive button of the Reset the retouching? dialog."), role: .destructive) {
                    paint.reset()
                }
            } message: {
                Text(
                    "Every painted pixel goes back to the stacked result, and the undo history is cleared. This can't be undone.",
                    comment: "Text of the Reset the retouching? dialog.")
            }
        }
        .keptVisible()

        ToolbarItem(placement: .confirmationAction) {
            Button(role: .confirm) {
                model.stopPainting()
            } label: {
                Label(
                    LocalizedStringResource("Done", comment: "Toolbar button in paint mode: leaves paint mode."),
                    systemImage: "checkmark")
            }
            .buttonStyle(.glassProminent)
            .tint(.accentFill)
        }
        .keptVisible()
    }
}

/// Over the canvas while retouching: top status label, notices, and
/// `RetouchDeck` (two-tier bottom tray in portrait/iPad, or split leading/
/// trailing rails in compact height).
struct PaintOverlay: View {
    @Environment(AppModel.self) private var model
    @Binding var tab: RetouchTab
    /// Compact width: the iPhone, the Duo's outer screen.
    var phone = false
    /// Compact height (iPhone landscape): splits the mode rail to the leading
    /// edge and contextual controls to the trailing/bottom edge.
    var splitRails = false
    /// The deck's room before a top-to-bottom fold, if any.
    var foldLeading: CGFloat?
    /// How far the controls move in from each edge, clear of the inner
    /// camera.
    var clear = EdgeInsets()
    var onPreviewBrush: (Bool) -> Void = { _ in }
    /// The tray's depth past the safe area on each edge, for clear-rect fit.
    var onTrayInsets: (UIEdgeInsets) -> Void = { _ in }

    @State private var measuredBottom: CGFloat = 0
    @State private var measuredLeading: CGFloat = 0
    @State private var measuredTrailing: CGFloat = 0

    private var paint: PaintModel { model.paint }

    var body: some View {
        Group {
            if splitRails {
                landscapeLayout
            } else {
                portraitLayout
            }
        }
        .padding(.leading, (phone ? 16 : 12) + clear.leading)
        .padding(.trailing, (phone ? 16 : 12) + clear.trailing)
        .padding(.top, phone ? 8 : 12)
        .padding(.bottom, (phone ? 8 : 12) + clear.bottom)
        .onChange(of: splitRails) { _, isSplit in
            if !isSplit {
                measuredLeading = 0
                measuredTrailing = 0
                publishInsets()
            }
        }
    }

    private var portraitLayout: some View {
        VStack(alignment: .leading, spacing: 8) {
            topStatusRow
            Spacer(minLength: 0)
            VStack(spacing: 8) {
                notice
                HStack(spacing: 0) {
                    if !phone, foldLeading == nil { Spacer(minLength: 0) }
                    RetouchDeck(tab: $tab, onPreviewBrush: onPreviewBrush, railRole: .full)
                        .frame(maxWidth: foldLeading ?? (phone ? .infinity : 580))
                    if !phone || foldLeading != nil { Spacer(minLength: 0) }
                }
            }
            .measure("retouch bottom bars", for: CGFloat.self) { $0.size.height } action: { height in
                measuredBottom = height + (phone ? 8 : 12)
                measuredLeading = 0
                measuredTrailing = 0
                publishInsets()
            }
        }
    }

    private var landscapeLayout: some View {
        VStack(alignment: .leading, spacing: 8) {
            topStatusRow
            HStack(alignment: .center, spacing: 12) {
                RetouchDeck(
                    tab: $tab, onPreviewBrush: onPreviewBrush,
                    splitRails: true, railRole: .leadingRail
                )
                .measure("retouch leading rail", for: CGFloat.self) { $0.size.width } action: { width in
                    measuredLeading = width + 16
                    publishInsets()
                }
                Spacer(minLength: 0)
                if tab != .source {
                    RetouchDeck(
                        tab: $tab, onPreviewBrush: onPreviewBrush,
                        splitRails: true, railRole: .trailingOrBottom
                    )
                    .measure("retouch trailing rail", for: CGFloat.self) { $0.size.width } action: { width in
                        measuredTrailing = width + 16
                        measuredBottom = 0
                        publishInsets()
                    }
                }
            }
            .frame(maxHeight: .infinity)
            if tab == .source {
                VStack(spacing: 8) {
                    notice
                    RetouchDeck(
                        tab: $tab, onPreviewBrush: onPreviewBrush,
                        splitRails: true, railRole: .trailingOrBottom
                    )
                }
                .measure("retouch landscape bottom", for: CGFloat.self) { $0.size.height } action: { height in
                    measuredBottom = height + 8
                    measuredTrailing = 0
                    publishInsets()
                }
            } else {
                notice
            }
        }
    }

    private func publishInsets() {
        onTrayInsets(UIEdgeInsets(
            top: 0, left: measuredLeading, bottom: measuredBottom, right: measuredTrailing))
    }

    private var topStatusRow: some View {
        HStack {
            if case .opening? = paint.canvasState {
                PhotoLabel(text: .app(Self.openingLabel)).allowsHitTesting(false)
            } else if paint.touch == .picksFrame {
                PhotoLabel(text: .app(RetouchDeck.pickHint)).allowsHitTesting(false)
            } else if paint.peeking {
                PhotoLabel(text: .app(peekLabel)).allowsHitTesting(false)
            } else if paint.showsDepthMap {
                mapLabel
            } else if OnlyPencilToggle.offered, paint.onlyPencilPaints {
                pencilOnlyPill
            }
            Spacer().allowsHitTesting(false)
        }
        .padding(.top, clear.top)
    }

    private var pencilOnlyPill: some View {
        Button {
            withAnimation(.snappy) { paint.onlyPencilPaints = false }
        } label: {
            HStack(spacing: 5) {
                Image(systemName: "applepencil")
                    .font(.caption2.weight(.semibold))
                Text("Pencil Only", comment: "Status pill in Retouch on iPad when Only Pencil paints is enabled; tapping allows finger painting.")
                Image(systemName: "xmark")
                    .font(.caption2.weight(.bold))
                    .foregroundStyle(.secondary)
            }
            .font(.caption.weight(.semibold))
            .lineLimit(1)
            .padding(.horizontal, 10)
            .padding(.vertical, 6)
            .glassEffect(.regular.interactive(), in: .capsule)
            .padding(10)
            .contentShape(Rectangle())
            .padding(-10)
        }
        .buttonStyle(.plain)
        .accessibilityLabel(LocalizedStringResource(
            "Only Pencil paints",
            comment: "VoiceOver label of the Pencil Only status pill in Retouch on iPad."))
        .accessibilityHint(LocalizedStringResource(
            "Tap to also paint with a finger",
            comment: "VoiceOver hint of the Pencil Only status pill in Retouch on iPad."))
        .help(LocalizedStringResource(
            "Only Apple Pencil paints; tap to also paint with a finger",
            comment: "Pointer tooltip of the Pencil Only status pill in Retouch on iPad."))
    }

    private var mapLabel: some View {
        DepthMapLabel(
            text: model.depthMapLabel(lit: paint.shownLitMap?.file),
            help: LocalizedStringResource(
                "Back to the painted result", comment: "Pointer tooltip of the Depth Map label at the top of the photo in Retouch."),
            close: { paint.toggleDepthMap() })
    }

    /// While the canvas opens, painting not yet possible.
    static let openingLabel = LocalizedStringResource(
        "Opening for Retouch…", comment: "Retouch: the label at the top of the photo while the render opens for painting, which can't start until it has.")

    /// That nothing painted can be saved, while the worker says so, and the
    /// notice, if any, which a tap hides.
    private var notice: some View {
        VStack(spacing: 8) {
            if let unsaveable = paint.unsaveable {
                Label {
                    Text(verbatim: unsaveable)
                } icon: {
                    Image(systemName: "exclamationmark.triangle.fill")
                        .foregroundStyle(.yellow)
                }
                .font(.footnote)
                .padding(.horizontal, 14)
                .padding(.vertical, 10)
                .frame(maxWidth: 420)
                .glassEffect(.regular, in: RoundedRectangle(cornerRadius: 16))
                .transition(.opacity)
            }
            if let notice = model.notice {
                Text(notice)
                    .font(.footnote)
                    .multilineTextAlignment(.center)
                    .padding(.horizontal, 14)
                    .padding(.vertical, 10)
                    .frame(maxWidth: 420)
                    .glassEffect(.regular, in: RoundedRectangle(cornerRadius: 16))
                    .onTapGesture { model.notice = nil }
                    .accessibilityAddTraits(.isButton)
                    .transition(.opacity)
            }
        }
    }

    /// When only the Pencil paints, a finger's hold shows the source.
    static let holdHint = LocalizedStringResource(
        "Touch and hold to see the source", comment: "Paint mode hint, when only Apple Pencil paints: a finger's touch and hold shows the source.")

    /// What shows in place of the render.
    private var peekLabel: LocalizedStringResource {
        let source = paint.label(of: paint.selection, renders: model.history.renders).string
        return LocalizedStringResource(
            "Source · \(source)",
            comment: "Label over the picture in paint mode while the source is shown. The placeholder is the source, such as 'Frame 23', '#3' or 'Original'.")
    }
}
