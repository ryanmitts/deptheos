import SwiftUI
@preconcurrency import DeptheosEngine

/// The controls along the photo's foot outside Retouch: the home control,
/// the rail (`FocusRail`), the view menu, Flip in Flicker, and Stack or its
/// progress. Each control owns its glass (`deckGlass`), lit while on; the
/// rail's glass takes no touches, so a menu over it gets every tap.
/// ADR-163, ADR-149.
///
/// Laid out by its room: one row where everything fits, else two tiers
/// (home and rail above the view menu and Stack). Stack is left out where
/// the system's bars are vertical (`includesStack`); a render-less stack
/// has only the rail and Stack. While an opening render is on its way, home
/// and the view menu are laid out and off, so nothing reflows as it lands.
///
/// While a stack runs, the rail is the live map's key (`RunningRail`)
/// beside the progress, or under it.
struct ViewerDeck: View {
    static let depthMap = LocalizedStringResource(
        "Depth Map",
        comment: "The depth map view: which frame each area's detail came from. An item of the view menu under the photo, a View menu item and the label over the picture.")
    static let contributionLabel = LocalizedStringResource(
        "Contribution",
        comment: "Item of the view menu under the photo and View menu item, with a frame shown or compared with: shows what the result took from that frame as it is, the rest dimmed.")
    static let zoomToFit = LocalizedStringResource(
        "Zoom to Fit", comment: "View menu item (key F): fits the picture to the view.")
    /// As a toolbar button (Retouch), so a side bar can take it.
    static let zoomToFitSymbol = "arrow.up.left.and.down.right.magnifyingglass"
    static let compareLabel = LocalizedStringResource(
        "Compare",
        comment: "View menu item: lays the result out with a frame or another render, in the layout used last; off, the result alone.")
    /// The view menu's while nothing is on.
    static let compareSymbol = "square.split.2x1"
    /// A part kept whole, the rest faded.
    static let contributionSymbol = "circle.dashed.inset.filled"
    static let depthMapSymbol = "mountain.2"
    static let flipLabel = LocalizedStringResource(
        "Flip", comment: "Button beside the view menu in Flicker: switches between the result and what it is compared with.")
    /// The header of the menu of the stack's renders.
    static let rendersSection = LocalizedStringResource(
        "Renders of This Stack",
        comment: "Header of the menu section listing the stack's other renders, and the VoiceOver label and pointer tooltip of the menu of the stack's renders under the photo.")
    /// In the system's bar.
    static let stackSymbol = "square.3.layers.3d"
    /// With the glass's padding, a target of at least 44 points.
    static let symbolSide: CGFloat = 30

    @Environment(AppModel.self) private var model
    @Environment(\.accessibilityVoiceOverEnabled) private var voiceOver
    /// Not where the system's vertical bar has it.
    let includesStack: Bool
    /// Bumped per tap on the home control, for its haptic.
    @State private var homeTaps = 0
    /// Expands the 1-tap Liquid Glass View strip above the deck (matching
    /// Retouch's View pills).
    @State private var viewStripOpen = false

    var body: some View {
        let frames = model.railFrames
        // The live map is on screen, unless a frame is shown alone.
        let key = model.showsFrameAlone ? nil : model.stacking?.railKey
        Group {
            if let key, !key.colors.isEmpty {
                // Its key, and the progress.
                running(key)
            } else if model.stacking != nil || model.batchRun != nil {
                // Only the progress.
                HStack {
                    Spacer(minLength: 0)
                    StackCorner(showsButton: includesStack)
                }
            } else {
                VStack(spacing: 8) {
                    if viewStripOpen, hasResult {
                        viewStrip
                            .transition(.opacity.combined(with: .scale(scale: 0.98, anchor: .bottom)))
                    }
                    ViewThatFits(in: .horizontal) {
                        oneRow(frames)
                        twoTiers(frames)
                    }
                }
                .animation(.snappy(duration: 0.22), value: viewStripOpen)
            }
        }
        // Over the photo: it grows with text only so far.
        .dynamicTypeSize(...DynamicTypeSize.accessibility1)
        .foregroundStyle(.primary)
        .sensoryFeedback(.impact(weight: .light), trigger: homeTaps)
        .onChange(of: hasResult) { _, has in
            if !has { viewStripOpen = false }
        }
        .onChange(of: model.panel.hidesControls) { _, hidden in
            if hidden { viewStripOpen = false }
        }
        .onChange(of: announced) { _, text in
            guard voiceOver, let text else { return }
            AccessibilityNotification.Announcement(text.string).post()
        }
    }

