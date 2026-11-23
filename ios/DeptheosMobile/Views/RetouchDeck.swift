import SwiftUI
@preconcurrency import DeptheosEngine

/// Active tray tab in the Photos-Edit–style Retouch session.
enum RetouchTab: String, CaseIterable, Hashable {
    case source
    case brush
    case view

    var label: LocalizedStringResource {
        switch self {
        case .source:
            LocalizedStringResource("Source", comment: "Retouch tray tab: choose the frame or render to paint from.")
        case .brush:
            LocalizedStringResource("Brush", comment: "Retouch tray tab: adjust brush/eraser size, hardness, opacity, flow and color tolerance.")
        case .view:
            LocalizedStringResource("View", comment: "Retouch tray tab: choose Painted, Split, Side by Side, Top and Bottom, or Depth Map.")
        }
    }
}

/// Which brush parameter's inline slider is active in the Retouch Brush tray.
enum BrushParameter: String, CaseIterable, Hashable {
    case size
    case hardness
    case opacity
    case flow
    case tolerance
}

/// Retouch's Photos-Edit–style contextual tray:
/// - In portrait / regular height: two floating Liquid Glass tiers at the foot
///   of the photo (contextual row above, persistent mode strip + Eye +
///   Eyedropper below).
/// - In compact height (iPhone landscape): mode strip + Eye + Eyedropper split
///   into a leading vertical rail, while Source's FocusRail stays along the
///   bottom and Brush/View float on the trailing edge.
struct RetouchDeck: View {
    @Environment(AppModel.self) private var model
    /// Bound by `PaintOverlay` so landscape can split leading/trailing rails.
    @Binding var tab: RetouchTab
    /// True while scrubbing Size or Hardness so `PaintCanvas` shows the live
    /// brush ring centered on the photo.
    var onPreviewBrush: (Bool) -> Void = { _ in }
    /// Compact vertical space (iPhone landscape): renders the mode rail
    /// vertically on the leading edge and contextual controls on the trailing
    /// edge (except Source's horizontal FocusRail).
    var splitRails = false
    /// When `splitRails` is true, which side this instance renders.
    enum RailRole { case full, leadingRail, trailingOrBottom }
    var railRole: RailRole = .full

    @State private var brushParam: BrushParameter = .size
    /// Bumped per Paint From pick, for its haptic.
    @State private var pickTaps = 0
    /// Over the deck for a moment as a render opens.
    @State private var hintShown = false
    /// Whether the source showed when the hold began, so a tap toggles and a
    /// hold shows it only while held.
    @State private var eyeHeld: (wasPeeking: Bool, since: Date)?
    /// Sliding accent capsule inside the mode switcher.
    @Namespace private var tabFill

    private var paint: PaintModel { model.paint }

    var body: some View {
        Group {
            switch railRole {
            case .full:
                fullTray
            case .leadingRail:
                leadingVerticalRail
            case .trailingOrBottom:
                trailingOrBottomContextual
            }
        }
        .dynamicTypeSize(...DynamicTypeSize.accessibility1)
        .foregroundStyle(.primary)
        .sensoryFeedback(.impact(weight: .light), trigger: pickTaps)
        .background {
            if railRole != .trailingOrBottom { keys }
        }
        .onChange(of: tab) { _, newTab in
            if newTab != .brush { onPreviewBrush(false) }
        }
        .onDisappear { onPreviewBrush(false) }
        .task(id: paint.renderID) {
            guard paint.renderID != nil, railRole != .trailingOrBottom else { return }
            hintShown = true
            try? await Task.sleep(for: .seconds(4))
            withAnimation(.easeOut(duration: 0.3)) { hintShown = false }
        }
    }

    // MARK: Full Two-Tier Tray (Portrait / iPad)

    private var fullTray: some View {
        GlassEffectContainer(spacing: 8) {
            VStack(spacing: 8) {
                contextualRow
                    .id(tab)
                    .transition(.opacity.combined(with: .scale(scale: 0.98, anchor: .bottom)))
                HStack(spacing: 8) {
                    modeStrip(vertical: false)
                    Spacer(minLength: 4)
                    eye
                    picker
                }
            }
            .animation(.snappy(duration: 0.22), value: tab)
        }
        .overlay(alignment: .top) {
            hintOverlay
        }
    }

