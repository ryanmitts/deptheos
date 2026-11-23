import SwiftUI
import UIKit
@preconcurrency import DeptheosEngine

private let viewerLog = AppLog(category: "viewer")

/// The selected source, the result or its map, with pinch to zoom; the
/// result laid out with its compare; during a stack, the live map and its
/// progress. While a stack opens, its base's thumbnail stands in
/// (`AppModel.standIn`).
///
/// The photo runs full bleed under the bars and the controls float over it
/// inside the safe area, hidden by a tap (`Panel`). A sheet never rescales
/// the photo: a low sheet only limits how far a zoomed photo drags clear
/// (`below`). ADR-163.
struct ViewerView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.verticalBarEdge) private var verticalBarEdge
    @Environment(\.verticalSizeClass) private var verticalSizeClass
    @Environment(\.accessibilityVoiceOverEnabled) private var voiceOver
    @Environment(\.layoutDirection) private var layoutDirection
    /// Compact width: the photo runs across, and a compared frame's label
    /// is its number alone.
    var phone = false
    /// The sources column is beside the photo (regular width), so the
    /// photo doesn't run under a leading side bar.
    var sidebarShown = false
    /// Held, the result gives way to the frame selected until let go.
    @State private var holding = false
    @State private var safeArea = EdgeInsets()
    /// The photo's height under the bars.
    @State private var bleedHeight: CGFloat = 0
    /// With its padding.
    @State private var bottomRow: CGFloat = 0
    /// The fold and inner camera where they cross the controls (iOS 27.1).
    @State private var reserved = ReservedAreas()
    /// A finger scrubs the rail: the panes' labels give way to its loupe.
    @State private var scrubbing = false

    var body: some View {
        // The backdrop and photo fill what they are offered, so the
        // viewer's size and place are its parent's alone; everything placed
        // from a measurement is in the overlay, which can't change them
        // (ADR-169).
        ZStack {
            // `viewer` in design/tokens.json, generated into the asset
            // catalog: the same dark gray in light and dark mode.
            Color.viewer.ignoresSafeArea()
            photo
                .scrollEdgeEffectHidden(true, for: .all)
                .ignoresSafeArea(.container, edges: photoEdges)
        }
        .overlay {
            ZStack {
                if !model.paint.active, !showsPhoto {
                    placeholder
                }
                if !model.paint.active, !model.panel.hidesControls {
                    controls
                        .transition(.opacity)
                }
                if holding {
                    VStack {
                        PhotoLabel(text: .app(compareLabel))
                            .padding(.top, 12 + occluded.top)
                        Spacer()
                    }
                    .allowsHitTesting(false)
                }
                #if DEBUG
                if E2E.frames != nil { E2EViewProbe() }
                #endif
            }
        }
        .background {
            // How deep the bars and screen edges are on the edges it runs
            // under, for the labels and Split's divider: one reading, in
            // whole points. The photo reads its own (`TiledZoomView`).
            Color.clear
                .measure("viewer safe area", for: Bleed.self) { [space = Self.space, rtl = layoutDirection == .rightToLeft] in Bleed($0, viewer: space, rtl: rtl) } action: {
                    bleedHeight = $0.height
                    setSafeArea($0.insets)
                }
                .ignoresSafeArea(.container, edges: photoEdges)
        }
        .coordinateSpace(.named(Self.space))
        .measure("viewer reserved areas", for: ReservedAreas.self) { ReservedAreas.read($0) } action: { areas in
            reserved = areas
            viewerLog.debug(
                "viewer reserved: fold \(String(describing: areas.fold), privacy: .public), occlusions \(String(describing: areas.occlusions), privacy: .public) in \(String(describing: areas.size), privacy: .public)")
        }
        // Light text and controls on the dark backdrop in either mode.
        .environment(\.colorScheme, .dark)
        .environment(\.viewerTap, ViewerTapAction {
            withAnimation(.easeInOut(duration: 0.2)) { model.toggleControls(voiceOver: voiceOver) }
        })
        // VoiceOver needs the controls to find.
        .onChange(of: voiceOver, initial: true) { _, on in
            if on { model.showControls() }
        }
        .sensoryFeedback(.selection, trigger: holding) { _, new in new }
        // The compare loads whenever comparing begins, however it does.
        .onChange(of: model.comparing, initial: true) { _, comparing in
            if comparing { model.refreshCompare() }
        }
        .onChange(of: canHold) { _, can in
            if !can { holding = false }
        }
        .onPreferenceChange(RailScrubbing.self) { scrubbing in
            withAnimation(.easeOut(duration: 0.15)) { self.scrubbing = scrubbing }
        }
    }

    private static let space = "viewer"

    /// The photo's height and the insets it runs under: the viewer's
    /// bounds read from the photo's, in whole points.
    nonisolated private struct Bleed: Equatable, Sendable {
        var height: CGFloat
        var insets: EdgeInsets

        init(_ proxy: GeometryProxy, viewer space: String, rtl: Bool) {
            height = proxy.size.height
            guard let inner = proxy.bounds(of: .named(space)) else {
                insets = EdgeInsets()
                return
            }
            let left = max(inner.minX, 0)
            let right = max(proxy.size.width - inner.maxX, 0)
            insets = EdgeInsets(
                top: max(inner.minY, 0), leading: rtl ? right : left,
                bottom: max(proxy.size.height - inner.maxY, 0), trailing: rtl ? left : right
            ).rounded
        }
    }

    private func setSafeArea(_ insets: EdgeInsets) {
        guard insets != safeArea else { return }
        safeArea = insets
        viewerLog.debug("viewer safe area: \(String(describing: insets), privacy: .public)")
    }

    /// Top and bottom always; across at compact width; at regular width the
    /// side bar's edge while no column is there.
    private var photoEdges: Edge.Set {
        if phone { return .all }
        var edges: Edge.Set = [.top, .bottom]
        switch verticalBarEdge {
        case .leading? where !sidebarShown: edges.insert(.leading)
        case .trailing?: edges.insert(.trailing)
        default: break
        }
        return edges
    }

    /// The chrome's depth over each edge: bars, screen edges, and the bottom
    /// row. Compare labels and Split's divider keep inside it; the photo
    /// reads the bars' depth itself. Retouch places its own (`PaintControls`).
    private var chrome: UIEdgeInsets {
        let below = model.panel.hidesControls ? 0 : bottomRow
        let leading = safeArea.leading
        let trailing = safeArea.trailing
        let rtl = layoutDirection == .rightToLeft
        return UIEdgeInsets(
            top: safeArea.top, left: rtl ? trailing : leading,
            bottom: safeArea.bottom + below, right: rtl ? leading : trailing)
    }

    /// What a zoomed photo drags clear of at the bottom past the safe
    /// area, which it reads itself (`ZoomToFit.swift`): the bottom row, or
    /// under a low sheet, its depth.
    private var below: Below {
        if let lowSheetDepth { return .sheet(depth: lowSheetDepth) }
        return .row(model.panel.hidesControls ? 0 : bottomRow)
    }

    /// A low sheet's depth from the screen's lower edge: half the screen
    /// (its drag isn't followed). Only at compact width and not on a short
    /// screen, whose sheets take the whole height; nil otherwise.
    private var lowSheetDepth: CGFloat? {
        guard phone, verticalSizeClass != .compact, model.panel.height == .low, bleedHeight > 0
        else { return nil }
        return bleedHeight / 2
    }

    /// From the controls' lower edge (8 points up) to a low sheet's top,
    /// for the notices to sit above it.
    private var noticeLift: CGFloat? {
        lowSheetDepth.map { max($0 - safeArea.bottom, 0) }
    }

    /// The photo itself, under the bars.
    @ViewBuilder
    private var photo: some View {
        if model.paint.active {
            Color.viewer
        } else if model.comparing, let display = model.result?.displayPane {
            CompareView(
                result: display, below: below, controlsHidden: model.panel.hidesControls,
                chrome: chrome, labelInsets: safeArea,
                showsLabels: !model.panel.hidesControls && !scrubbing, compact: phone)
        } else if let picture {
            ZoomableImage(
                // A frame in the result's place keeps its zoom, as the hold
                // does.
                picture: picture, keepsFrame: holding || model.showsFrameAlone,
                onHold: canHold ? { held in
                    holding = held
                    // The frame held, at its full size.
                    if held { model.requestFullFrame(held: true) }
                } : nil,
                fitRequest: model.fitRequest, below: below,
                controlsHidden: model.panel.hidesControls,
                // A stand-in's zoom isn't kept: its picture lands at fit.
                remembered: model.viewerZoom,
                onZoom: model.standIn == nil ? { model.viewerZoom = $0 } : nil,
                onImageTap: mapTap)
                .accessibilityPhoto(photoLabel)
                .accessibilityHint(canHold
                    ? LocalizedStringResource(
                        "Touch and hold to see the source frame",
                        comment: "VoiceOver hint on the result: touch and hold shows the frame selected in its place.")
                    : .empty)
        }
    }

    /// A tap on a lightable depth map with the controls shown
    /// (`AppModel.tapMap`). Off the picture, or with controls hidden, it
    /// toggles them as on any photo.
    private var mapTap: ((CGPoint) -> Bool)? {
        guard model.canLightMap, !model.panel.hidesControls, !holding else { return nil }
        return { point in
            withAnimation(.snappy) { model.tapMap(at: point) }
        }
    }

    /// "Depth Map ✕", "Depth Map · Frame 18 ✕" while lit: a tap goes back
    /// to what the map came from, as Retouch's label does.
    private var mapLabel: some View {
        DepthMapLabel(
            text: shownLabel,
            help: LocalizedStringResource(
                "Close the depth map", comment: "Pointer tooltip of the Depth Map label at the top of the photo outside Retouch."),
            close: { withAnimation(.snappy) { model.toggleDepthMap() } },
            wholeMap: model.litFrame == nil ? nil : { model.unlightMap() })
    }

    /// Else the placeholder says why not.
    private var showsPhoto: Bool {
        (model.comparing && model.result?.display != nil) || picture != nil
    }

    /// Over the photo, inside the safe area: the label, the notices, and
    /// the bottom row.
    private var controls: some View {
        VStack(spacing: 8) {
            VStack(spacing: 8) {
                if showsResultLabel {
                    HStack {
                        if model.showsDepthMap {
                            mapLabel
                        } else {
                            PhotoLabel(text: shownLabel).allowsHitTesting(false)
                        }
                        Spacer().allowsHitTesting(false)
                    }
                }
            }
            .padding(.top, occluded.top)
            Spacer(minLength: 0)
            if showsNotices, noticeLift == nil {
                notices
            }
            bottomRowView
        }
        // Above a low sheet, which covers the bottom row, so a notice stays
        // seen.
        .overlay(alignment: .bottom) {
            if showsNotices, let noticeLift {
                notices.padding(.bottom, noticeLift)
            }
        }
        .padding(.leading, sidePadding + occluded.leading)
        .padding(.trailing, sidePadding + occluded.trailing)
        .padding(.top, 8)
        .padding(.bottom, 8 + occluded.bottom)
    }

    /// Not during a stack.
    private var showsNotices: Bool {
        model.stacking == nil && model.batchRun == nil
    }

    /// Frames just added and the last batch: one column, above the bottom
    /// row or a low sheet.
    @ViewBuilder
    private var notices: some View {
        VStack(spacing: 8) {
            if let added = model.addedToExisting {
                AddedToStackBanner(added: added)
                    .padding(.horizontal, 14)
                    .padding(.vertical, 10)
                    .frame(maxWidth: 420)
                    .glassEffect(.regular, in: RoundedRectangle(cornerRadius: 16))
            }
            if let notice = model.notice {
                // Tap to hide.
                Text(notice)
                    .font(.footnote)
                    .multilineTextAlignment(.center)
                    .padding(.horizontal, 14)
                    .padding(.vertical, 10)
                    .frame(maxWidth: 420)
                    .glassEffect(.regular, in: RoundedRectangle(cornerRadius: 16))
                    .onTapGesture { model.notice = nil }
                    .accessibilityAddTraits(.isButton)
                    .accessibilityHint(LocalizedStringResource(
                        "Hides this notice", comment: "VoiceOver hint of the notice over the photo: a tap hides it."))
            }
        }
    }

    /// The deck (`ViewerDeck`) and Stack. Where a fold divides the viewer
    /// top to bottom, the deck keeps leading and Stack trailing; where the
    /// system's bars are a vertical strip, Stack is in it.
    @ViewBuilder
    private var bottomRowView: some View {
        let shows = model.stacking != nil || model.batchRun != nil || model.result != nil || !model.rows.isEmpty
        let stackHere = verticalBarEdge == nil
        Group {
            if !shows {
                HStack {
                    Spacer(minLength: 0)
                    if stackHere { StackCorner() }
                }
            } else if let sides = foldSides {
                HStack(alignment: .bottom, spacing: 0) {
                    ViewerDeck(includesStack: false)
                        .frame(width: sides.leading)
                    Color.clear.frame(width: sides.gutter, height: 1)
                    HStack(spacing: 0) {
                        Spacer(minLength: 0)
                        if model.stacking == nil, model.batchRun == nil, stackHere { StackCorner() }
                    }
                }
            } else {
                ViewerDeck(includesStack: stackHere)
            }
        }
        .environment(\.automationLoupe, Automation.flag("RailLoupe"))
        // 8 pt bottom padding + 12 pt breathing room above the deck / View strip.
        .measure("viewer bottom row", for: CGFloat.self) { $0.size.height + 20 } action: { bottomRow = $0 }
    }

    private var sidePadding: CGFloat { phone ? 16 : 12 }

    /// The bottom row's room before the fold and the gap across it, 8
    /// points clear each side, where a fold divides it (iOS 27.1).
    private var foldSides: (leading: CGFloat, gutter: CGFloat)? {
        guard let fold = reserved.verticalFold(minGutter: 0) else { return nil }
        let leading = fold.leading - sidePadding - 8
        guard leading > 0 else { return nil }
        return (leading, fold.gutter + 16)
    }

    /// How far the controls move in from each edge to clear the inner
    /// camera (iOS 27.1), past their own padding.
    private var occluded: EdgeInsets {
        let depth = reserved.occludedInsets(within: 64)
        return EdgeInsets(
            top: max(depth.top - 8, 0), leading: max(depth.leading - sidePadding, 0),
            bottom: max(depth.bottom - 8, 0), trailing: max(depth.trailing - sidePadding, 0))
    }

    /// On the result alone, with a frame to show.
    private var canHold: Bool {
        guard case .result = model.viewState else { return false }
        return !model.paint.active && model.stacking == nil && model.result?.display != nil
            && model.sourcePreview?.image != nil
    }

    /// Only the map or a frame alone is named here; the subtitle names the
    /// result.
    private var showsResultLabel: Bool {
        guard !holding, model.stacking == nil, !model.comparing, picture != nil else { return false }
        if model.showsFrameAlone { return model.shownFrameFile != nil }
        if case .result = model.viewState { return false }
        return model.result?.display != nil
    }

    /// "Result · #4", "Depth Map", "Depth Map · Frame 18", "Frame 3 ·
    /// spider-03.jpg": a frame by the pixels on screen, which during a drag
    /// can be one the finger passed.
    private var shownLabel: UserText {
        switch model.viewState {
        case .map: model.depthMapLabel(lit: model.shownLitFile)
        case .result, .compare: .app(model.resultLabel)
        case .frame: model.shownFrameFile.map { model.label(of: .frame(path: $0, overlay: .none)) } ?? .verbatim("")
        }
    }

    /// For VoiceOver, in the label's words: the frame held, the live map,
    /// else `shownLabel` ("Result · #4" for the result alone).
    private var photoLabel: UserText {
        if holding, canHold { return .app(compareLabel) }
        if model.stacking != nil, !model.showsFrameAlone { return .app(ViewerDeck.depthMap) }
        return shownLabel
    }

    /// "Source · Frame 12".
    private var compareLabel: LocalizedStringResource {
        model.frameNumber(of: model.sourcePreview?.file).map {
            LocalizedStringResource(
                "Source · Frame \($0)",
                comment: "Label over the frame shown while the result is touched and held. The placeholder is the frame's number.")
        } ?? LocalizedStringResource(
            "Source", comment: "Label over the frame shown while the result is touched and held, when its number isn't known.")
    }

    /// Each picture with its own tiles where it has them (`PanePicture`).
    private var picture: PanePicture? {
        if let stacking = model.stacking, !model.showsFrameAlone {
            return stacking.liveMap.map(PanePicture.init)
        }
        if holding, canHold {
            return model.sourcePreview?.pane
        }
        return switch model.viewState {
        case .frame:
            model.shownContributionFrame?.pane ?? model.sourcePreview?.pane ?? frameStandIn
        case .result, .compare: model.result?.displayPane ?? model.standIn.map(PanePicture.init)
        case .map: model.shownLitMap?.pane ?? model.result?.mapPane
        }
    }

    /// While the first frame scrubbed to has no picture, the result (or a
    /// stand-in) stays, so the view isn't remade at fit when the frame
    /// lands; no label names a frame over it. Not for an unreadable frame,
    /// whose placeholder says why.
    private var frameStandIn: PanePicture? {
        guard let file = model.selectedFile, model.unavailable[file] == nil,
              !(model.sourcePreview?.file == file && model.sourcePreview?.image == nil)
        else { return nil }
        return model.result?.displayPane ?? model.standIn.map(PanePicture.init)
    }

    @ViewBuilder
    private var placeholder: some View {
        if model.rows.isEmpty, model.result == nil, model.stacking == nil, !phone {
            // The Frames list beside it says so, with the ways to add.
            Color.clear
        } else if model.rows.isEmpty, model.result == nil, model.stacking == nil {
            ContentUnavailableView {
                Label(
                    LocalizedStringResource("No Frames", comment: "Viewer empty-state title: the stack has no frames."),
                    systemImage: "square.3.layers.3d")
            } description: {
                Text(
                    "Take a burst, or add a focus bracket from Photos or Files.",
                    comment: "Viewer empty-state text under No Frames.")
            } actions: {
                if !model.panel.isOpen {
                    Button(
                        LocalizedStringResource(
                            "Add Frames", comment: "Button under the iPhone viewer's No Frames: opens the Frames sheet, which adds them."),
                        systemImage: "plus"
                    ) { model.setPanel(.sources(.frames, .tall)) }
                        .buttonStyle(.glass)
                }
            }
        } else if model.showsFrameAlone, let file = model.selectedFile,
                  let state = model.unavailable[file] {
            switch state {
            case .missing:
                ContentUnavailableView(
                    LocalizedStringResource(
                        "Frame Missing", comment: "Viewer empty-state title: the frame selected isn't where the list says."),
                    systemImage: "exclamationmark.triangle",
                    description: Text(
                        "It isn't where the list says. If its folder moved, find it with Locate… in Frames.",
                        comment: "Viewer empty-state text under Frame Missing. Locate… is a command in the Frames list's menu."))
            case .inCloud:
                ContentUnavailableView(
                    LocalizedStringResource(
                        "In iCloud", comment: "Viewer empty-state title: the frame selected is in iCloud and not downloaded."),
                    systemImage: "icloud.and.arrow.down",
                    description: Text(
                        "This frame isn't downloaded. Stack downloads the frames it needs.",
                        comment: "Viewer empty-state text under In iCloud."))
            case .offline(let place):
                ContentUnavailableView(
                    LocalizedStringResource(
                        "Not Connected", comment: "Viewer empty-state title: the frame selected is on a drive or server that isn't connected."),
                    systemImage: "externaldrive.badge.xmark",
                    description: place.map { Text($0.connectToView) } ?? Text(
                        "Connect the drive or server it is on, or use Locate… if the frames moved.",
                        comment: "Viewer empty-state text under Not Connected, when the drive's or server's name isn't known."))
            }
        } else if model.showsFrameAlone, model.selectedFile == nil, model.result == nil {
            ContentUnavailableView(
                LocalizedStringResource("No Frame Selected", comment: "Viewer empty-state title: no frame is selected in the list."),
                systemImage: "photo",
                description: Text("Tap a frame in the list.", comment: "Viewer empty-state text under No Frame Selected."))
        } else if !model.showsFrameAlone, model.result == nil, model.stacking == nil {
            ContentUnavailableView(
                LocalizedStringResource("No Result Yet", comment: "Viewer empty-state title: the stack has not been stacked yet."),
                systemImage: RenderInfo.symbol,
                description: Text(
                    "Choose the frames and settings, then tap Stack.", comment: "Viewer empty-state text under No Result Yet."))
        } else {
            ProgressView()
        }
    }
}