    /// A result is shown, to compare and look into.
    private var hasResult: Bool { model.result?.display != nil }

    /// Shown or on its way (`namedRender`); home and the view menu are
    /// laid out, off until it is shown.
    private var hasRender: Bool { hasResult || model.namedRender != nil }

    private func oneRow(_ frames: [RailFrame]) -> some View {
        GlassEffectContainer(spacing: 8) {
            HStack(spacing: 8) {
                if hasRender { home }
                if frames.isEmpty {
                    Spacer(minLength: 0)
                } else {
                    rail(frames)
                        .frame(minWidth: 150, idealWidth: 200, maxWidth: .infinity)
                }
                if hasRender { viewControls }
                if includesStack {
                    StackCorner()
                }
            }
        }
    }

    private func twoTiers(_ frames: [RailFrame]) -> some View {
        GlassEffectContainer(spacing: 8) {
            VStack(spacing: 8) {
                if hasRender || !frames.isEmpty {
                    HStack(spacing: 8) {
                        if hasRender { home }
                        if frames.isEmpty {
                            Spacer(minLength: 0)
                        } else {
                            rail(frames).frame(maxWidth: .infinity)
                        }
                    }
                }
                if hasRender || includesStack {
                    HStack(spacing: 8) {
                        if hasRender { viewControls }
                        Spacer(minLength: 0)
                        if includesStack { StackCorner() }
                    }
                }
            }
        }
    }

    /// The live map's key beside the progress, or the progress above it.
    /// The progress keeps one width beside the rail, so the choice doesn't
    /// change as its words do.
    private func running(_ key: (colors: [LegendColor?], taken: Int)) -> some View {
        ViewThatFits(in: .horizontal) {
            HStack(spacing: 8) {
                runningRail(key).frame(minWidth: 200, maxWidth: .infinity)
                StackCorner(showsButton: includesStack).frame(width: Self.progressWidth, alignment: .trailing)
            }
            VStack(alignment: .trailing, spacing: 8) {
                StackCorner(showsButton: includesStack)
                runningRail(key).frame(maxWidth: .infinity)
            }
        }
    }

    /// Its widest (`StackCorner`).
    private static let progressWidth: CGFloat = 360

    /// It takes no touch, nor does its glass.
    private func runningRail(_ key: (colors: [LegendColor?], taken: Int)) -> some View {
        RunningRail(colors: key.colors, taken: key.taken)
            .glassEffect(.regular, in: .capsule)
    }

    // MARK: The rail

    /// A scrubber, not a button: its glass isn't interactive, so it never
    /// takes a touch meant for a menu over it. Over the depth map it is the
    /// legend: scrubbing lights a frame, and a tap on the lit one or a drag
    /// past the first shows the whole map (VoiceOver: `DepthMapLabel`).
    private func rail(_ frames: [RailFrame]) -> some View {
        FocusRail(
            frames: frames,
            selection: Binding(
                get: { model.railSelection(in: frames) },
                set: { index in
                    if let index, frames.indices.contains(index) {
                        model.scrub(to: frames[index].path)
                    } else if model.litFrame != nil {
                        model.unlightMap()
                    } else {
                        model.showResultAlone()
                    }
                }),
            homeBeforeFirst: hasResult && !model.comparing,
            reselectClears: model.litFrame != nil)
            .glassEffect(.regular, in: .capsule)
    }

    // MARK: Home