    // MARK: Split Rails (Landscape / Compact Height)

    private var leadingVerticalRail: some View {
        GlassEffectContainer(spacing: 8) {
            VStack(spacing: 8) {
                modeStrip(vertical: true)
                eye
                picker
            }
        }
    }

    @ViewBuilder
    private var trailingOrBottomContextual: some View {
        GlassEffectContainer(spacing: 8) {
            switch tab {
            case .source:
                sourceRow(compact: false)
            case .brush:
                brushCard
                    .frame(maxWidth: 340)
            case .view:
                viewStrip(vertical: true)
            }
        }
        .animation(.snappy(duration: 0.22), value: tab)
    }

    // MARK: Contextual Upper Tier

    @ViewBuilder
    private var contextualRow: some View {
        switch tab {
        case .source:
            ViewThatFits(in: .horizontal) {
                sourceRow(compact: false)
                sourceRow(compact: true)
            }
        case .brush:
            brushCard
        case .view:
            viewStrip(vertical: false)
        }
    }

    private func sourceRow(compact: Bool) -> some View {
        let frames = model.railFrames
        return HStack(spacing: Self.joint) {
            paintFrom(compact: compact)
            rail(frames)
                .frame(minWidth: compact ? nil : 150, maxWidth: .infinity)
        }
    }

    /// Standard container spacing so Paint From and FocusRail sit cleanly
    /// side by side without Liquid Glass edge tension.
    private static let joint: CGFloat = 8

    // MARK: Inline Brush Tray