extension View {
    /// Chrome over the photo: the `hud` fill, edge and text (`design/tokens.json`).
    func hudPill() -> some View {
        foregroundStyle(Color.hudText)
            .background(Color.hud, in: Capsule())
            .overlay(Capsule().strokeBorder(Color.hudBorder, lineWidth: 1))
    }

    /// For a panel of text and buttons over the photo.
    func hudPanel() -> some View {
        foregroundStyle(Color.hudText)
            .background(Color.hud, in: RoundedRectangle(cornerRadius: 16))
            .overlay(RoundedRectangle(cornerRadius: 16).strokeBorder(Color.hudBorder, lineWidth: 1))
    }
}

extension View {
    /// One image for VoiceOver, named as its label ("Frame 18", "Result ·
    /// #4"); the controls stay their own elements.
    func accessibilityPhoto(_ label: UserText) -> some View {
        accessibilityElement(children: .ignore)
            .accessibilityAddTraits(.isImage)
            .accessibilityLabel(Text(label))
    }
}

extension UserText {
    /// For VoiceOver: the leading side, then the trailing.
    static func split(_ leading: UserText, _ trailing: UserText) -> UserText {
        .app(LocalizedStringResource(
            "\(leading.string) and \(trailing.string)",
            comment: "VoiceOver label of a photo split by a divider. The placeholders are the pictures either side of it, leading first, as their labels name them ('Frame 18', 'Result · #4')."))
    }
}