    /// The render shown, #4 ★, unlit: it is the default, and lit glass
    /// beside Stack would read as a second main action. With other renders,
    /// a menu of them; with none, a tap goes back to the result alone. Off
    /// while an opening render is on its way.
    @ViewBuilder
    private var home: some View {
        let render = model.namedRender
        if model.stackRenders.count > 1 {
            Menu {
                homeMenu
            } label: {
                DeckHomeLabel(render: render, fallback: CompareLayout.resultLabel, menu: true)
            }
            .deckGlass()
            .buttonBorderShape(.capsule)
            .fixedSize()
            .accessibilityLabel(Self.rendersSection)
            .accessibilityValue(render.map { Text(verbatim: $0.spokenLabel) } ?? Text(verbatim: ""))
            .help(Self.rendersSection)
            .disabled(!hasResult)
        } else {
            Button {
                homeTaps += 1
                model.showResultAlone()
            } label: {
                DeckHomeLabel(render: render, fallback: CompareLayout.resultLabel, menu: false)
            }
            .deckGlass()
            .buttonBorderShape(.capsule)
            .fixedSize()
            .accessibilityLabel(CompareLayout.resultLabel)
            .accessibilityValue(render.map { Text(verbatim: $0.spokenLabel) } ?? Text(verbatim: ""))
            .accessibilityAddTraits(model.viewState.isResultAlone ? .isSelected : [])
            .accessibilityHint(LocalizedStringResource(
                "Shows the result alone.", comment: "VoiceOver hint of the render shown, under the photo: a tap shows the result alone."))
            .help(LocalizedStringResource(
                "Show the result alone", comment: "Pointer tooltip of the render shown, under the photo."))
            .disabled(!hasResult)
        }
    }

    /// Newest first, the one shown checked, under an explicit Renders header
    /// with two-line descriptive labels (method + number + star, and settings
    /// summary underneath).
    private var homeMenu: some View {
        Section(Self.rendersSection) {
            ForEach(model.stackRenders, id: \.id) { render in
                let checked = model.showsRenderShown && render.id == model.result?.renderID
                let method = AlgorithmKind(render.settings.algorithm).label
                Toggle(isOn: Binding(get: { checked }, set: { _ in Task { await model.open(render) } })) {
                    Label {
                        Text("\(method) \(render.nameLabel(starred: true).string)")
                        Text(verbatim: AppWords.words.algorithmSummary(algorithm: render.settings.algorithm))
                    } icon: {
                        Image(systemName: RenderInfo.symbol)
                    }
                }
                .accessibilityLabel(Text(verbatim: render.spokenLabel))
            }
        }
        // Another render picked while one loads is shown instead.
        .disabled(model.held)
    }

    // MARK: The view menu

    /// With Flip beside it in Flicker; off while an opening render is on its
    /// way.
    private var viewControls: some View {
        HStack(spacing: 8) {
            viewMenu
            if case .flicker(let showing)? = model.compareState?.layout, model.comparing {
                Button {
                    model.flip()
                } label: {
                    Image(systemName: "arrow.left.arrow.right")
                        .font(.callout.weight(.medium))
                        .frame(width: Self.symbolSide, height: Self.symbolSide)
                }
                .deckGlass(lit: showing)
                .buttonBorderShape(.circle)
                .accessibilityLabel(Self.flipLabel)
                .accessibilityValue(showing
                    ? LocalizedStringResource(
                        "Showing what it is compared with",
                        comment: "VoiceOver value of Flip in Flicker: the frame or render is shown in place of the result.")
                    : LocalizedStringResource(
                        "Showing the result", comment: "VoiceOver value of Flip in Flicker: the result is shown."))
                .accessibilityShowsLargeContentViewer {
                    Label(Self.flipLabel, systemImage: "arrow.left.arrow.right")
                }
                .help(LocalizedStringResource(
                    "Switch between the result and what it is compared with", comment: "Pointer tooltip of Flip in Flicker."))
            }
        }
        .fixedSize()
        .disabled(!hasResult)
    }

    /// For its symbol and VoiceOver value: a compare's layout, else the
    /// depth map, else Contribution.
    private var viewShown: (symbol: String, label: LocalizedStringResource)? {
        if model.comparing, let layout = model.compareState?.layout.kind {
            return (layout.systemImage, layout.label)
        }
        if model.showsDepthMap { return (Self.depthMapSymbol, Self.depthMap) }
        if model.frameOverlay == .contribution { return (Self.contributionSymbol, Self.contributionLabel) }
        return nil
    }