    private var brushCard: some View {
        VStack(spacing: 10) {
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 6) {
                    brushEraserPill
                    Divider().frame(height: 18).opacity(0.4)
                    paramChip(.size, title: BrushSliders.size, value: .app(paint.radiusLabel))
                    paramChip(
                        .hardness,
                        title: LocalizedStringResource("Hardness", comment: "Brush slider, a name: how sharp the brush's edge is."),
                        value: .verbatim(paint.hardness.formatted(.number.precision(.fractionLength(2)))))
                    paramChip(
                        .opacity,
                        title: LocalizedStringResource("Opacity", comment: "Brush slider, a name: how much one stroke paints at most."),
                        value: .verbatim(paint.opacity.formatted(.percent.precision(.fractionLength(0)))))
                    paramChip(
                        .flow,
                        title: LocalizedStringResource("Flow", comment: "Brush slider, a name: how much each dab of a stroke paints."),
                        value: .verbatim(paint.flow.formatted(.percent.precision(.fractionLength(0)))))
                    paramChip(
                        .tolerance,
                        title: LocalizedStringResource("Color Match", comment: "Compact chip in Retouch's inline brush tray for Only where colors match."),
                        value: paint.toleranceOn
                            ? .verbatim(paint.tolerance.formatted(.percent.precision(.fractionLength(0))))
                            : .app(LocalizedStringResource("Off", comment: "Value shown on Color Match chip when disabled.")))
                }
                .padding(.horizontal, 2)
            }
            activeBrushSlider
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
        .glassEffect(.regular, in: RoundedRectangle(cornerRadius: 22))
    }

    private var brushEraserPill: some View {
        HStack(spacing: 2) {
            miniToolButton(
                LocalizedStringResource("Brush", comment: "Retouch's inline brush tray: the brush tool."),
                systemImage: "paintbrush.pointed",
                selected: !paint.erasing
            ) { paint.useBrush() }
            miniToolButton(
                LocalizedStringResource("Eraser", comment: "Retouch's inline brush tray: the eraser tool."),
                systemImage: "eraser",
                selected: paint.erasing
            ) { paint.useEraser() }
        }
        .padding(3)
        .background(.white.opacity(0.08), in: Capsule())
    }

    private func miniToolButton(
        _ title: LocalizedStringResource, systemImage: String, selected: Bool, action: @escaping () -> Void
    ) -> some View {
        Button(action: action) {
            Image(systemName: systemImage)
                .font(.caption.weight(.semibold))
                .foregroundStyle(selected ? .white : .primary)
                .frame(width: 30, height: 26)
                .background {
                    if selected { Capsule().fill(Color.accentColor) }
                }
                .contentShape(Capsule())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(title)
        .accessibilityAddTraits(selected ? .isSelected : [])
    }

    private func paramChip(
        _ param: BrushParameter, title: LocalizedStringResource, value: UserText
    ) -> some View {
        let selected = brushParam == param
        return Button {
            withAnimation(.snappy(duration: 0.18)) { brushParam = param }
        } label: {
            HStack(spacing: 4) {
                Text(title)
                Text(value)
                    .monospacedDigit()
                    .foregroundStyle(selected ? .white.opacity(0.85) : .secondary)
            }
            .font(.caption.weight(.semibold))
            .lineLimit(1)
            .padding(.horizontal, 10)
            .padding(.vertical, 6)
            .foregroundStyle(selected ? .white : .primary)
            .background(selected ? AnyShapeStyle(Color.accentColor) : AnyShapeStyle(.white.opacity(0.1)), in: Capsule())
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(selected ? .isSelected : [])
    }

    @ViewBuilder
    private var activeBrushSlider: some View {
        @Bindable var paint = model.paint
        let ranges = paint.ranges
        switch brushParam {
        case .size:
            HStack(spacing: 10) {
                Image(systemName: "circle.fill").font(.system(size: 6)).foregroundStyle(.secondary)
                Slider(
                    value: Binding(get: { log2(paint.radius) }, set: { paint.radius = exp2($0) }),
                    in: log2(ranges.radius.min)...log2(ranges.radius.max),
                    onEditingChanged: { editing in onPreviewBrush(editing) })
                Image(systemName: "circle.fill").font(.system(size: 14)).foregroundStyle(.secondary)
            }
            .accessibilityLabel(BrushSliders.size)
            .accessibilityValue(Text(paint.radiusLabel))
        case .hardness:
            HStack(spacing: 10) {
                Text("Soft", comment: "Low end of inline Hardness slider in Retouch.")
                    .font(.caption2).foregroundStyle(.secondary)
                Slider(
                    value: $paint.hardness,
                    in: ranges.hardness.min...ranges.hardness.max,
                    onEditingChanged: { editing in onPreviewBrush(editing) })
                Text("Hard", comment: "High end of inline Hardness slider in Retouch.")
                    .font(.caption2).foregroundStyle(.secondary)
            }
            .accessibilityLabel(LocalizedStringResource("Hardness", comment: "Brush slider, a name: how sharp the brush's edge is."))
        case .opacity:
            Slider(value: $paint.opacity, in: ranges.opacity.min...ranges.opacity.max)
                .accessibilityLabel(LocalizedStringResource("Opacity", comment: "Brush slider, a name: how much one stroke paints at most."))
        case .flow:
            Slider(value: $paint.flow, in: ranges.flow.min...ranges.flow.max)
                .accessibilityLabel(LocalizedStringResource("Flow", comment: "Brush slider, a name: how much each dab of a stroke paints."))
        case .tolerance:
            HStack(spacing: 10) {
                Toggle(ToleranceSlider.onlyWhereColorsMatch, isOn: $paint.toleranceOn)
                    .labelsHidden()
                    .tint(.accentColor)
                Slider(value: $paint.tolerance, in: ranges.tolerance.min...ranges.tolerance.max)
                    .disabled(!paint.toleranceOn)
                    .opacity(paint.toleranceOn ? 1 : 0.4)
            }
            .accessibilityElement(children: .contain)
        }
    }

    // MARK: Direct View-Mode Pills

    @ViewBuilder
    private func viewStrip(vertical: Bool) -> some View {
        let layout = paint.view.layout.kind
        let pills = ForEach(PaintLayout.Kind.allCases, id: \.self) { kind in
            let selected = !paint.showsDepthMap && layout == kind
            Button {
                withAnimation(.snappy) {
                    if paint.showsDepthMap { paint.toggleDepthMap() }
                    paint.setLayout(kind)
                }
            } label: {
                viewPillLabel(title: kind.label, systemImage: kind.systemImage, vertical: vertical)
            }
            .deckGlass(lit: selected)
            .buttonBorderShape(.capsule)
            .accessibilityAddTraits(selected ? .isSelected : [])
        }
        let depthPill = Button {
            withAnimation(.snappy) { paint.toggleDepthMap() }
        } label: {
            viewPillLabel(title: ViewerDeck.depthMap, systemImage: ViewerDeck.depthMapSymbol, vertical: vertical)
        }
        .deckGlass(lit: paint.showsDepthMap)
        .buttonBorderShape(.capsule)
        .disabled(model.result?.map == nil)
        .accessibilityAddTraits(paint.showsDepthMap ? .isSelected : [])

        if vertical {
            VStack(spacing: 6) {
                pills
                Divider().frame(width: 22).opacity(0.35)
                depthPill
            }
        } else {
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 6) {
                    pills
                    Divider().frame(height: 18).opacity(0.35)
                    depthPill
                }
                .padding(.horizontal, 2)
            }
            .scrollClipDisabled()
        }
    }

    private func viewPillLabel(
        title: LocalizedStringResource, systemImage: String, vertical: Bool
    ) -> some View {
        HStack(spacing: 5) {
            Image(systemName: systemImage)
                .font(.callout.weight(.medium))
            if !vertical {
                Text(title)
                    .font(.caption.weight(.semibold))
                    .lineLimit(1)
            }
        }
        .padding(.horizontal, vertical ? 2 : 4)
        .frame(minWidth: vertical ? ViewerDeck.symbolSide : nil, minHeight: ViewerDeck.symbolSide)
    }

    // MARK: Mode Strip (Source | Brush | View)

    @ViewBuilder
    private func modeStrip(vertical: Bool) -> some View {
        let items = ForEach(RetouchTab.allCases, id: \.self) { item in
            let selected = tab == item
            Button {
                withAnimation(.snappy(duration: 0.22)) { tab = item }
            } label: {
                HStack(spacing: 5) {
                    Image(systemName: symbol(for: item))
                        .font(.callout.weight(.medium))
                    if !vertical {
                        Text(title(for: item))
                            .font(.caption.weight(.semibold))
                            .lineLimit(1)
                    }
                }
                .foregroundStyle(selected ? .white : .primary)
                .padding(.horizontal, vertical ? 0 : 12)
                .frame(width: vertical ? 38 : nil, height: 36)
                .background {
                    if selected {
                        Capsule().fill(Color.accentColor)
                            .matchedGeometryEffect(id: "retouch-tab", in: tabFill)
                    }
                }
                .contentShape(Capsule())
            }
            .buttonStyle(.plain)
            .accessibilityLabel(title(for: item))
            .accessibilityAddTraits(selected ? .isSelected : [])
        }
        if vertical {
            VStack(spacing: 2) { items }
                .padding(Self.toolInset)
                .glassEffect(.regular, in: .capsule)
                .fixedSize()
        } else {
            HStack(spacing: 2) { items }
                .padding(Self.toolInset)
                .glassEffect(.regular, in: .capsule)
                .fixedSize()
        }
    }

    private func symbol(for item: RetouchTab) -> String {
        switch item {
        case .source: return Self.framesSymbol
        case .brush: return paint.erasing ? "eraser" : "paintbrush.pointed"
        case .view:
            if paint.showsDepthMap { return ViewerDeck.depthMapSymbol }
            let kind = paint.view.layout.kind
            return kind == .canvas ? ViewerDeck.compareSymbol : kind.systemImage
        }
    }

    private func title(for item: RetouchTab) -> LocalizedStringResource {
        if item == .brush, paint.erasing {
            return LocalizedStringResource("Eraser", comment: "Retouch tray tab when Eraser is selected.")
        }
        return item.label
    }

    private static let toolInset: CGFloat = 4

    // MARK: Hint Overlay

    private var hintOverlay: some View {
        Color.clear
            .frame(maxWidth: .infinity, maxHeight: 0)
            .overlay(alignment: .bottomLeading) {
                if hintShown, !paint.peeking, paint.touch == .paints, model.notice == nil {
                    Text(hint)
                        .font(.caption)
                        .padding(.horizontal, 10)
                        .padding(.vertical, 5)
                        .glassEffect(.regular, in: .capsule)
                        .fixedSize()
                        .padding(.bottom, 8)
                        .allowsHitTesting(false)
                        .transition(.opacity)
                }
            }
    }

    // MARK: Paint From

    private func paintFrom(compact: Bool) -> some View {
        let renders = model.history.renders
        let source = paint.selection
        return Menu {
            paintFromMenu
        } label: {
            PaintFromLabel(source: source, renders: renders, compact: compact)
        }
        .deckGlass(lit: paint.sourceFrame == nil)
        .buttonBorderShape(.capsule)
        .fixedSize()
        .accessibilityLabel(Self.paintFromTitle)
        .accessibilityValue(Text(verbatim: spoken(source, renders: renders)))
        .help(Self.paintFromTitle)
    }

    private func spoken(_ source: SourceKind?, renders: [RenderInfo]) -> String {
        if case .render(let id)? = source, let render = renders.first(where: { $0.id == id }) {
            return render.spokenNumber
        }
        return paint.label(of: source, renders: renders).string
    }

    @ViewBuilder
    private var paintFromMenu: some View {
        Toggle(isOn: Binding(get: { paint.sourceFrame != nil }, set: { _ in
            guard paint.sourceFrame == nil else { return }
            pickTaps += 1
            model.showPaintSources(.frames)
            paint.useFrames()
        })) {
            Label { Text(ListScreen.frames) } icon: { Image(systemName: Self.framesSymbol) }
        }
        .disabled(paint.frames.isEmpty)
        if !model.paintRenders.isEmpty {
            Section {
                ForEach(model.paintRenders, id: \.id) { render in
                    Toggle(isOn: Binding(get: { paint.sourceRender == render.id }, set: { _ in
                        guard paint.sourceRender != render.id else { return }
                        pickTaps += 1
                        model.showPaintSources(.renders)
                        model.paintFrom(render: render.id)
                    })) {
                        Label { Text(render.menuTitle) } icon: { Image(systemName: RenderInfo.symbol) }
                    }
                    .accessibilityLabel(Text(verbatim: render.spokenLabel))
                }
            }
        }
        Section {
            Toggle(isOn: Binding(get: { paint.erasing }, set: { _ in
                guard !paint.erasing else { return }
                pickTaps += 1
                paint.useEraser()
            })) {
                Label { Text(Self.originalLabel) } icon: { Image(systemName: Self.originalSymbol) }
                Text("Paints the render back as stacked", comment: "Under Original, the last item of the Paint From menu under the photo in Retouch.")
            }
        }
    }

    static let framesSymbol = "photo.stack"
    static let originalSymbol = "eraser"

    static let originalLabel = LocalizedStringResource(
        "Original", comment: "What the brush paints from: the render as stacked, which the eraser and Original in the Paint From menu under the photo paint back. After 'Source · ' over the source's picture.")

    static let paintFromTitle = LocalizedStringResource(
        "Paint From", comment: "VoiceOver label and pointer tooltip of the Paint From menu under the photo in Retouch: the frames, the renders and the original the brush can paint from. Its value is what is painted from.")

    // MARK: The rail

    private func rail(_ frames: [RailFrame]) -> some View {
        FocusRail(
            frames: frames,
            selection: Binding(
                get: { paint.sourceFrame.flatMap { file in frames.firstIndex { $0.path == file } } },
                set: { index in
                    if let index, frames.indices.contains(index) { paint.scrub(to: frames[index].path) }
                }),
            marks: model.paintedMarks(in: frames))
            .glassEffect(.regular, in: .capsule)
    }

    // MARK: Eye & Eyedropper

    private var eye: some View {
        Image(systemName: paint.peeking ? "eye.fill" : "eye")
            .font(.callout.weight(.medium))
            .frame(width: 44, height: 44)
            .contentShape(Circle())
            .glassEffect(
                paint.peeking ? .regular.tint(.accentColor).interactive() : .regular.interactive(), in: .circle)
            .gesture(
                DragGesture(minimumDistance: 0)
                    .onChanged { _ in
                        guard eyeHeld == nil else { return }
                        eyeHeld = (paint.peeking, .now)
                        paint.setPeeking(true)
                    }
                    .onEnded { _ in
                        guard let held = eyeHeld else { return }
                        eyeHeld = nil
                        let tapped = Date.now.timeIntervalSince(held.since) < 0.3
                        paint.setPeeking(tapped ? !held.wasPeeking : held.wasPeeking)
                    })
            .accessibilityElement()
            .accessibilityLabel(Self.showSource)
            .accessibilityAddTraits(paint.peeking ? [.isButton, .isSelected] : .isButton)
            .accessibilityAction { paint.togglePeek() }
            .accessibilityShowsLargeContentViewer {
                Label(Self.showSource, systemImage: "eye")
            }
            .help(hint)
    }

    static let showSource = LocalizedStringResource(
        "Show the source", comment: "VoiceOver label of the eye under the photo in Retouch: shows what the brush paints from in place of the painted result.")

    private var picker: some View {
        let on = paint.touch == .picksFrame
        return Button {
            paint.toggleFramePicker()
        } label: {
            Image(systemName: "eyedropper")
                .font(.callout.weight(.medium))
                .frame(width: ViewerDeck.symbolSide, height: ViewerDeck.symbolSide)
        }
        .deckGlass(lit: on)
        .buttonBorderShape(.circle)
        .disabled(!paint.canPickAtPoint)
        .accessibilityLabel(Self.sharpestFrame)
        .accessibilityAddTraits(on ? [.isToggle, .isSelected] : .isToggle)
        .accessibilityHint(Self.pickHint)
        .accessibilityShowsLargeContentViewer {
            Label(Self.sharpestFrame, systemImage: "eyedropper")
        }
        .help(Self.pickHint)
    }

    static let sharpestFrame = LocalizedStringResource(
        "Sharpest Frame", comment: "Button under the photo in Retouch (VoiceOver label): the next tap on the photo paints from the frame sharpest there.")
    static let pickHint = LocalizedStringResource(
        "Tap the photo to paint from the frame sharpest there",
        comment: "Retouch: the hint over the photo while Sharpest Frame is on, and that button's VoiceOver hint and pointer tooltip.")

    private var hint: LocalizedStringResource {
        paint.onlyPencilPaints
            ? PaintOverlay.holdHint
            : LocalizedStringResource(
                "Touch and hold the eye to see the source",
                comment: "Retouch: the hint shown for a moment over the deck under the photo, and the eye's help, when a finger paints too.")
    }

    // MARK: Keys

    private var keys: some View {
        ZStack {
            Button { paint.scaleRadius(by: PaintModel.smallerStep) } label: { EmptyView() }
                .keyboardShortcut("[", modifiers: [])
            Button { paint.scaleRadius(by: PaintModel.largerStep) } label: { EmptyView() }
                .keyboardShortcut("]", modifiers: [])
            Button { paint.nudgeHardness(by: -PaintModel.hardnessStep) } label: { EmptyView() }
                .keyboardShortcut("[", modifiers: .shift)
            Button { paint.nudgeHardness(by: PaintModel.hardnessStep) } label: { EmptyView() }
                .keyboardShortcut("]", modifiers: .shift)
            Button { paint.togglePeek() } label: { EmptyView() }
                .keyboardShortcut("\\", modifiers: [])
        }
        .opacity(0)
        .allowsHitTesting(false)
        .accessibilityHidden(true)
    }
}