/// The label over the depth map: "Depth Map ✕", or "Depth Map · Frame 18
/// ✕". A tap leaves the map. With `wholeMap`, VoiceOver can put the light
/// out from it, since it can't aim a tap on the map.
struct DepthMapLabel: View {
    let text: UserText
    let help: LocalizedStringResource
    let close: () -> Void
    var wholeMap: (() -> Void)?

    var body: some View {
        Button(action: close) {
            HStack(spacing: 6) {
                Text(text)
                Image(systemName: "xmark").font(.caption2.weight(.bold)).foregroundStyle(.secondary)
            }
            .font(.caption.weight(.semibold))
            .lineLimit(1)
            .padding(.horizontal, 10)
            .padding(.vertical, 6)
            .glassEffect(.regular.interactive(), in: .capsule)
            // A finger's room around the pill, taking none in the layout.
            .padding(10)
            .contentShape(Rectangle())
            .padding(-10)
        }
        .buttonStyle(.plain)
        .accessibilityLabel(LocalizedStringResource(
            "Close Depth Map", comment: "VoiceOver label of the Depth Map label at the top of the photo: a tap goes back from the map."))
        .accessibilityValue(Text(text))
        .accessibilityActions {
            if let wholeMap {
                Button(LocalizedStringResource(
                    "Show Whole Map",
                    comment: "VoiceOver action on the Depth Map label while a frame is lit on the map: shows the map whole again, no frame lit.")
                ) { wholeMap() }
            }
        }
        .help(help)
    }
}