    /// Toggles the 1-tap Liquid Glass View strip above the deck (matching
    /// Retouch's View pills). Its symbol is what's on, lit while active or open.
    private var viewMenu: some View {
        let shown = viewShown
        return Button {
            withAnimation(.snappy(duration: 0.22)) { viewStripOpen.toggle() }
        } label: {
            Image(systemName: shown?.symbol ?? Self.compareSymbol)
                .font(.callout.weight(.medium))
                .frame(width: Self.symbolSide, height: Self.symbolSide)
        }
        .deckGlass(lit: viewStripOpen || shown != nil)
        .buttonBorderShape(.circle)
        .accessibilityLabel(Self.viewLabel)
        .accessibilityValue(Text(shown?.label ?? CompareLayout.resultLabel))
        .accessibilityAddTraits(viewStripOpen ? .isSelected : [])
        .accessibilityShowsLargeContentViewer {
            Label(Self.viewLabel, systemImage: shown?.symbol ?? Self.compareSymbol)
        }
        .help(LocalizedStringResource(
            "Compare the result with a frame or another render, show what it took from a frame, or show the depth map",
            comment: "Pointer tooltip of the view button under the photo."))
    }

    /// 1-tap floating Liquid Glass strip above the deck: Result, the compare
    /// layouts, Contribution, Depth Map, and a compact Compare With menu when
    /// multiple renders exist—matching Retouch's View strip.
    private var viewStrip: some View {
        let layout = model.comparing ? model.compareState?.layout.kind : nil
        let canCompare = hasResult && model.compareFrame != nil
        return GlassEffectContainer(spacing: 6) {
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 6) {
                    viewStripPill(
                        title: CompareLayout.resultLabel,
                        systemImage: "square",
                        selected: model.viewState.isResultAlone
                    ) {
                        guard !model.viewState.isResultAlone else { return }
                        withAnimation(.snappy) { model.showResultAlone() }
                    }
                    ForEach(CompareLayout.Kind.allCases, id: \.self) { kind in
                        viewStripPill(
                            title: kind.label,
                            systemImage: kind.systemImage,
                            selected: layout == kind
                        ) {
                            withAnimation(.snappy) { model.setLayout(kind) }
                        }
                        .disabled(!canCompare)
                    }
                    if model.canShowContribution || model.result?.map != nil {
                        Divider().frame(height: 18).opacity(0.35)
                    }
                    if model.canShowContribution {
                        viewStripPill(
                            title: Self.contributionLabel,
                            systemImage: Self.contributionSymbol,
                            selected: model.contributionChosen
                        ) {
                            withAnimation(.snappy) { model.toggleContribution() }
                        }
                    }
                    if model.result?.map != nil {
                        viewStripPill(
                            title: Self.depthMap,
                            systemImage: Self.depthMapSymbol,
                            selected: model.showsDepthMap
                        ) {
                            withAnimation(.snappy) { model.toggleDepthMap() }
                        }
                    }
                    if !model.compareRenders.isEmpty, canCompare {
                        Divider().frame(height: 18).opacity(0.35)
                        compareWithPill
                    }
                }
                .padding(.horizontal, 2)
            }
            .scrollClipDisabled()
        }
    }

    private func viewStripPill(
        title: LocalizedStringResource, systemImage: String, selected: Bool, action: @escaping () -> Void
    ) -> some View {
        Button(action: action) {
            HStack(spacing: 5) {
                Image(systemName: systemImage)
                    .font(.callout.weight(.medium))
                Text(title)
                    .font(.caption.weight(.semibold))
                    .lineLimit(1)
            }
            .padding(.horizontal, 4)
            .frame(minHeight: Self.symbolSide)
        }
        .deckGlass(lit: selected)
        .buttonBorderShape(.capsule)
        .accessibilityAddTraits(selected ? .isSelected : [])
    }

    /// Compact menu pill on the View strip to pick which frame or render the
    /// result is compared against.
    private var compareWithPill: some View {
        let target = model.comparing ? model.compareWith : nil
        return Menu {
            if let frame = model.compareFrame {
                Section {
                    Toggle(isOn: Binding(get: { target?.framePath != nil }, set: { _ in
                        withAnimation(.snappy) { model.compare(withFrame: frame) }
                    })) {
                        Text(model.label(of: .frame(path: frame, overlay: .none)))
                    }
                }
            }
            Section(Self.rendersSection) {
                ForEach(model.compareRenders, id: \.id) { render in
                    let method = AlgorithmKind(render.settings.algorithm).label
                    Toggle(isOn: Binding(get: { target?.renderID == render.id }, set: { _ in
                        withAnimation(.snappy) { model.compare(withRender: render.id) }
                    })) {
                        Label {
                            Text("\(method) \(render.nameLabel(starred: true).string)")
                            Text(verbatim: AppWords.words.algorithmSummary(algorithm: render.settings.algorithm))
                        } icon: {
                            Image(systemName: RenderInfo.symbol)
                        }
                    }
                    .accessibilityLabel(Text(verbatim: render.spokenLabel))
                }
            }
        } label: {
            HStack(spacing: 4) {
                Text(Self.compareWithLabel)
                    .font(.caption.weight(.semibold))
                    .lineLimit(1)
                Image(systemName: "chevron.down")
                    .font(.caption2.weight(.semibold))
                    .foregroundStyle(.secondary)
            }
            .padding(.horizontal, 4)
            .frame(minHeight: Self.symbolSide)
        }
        .deckGlass(lit: target?.renderID != nil)
        .buttonBorderShape(.capsule)
    }

    static let viewLabel = LocalizedStringResource(
        "View",
        comment: "VoiceOver label of the view menu under the photo (Result, the compare layouts, Compare With, Contribution, Depth Map; in Retouch Painted, the layouts, Depth Map); its value is what is on, such as Split or Depth Map.")
    static let compareWithLabel = LocalizedStringResource(
        "Compare With",
        comment: "Submenu of the view menu under the photo: the frame on the rail and the stack's other renders, one of which the result is compared with.")

    /// Said by VoiceOver on change: "Comparing with Frame 23", "Comparing
    /// with render 3" (a word, not "number 3").
    private var announced: UserText? {
        guard model.comparing, let target = model.compareWith else { return nil }
        if let id = target.renderID, let number = model.history.renders.first(where: { $0.id == id })?.number {
            return .app(LocalizedStringResource(
                "Comparing with render \(number)",
                comment: "VoiceOver announcement when the result is now compared with another render. The placeholder is the render's number in its stack."))
        }
        guard let file = target.framePath, let number = model.frameNumber(of: file) else { return nil }
        let name = UserText.app(frameLabel(number))
        return .app(LocalizedStringResource(
            "Comparing with \(name.string)",
            comment: "VoiceOver announcement when what the result is compared with changes. The placeholder is a frame, such as 'Frame 23'."))
    }
}