/// Paint From's label: the source led by its kind's symbol ("Frames", "#2
/// ★", "Original"), with the chevron.
private struct PaintFromLabel: View {
    @Environment(AppModel.self) private var model
    let source: SourceKind?
    let renders: [RenderInfo]
    var compact = false
    @ScaledMetric(relativeTo: .callout) private var minWidth: CGFloat = 104
    @ScaledMetric(relativeTo: .callout) private var compactMinWidth: CGFloat = 44

    var body: some View {
        HStack(spacing: 4) {
            Image(systemName: symbol).font(compact && render == nil ? .callout : .caption)
            if !compact || render != nil {
                Text(text)
                    .monospacedDigit()
                    .lineLimit(1)
            }
            if render?.picked == true {
                Image(systemName: "star.fill").font(.caption2).foregroundStyle(.yellow)
            }
            Image(systemName: "chevron.down")
                .font(.caption2.weight(.semibold))
                .foregroundStyle(.secondary)
        }
        .font(.callout.weight(.semibold))
        .padding(.horizontal, 2)
        .frame(minWidth: compact ? compactMinWidth : minWidth, minHeight: ViewerDeck.symbolSide)
    }

    private var render: RenderInfo? {
        guard case .render(let id)? = source else { return nil }
        return renders.first { $0.id == id }
    }

    private var symbol: String {
        switch source {
        case .frame?, nil: RetouchDeck.framesSymbol
        case .render?: RenderInfo.symbol
        case .original?: RetouchDeck.originalSymbol
        }
    }

    private var text: UserText {
        switch source {
        case .frame?, nil: .app(ListScreen.frames)
        case .render?, .original?: model.paint.label(of: source, renders: renders)
        }
    }
}