struct PhotoLabel: View {
    let text: UserText

    var body: some View {
        Text(text)
            .font(.caption.weight(.semibold))
            .lineLimit(1)
            .padding(.horizontal, 8)
            .padding(.vertical, 4)
            .hudPill()
    }
}

/// The viewer's bottom trailing corner: progress/Cancel while running;
/// prominent Stack when unstacked or edited; prominent Retouch when viewing
/// a clean, up-to-date render.
struct StackCorner: View {
    @Environment(AppModel.self) private var model
    /// Not where the system's side bar has it.
    var showsButton = true

    var body: some View {
        if let stacking = model.stacking {
            StackProgressRow(stacking: stacking, spacing: 12)
                .buttonStyle(.bordered)
                .controlSize(.large)
                .padding(.horizontal, 14)
                .padding(.vertical, 10)
                .frame(maxWidth: 360)
                .glassEffect(.regular, in: RoundedRectangle(cornerRadius: 16))
        } else if let run = model.batchRun {
            // Paused in the background, waiting to go on.
            PausedBatchRow(run: run, spacing: 12)
                .buttonStyle(.bordered)
                .controlSize(.large)
                .padding(.horizontal, 14)
                .padding(.vertical, 10)
                .glassEffect(.regular, in: RoundedRectangle(cornerRadius: 16))
        } else if showsButton {
            if model.result != nil, model.showing.changeLine == nil, model.canPaint {
                Button {
                    model.startPainting()
                } label: {
                    Label(
                        LocalizedStringResource("Retouch", comment: "Prominent deck button when viewing a clean render: opens Retouch."),
                        systemImage: "paintbrush.pointed")
                }
                .controlSize(.large)
                .lineLimit(1)
                .buttonStyle(.glassProminent)
                .tint(.accentFill)
                .accessibilityIdentifier("retouch")
                .help(LocalizedStringResource(
                    "Paint over the result from a frame or another render",
                    comment: "Pointer tooltip of the Retouch button."))
            } else {
                StackButton()
                    .controlSize(.large)
            }
        }
    }
}