extension View {
    /// Glass of its own, tinted with the accent while on, so the state reads
    /// over a dark photo. A button or menu takes it directly, never through
    /// a plain control under shared glass.
    func deckGlass(lit: Bool = false) -> some View {
        buttonStyle(.glass(lit ? .regular.tint(.accentColor) : .regular))
    }
}

/// "Depth #4 ★", with a chevron where a tap opens the Renders menu (`fallback`
/// with no render shown). Retouch's is `PaintFromLabel`.
struct DeckHomeLabel: View {
    let render: RenderInfo?
    let fallback: LocalizedStringResource
    let menu: Bool

    var body: some View {
        HStack(spacing: 4) {
            Image(systemName: RenderInfo.symbol).font(.caption)
            if let render {
                let method = AlgorithmKind(render.settings.algorithm).label
                Text("\(method) \(render.numberLabel.string)")
                    .monospacedDigit()
                    .lineLimit(1)
            } else {
                Text(fallback)
                    .monospacedDigit()
                    .lineLimit(1)
            }
            if render?.picked == true {
                Image(systemName: "star.fill").font(.caption2).foregroundStyle(.yellow)
            }
            if menu {
                Image(systemName: "chevron.down")
                    .font(.caption2.weight(.semibold))
                    .foregroundStyle(.secondary)
            }
        }
        .font(.callout.weight(.semibold))
        .padding(.horizontal, 2)
        .frame(minHeight: ViewerDeck.symbolSide)
    }
}