/// The result laid out with its compare: split, side by side, above and
/// below, or flicked. Panes zoom and pan together. Labels sit above the
/// bottom row and the divider spans between the bars (`chrome`); a low
/// sheet covers them, moving neither.
private struct CompareView: View {
    @Environment(AppModel.self) private var model
    let result: PanePicture
    /// What the controls cover at the bottom past the safe area
    /// (`ZoomToFit.swift`).
    let below: Below
    /// True in photo-only mode so fit expands edge to edge.
    let controlsHidden: Bool
    /// The bars, and the bottom row over them.
    let chrome: UIEdgeInsets
    /// The safe area's insets on the edges the photo runs under.
    let labelInsets: EdgeInsets
    /// A tap hides them with the other controls.
    let showsLabels: Bool
    /// A frame's label is its number alone, so two fit by the divider.
    let compact: Bool
    @State private var sync = ZoomSync()
    /// The fold across the panes (iOS 27.1): Side by Side and Top and
    /// Bottom divide there.
    @State private var reserved = ReservedAreas()

    var body: some View {
        layout
            .measure("compare reserved areas", for: ReservedAreas.self) { ReservedAreas.read($0) } action: { reserved = $0 }
    }

    @ViewBuilder
    private var layout: some View {
        let compare = model.shownCompare
        let compareLabel = compareText(compare)
        let state = model.compareState
        let swapped = state?.swapped == true && compare != nil
        switch state?.layout ?? .split {
        case .split:
            GeometryReader { geometry in
                ZStack(alignment: .bottomLeading) {
                    // Swapped, the result is left of the divider.
                    ZoomableImage(
                        picture: swapped ? compare!.pane : result,
                        overlay: swapped ? result : compare?.pane,
                        split: model.splitFraction,
                        fitRequest: model.fitRequest, below: below,
                        controlsHidden: controlsHidden,
                        remembered: model.viewerZoom, onZoom: { model.viewerZoom = $0 })
                        .accessibilityPhoto(
                            swapped
                                ? .split(.app(model.resultLabel), compareLabel)
                                : .split(compareLabel, .app(model.resultLabel)))
                    if compare != nil {
                        divider(in: geometry.size)
                    }
                }
                .overlay(alignment: .bottomLeading) {
                    if showsLabels {
                        HStack {
                            PhotoLabel(text: swapped ? .app(model.resultLabel) : compareLabel)
                            Spacer(minLength: 24)
                            PhotoLabel(text: swapped ? compareLabel : .app(model.resultLabel))
                        }
                        .padding(clear(leading: true, bottom: true, trailing: true))
                        .allowsHitTesting(false)
                    }
                }
                .coordinateSpace(.named("split"))
            }
        case .sideBySide:
            // Divided at the fold, where one runs from top to bottom.
            let fold = reserved.verticalFold()
            HStack(spacing: fold?.gutter ?? 2) {
                pane(
                    swapped ? result : compare?.pane, label: swapped ? .app(model.resultLabel) : compareLabel,
                    below: below, insets: clear(leading: true, bottom: true))
                    .frame(width: fold?.leading)
                pane(
                    swapped ? compare?.pane : result, label: swapped ? compareLabel : .app(model.resultLabel),
                    below: below, insets: clear(bottom: true, trailing: true))
            }
        case .topAndBottom:
            // Divided at the fold, where one runs from side to side.
            let fold = reserved.horizontalFold()
            VStack(spacing: fold?.gutter ?? 2) {
                pane(
                    swapped ? result : compare?.pane, label: swapped ? .app(model.resultLabel) : compareLabel,
                    // The bottom row is over the other pane.
                    below: .row(0), insets: clear(leading: true))
                    .frame(height: fold?.top)
                pane(
                    swapped ? compare?.pane : result, label: swapped ? compareLabel : .app(model.resultLabel),
                    below: below, insets: clear(leading: true, bottom: true))
            }
        case .flicker:
            let flipped = state?.layout.showsSource == true && compare != nil
            ZoomableImage(
                picture: flipped ? compare!.pane : result, keepsFrame: true,
                fitRequest: model.fitRequest, below: below,
                controlsHidden: controlsHidden,
                remembered: model.viewerZoom, onZoom: { model.viewerZoom = $0 })
                .accessibilityPhoto(flipped ? compareLabel : .app(model.resultLabel))
                .overlay(alignment: .bottomLeading) {
                    if showsLabels {
                        PhotoLabel(text: flipped ? compareLabel : .app(model.resultLabel))
                            .padding(clear(leading: true, bottom: true))
                            .allowsHitTesting(false)
                    }
                }
        }
    }

    /// 12 points in, plus the bars' depth on the edges named (the bottom
    /// row's too at the bottom).
    private func clear(leading: Bool = false, bottom: Bool = false, trailing: Bool = false) -> EdgeInsets {
        EdgeInsets(
            top: 12, leading: 12 + (leading ? labelInsets.leading : 0),
            bottom: bottom ? max(chrome.bottom, labelInsets.bottom) + 8 : 12,
            trailing: 12 + (trailing ? labelInsets.trailing : 0))
    }

    /// "Frame 23 · spider-24.jpg" ("Frame 23" compact), noting when it
    /// couldn't be lined up: what is on screen, possibly a frame passed
    /// during a drag.
    private func compareText(_ compare: CompareImage?) -> UserText {
        guard let target = compare?.source.labelTarget ?? model.compareWith else { return .verbatim("") }
        let text: UserText
        if compact, let file = target.framePath, let number = model.frameNumber(of: file) {
            text = .app(frameLabel(number))
        } else {
            text = model.label(of: target)
        }
        return compare?.aligned == false
            ? .line([text, .app(LocalizedStringResource(
                "not aligned",
                comment: "After the label of a frame compared with the result: it couldn't be lined up with the result."))])
            : text
    }

    private func pane(_ picture: PanePicture?, label: UserText, below: Below, insets: EdgeInsets) -> some View {
        Group {
            if let picture {
                ZoomableImage(
                    picture: picture, fitRequest: model.fitRequest, sync: sync, below: below,
                    controlsHidden: controlsHidden,
                    remembered: model.viewerZoom, onZoom: { model.viewerZoom = $0 })
                    .accessibilityPhoto(label)
            } else {
                ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        // An overlay, so the label never sizes the pane.
        .overlay(alignment: .bottomLeading) {
            if showsLabels {
                PhotoLabel(text: label)
                    .padding(insets)
                    .allowsHitTesting(false)
            }
        }
        .clipped()
    }

    /// Spans the photo between the bars and bottom row, its handle in the
    /// middle.
    private func divider(in size: CGSize) -> some View {
        // 8 points clear of the title above and the bottom row below.
        let top = chrome.top + 8
        let height = max(size.height - top - chrome.bottom - 8, 44)
        return dividerLine(height: height)
            .position(x: size.width * model.splitFraction, y: top + height / 2)
            .gesture(
                DragGesture(minimumDistance: 0, coordinateSpace: .named("split"))
                    .onChanged { value in
                        model.splitFraction = min(max(value.location.x / max(size.width, 1), 0.02), 0.98)
                    })
            .accessibilityLabel(LocalizedStringResource(
                "Divider", comment: "VoiceOver label of the divider between the result and what it is compared with, in Split."))
            // How far across the picture it is, "40%" in the user's locale.
            .accessibilityValue(Text(verbatim: Double(model.splitFraction).formatted(.percent.precision(.fractionLength(0)))))
            .accessibilityAdjustableAction { direction in
                model.splitFraction = min(max(model.splitFraction + (direction == .increment ? 0.05 : -0.05), 0.02), 0.98)
            }
    }

    private func dividerLine(height: CGFloat) -> some View {
        SplitDividerLine(height: height)
    }
}

/// White with a dark edge, so it shows on a light photo too, with a
/// handle in its middle.
struct SplitDividerLine: View {
    let height: CGFloat

    var body: some View {
        ZStack {
            Rectangle().fill(.black.opacity(0.45)).frame(width: 4)
            Rectangle().fill(.white).frame(width: 2)
            Circle()
                .fill(.white)
                .frame(width: 28, height: 28)
                .overlay(Circle().strokeBorder(.black.opacity(0.45), lineWidth: 1))
                .overlay(
                    Image(systemName: "arrow.left.and.right")
                        .font(.caption.weight(.bold))
                        .foregroundStyle(.black))
                .shadow(radius: 3)
        }
        .frame(width: 44, height: height)
        .contentShape(Rectangle())
    }
}

struct ViewerTapAction {
    let perform: () -> Void
    func callAsFunction() { perform() }
}

extension EnvironmentValues {
    /// A single tap on the photo: hide or show its controls.
    @Entry var viewerTap: ViewerTapAction? = nil
}
