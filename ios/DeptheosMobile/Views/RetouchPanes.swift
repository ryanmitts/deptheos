import SwiftUI
import UIKit
@preconcurrency import DeptheosEngine

/// Retouch's photo, laid out as `PaintView` says: the canvas alone, Split,
/// the source in a pane beside or above (divided at a fold where one
/// crosses), peeking, or the depth map over the canvas. The source's pane
/// is read-only; the two zoom together through a `ZoomSync`. The canvas
/// keeps its place through every layout, and its zoom through a fold
/// (`PaintModel.zoom`). ADR-163.
struct RetouchPhoto: View {
    @Environment(AppModel.self) private var model
    /// What the controls cover at the bottom past the safe area, which a
    /// zoomed canvas or pane drags clear of (`ZoomToFit.swift`).
    let below: Below
    /// Extra chrome insets past safeArea (e.g., landscape side rails).
    var extraChromeInsets: UIEdgeInsets = .zero
    /// Shows the live brush ring centered in the clear rect while scrubbing
    /// Size or Hardness in the Retouch tray.
    var previewingBrush = false
    /// The bars and the deck over them. Labels and Split's divider keep
    /// inside it; a low sheet covers them, moving neither.
    let chrome: UIEdgeInsets
    /// The safe area's insets on the edges the photo runs under.
    let labelInsets: EdgeInsets
    /// Not while a scrub's loupe says the same.
    var showsLabels = true
    @State private var sync = ZoomSync()
    /// The fold where it crosses the photo (iOS 27.1).
    @State private var reserved = ReservedAreas()

    private var paint: PaintModel { model.paint }

    var body: some View {
        let view = paint.view
        let layout: PaintLayout = if case .layout(let layout) = view { layout } else { .canvas }
        GeometryReader { geometry in
            ZStack(alignment: .bottomLeading) {
                PaneArrangement(arrangement: arrangement(layout)) {
                    sourcePane(layout)
                    PaintCanvas(
                        paint: paint, fitRequest: model.fitRequest, below: below,
                        extraChromeInsets: extraChromeInsets, previewingBrush: previewingBrush,
                        cover: cover(view), sync: sync)
                        .accessibilityPhoto(canvasLabel(view))
                        // The canvas's label, at its own foot; an overlay,
                        // so it never sizes the pane.
                        .overlay(alignment: .bottomLeading) {
                            if showsLabels, layout == .sideBySide || layout == .topBottom {
                                PhotoLabel(text: .app(Self.paintedLabel))
                                    .padding(layout == .sideBySide ? clear(bottom: true) : clear(leading: true, bottom: true))
                                    .allowsHitTesting(false)
                                    .transition(.opacity)
                            }
                        }
                        .clipped()
                    // A failed open: why, with Retry and Done, on the canvas
                    // rather than in an alert.
                    .overlay {
                        if let reason = paint.canvasState?.failure {
                            OpenFailedCard(reason: reason, retry: { paint.retryOpen() }, done: { model.stopPainting() })
                                .padding(clear(leading: true, bottom: true, trailing: true))
                        }
                    }
                }
                if case .split(let divider) = layout {
                    dividerView(at: divider, in: geometry.size)
                    if showsLabels {
                        HStack {
                            PhotoLabel(text: .app(sourceLabel))
                            Spacer(minLength: 24)
                            PhotoLabel(text: .app(Self.paintedLabel))
                        }
                        .padding(clear(leading: true, bottom: true, trailing: true))
                        .allowsHitTesting(false)
                        .transition(.opacity)
                    }
                }
            }
            .coordinateSpace(.named("retouch"))
        }
        .measure("retouch reserved areas", for: ReservedAreas.self) { ReservedAreas.read($0) } action: { reserved = $0 }
    }

    /// Divided at a fold that runs the same way.
    private func arrangement(_ layout: PaintLayout) -> PaneArrangement.Arrangement {
        switch layout {
        case .canvas, .split: .single
        case .sideBySide:
            reserved.verticalFold().map { .sideBySide(leading: $0.leading, gutter: $0.gutter) }
                ?? .sideBySide(leading: nil, gutter: 2)
        case .topBottom:
            reserved.horizontalFold().map { .topBottom(top: $0.top, gutter: $0.gutter) }
                ?? .topBottom(top: nil, gutter: 2)
        }
    }

    /// Read-only: Side by Side's leading pane, Top and Bottom's top one.
    @ViewBuilder
    private func sourcePane(_ layout: PaintLayout) -> some View {
        switch layout {
        case .sideBySide, .topBottom:
            Group {
                if let picture = paint.sourcePane {
                    ZoomableImage(
                        picture: picture, sync: sync,
                        // Above, the deck is over the other pane.
                        below: layout == .sideBySide ? below : .row(0),
                        extraChromeInsets: extraChromeInsets)
                        .accessibilityPhoto(.app(sourceLabel))
                        // A tap here doesn't hide the controls.
                        .environment(\.viewerTap, nil)
                } else {
                    ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
                }
            }
            .overlay(alignment: .bottomLeading) {
                if showsLabels {
                    PhotoLabel(text: .app(sourceLabel))
                        .padding(layout == .sideBySide ? clear(leading: true, bottom: true) : clear(leading: true))
                        .allowsHitTesting(false)
                        .transition(.opacity)
                }
            }
            .clipped()
        case .canvas, .split:
            Color.clear
        }
    }

    /// The source while peeking or left of Split's divider; the depth map,
    /// lit for the frame painted from once its picture lands.
    private func cover(_ view: PaintView) -> CanvasCover {
        switch view {
        case .peeking: .source(paint.sourcePane, split: nil)
        case .layout(.split(let divider)): .source(paint.sourcePane, split: divider)
        case .depthMap: .map(paint.shownLitMap?.pane ?? model.result?.mapPane)
        case .layout: .none
        }
    }

    /// 12 points in, plus the bars' depth on the edges named (the deck's
    /// too at the bottom).
    private func clear(leading: Bool = false, bottom: Bool = false, trailing: Bool = false) -> EdgeInsets {
        EdgeInsets(
            top: 12, leading: 12 + (leading ? labelInsets.leading : 0),
            bottom: bottom ? max(chrome.bottom, labelInsets.bottom) + 8 : 12,
            trailing: 12 + (trailing ? labelInsets.trailing : 0))
    }

    /// For VoiceOver, in its labels' words: the source while peeking, the
    /// depth map, Split's two sides, "Painted" beside the source's pane,
    /// alone the title's "Retouching #4".
    private func canvasLabel(_ view: PaintView) -> UserText {
        switch view {
        case .peeking: .app(sourceLabel)
        case .depthMap: model.depthMapLabel(lit: paint.shownLitMap?.file)
        case .layout(.split): .split(.app(sourceLabel), .app(Self.paintedLabel))
        case .layout(.sideBySide), .layout(.topBottom): .app(Self.paintedLabel)
        case .layout(.canvas): .app(model.retouchTitle)
        }
    }

    /// "Source · Frame 23".
    private var sourceLabel: LocalizedStringResource {
        let source = paint.label(of: paint.selection, renders: model.history.renders).string
        return LocalizedStringResource(
            "Source · \(source)",
            comment: "Label over the picture in paint mode while the source is shown. The placeholder is the source, such as 'Frame 23', '#3' or 'Original'.")
    }

    static let paintedLabel = LocalizedStringResource(
        "Painted", comment: "Retouch: the label over the painted result beside or under its source, and the layout menu's item for the painted result alone.")

    /// Spans the photo between the bars and the deck.
    private func dividerView(at fraction: CGFloat, in size: CGSize) -> some View {
        let top = chrome.top + 8
        let height = max(size.height - top - chrome.bottom - 8, 44)
        return SplitDividerLine(height: height)
            .position(x: size.width * fraction, y: top + height / 2)
            .gesture(
                DragGesture(minimumDistance: 0, coordinateSpace: .named("retouch"))
                    .onChanged { value in paint.setDivider(value.location.x / max(size.width, 1)) })
            .accessibilityLabel(LocalizedStringResource(
                "Divider", comment: "VoiceOver label of the divider between the result and what it is compared with, in Split."))
            .accessibilityValue(Text(verbatim: Double(fraction).formatted(.percent.precision(.fractionLength(0)))))
            .accessibilityAdjustableAction { direction in
                paint.setDivider(fraction + (direction == .increment ? 0.05 : -0.05))
            }
    }
}

/// Retouch's canvas couldn't open: why (the engine's sentences), Retry, and
/// Done, which leaves Retouch.
struct OpenFailedCard: View {
    let reason: UserText
    let retry: () -> Void
    let done: () -> Void

    var body: some View {
        VStack(spacing: 14) {
            Text(reason)
                .font(.callout)
                .multilineTextAlignment(.center)
                .fixedSize(horizontal: false, vertical: true)
            HStack(spacing: 12) {
                Button(action: done) {
                    Text(LocalizedStringResource(
                        "Done", comment: "Retouch: the button on the card over a canvas that couldn't be opened; leaves paint mode."))
                        .frame(minWidth: 72)
                }
                .buttonStyle(.glass)
                Button(action: retry) {
                    Text(LocalizedStringResource(
                        "Retry", comment: "Retouch: the button on the card over a canvas that couldn't be opened; opens the render for retouching again."))
                        .frame(minWidth: 72)
                }
                .buttonStyle(.glassProminent)
            }
        }
        .padding(20)
        .frame(maxWidth: 360)
        .glassEffect(.regular, in: RoundedRectangle(cornerRadius: 20))
        .accessibilityElement(children: .contain)
    }
}

/// Two panes, source and canvas: the canvas alone, side by side or
/// stacked, the first `leading` wide (or `top` tall) at a fold, else half,
/// with `gutter` between. One layout for all, so the canvas keeps its
/// identity and tiles.
struct PaneArrangement: Layout {
    enum Arrangement: Equatable {
        case single
        case sideBySide(leading: CGFloat?, gutter: CGFloat)
        case topBottom(top: CGFloat?, gutter: CGFloat)
    }

    let arrangement: Arrangement

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        proposal.replacingUnspecifiedDimensions()
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        guard subviews.count == 2 else { return }
        let (source, canvas) = (subviews[0], subviews[1])
        func place(_ view: LayoutSubview, _ rect: CGRect) {
            view.place(at: rect.origin, anchor: .topLeading, proposal: ProposedViewSize(rect.size))
        }
        switch arrangement {
        case .single:
            place(source, CGRect(origin: bounds.origin, size: .zero))
            place(canvas, bounds)
        case .sideBySide(let leading, let gutter):
            let first = min(leading ?? (bounds.width - gutter) / 2, bounds.width - gutter)
            place(source, CGRect(x: bounds.minX, y: bounds.minY, width: first, height: bounds.height))
            place(canvas, CGRect(
                x: bounds.minX + first + gutter, y: bounds.minY,
                width: bounds.width - first - gutter, height: bounds.height))
        case .topBottom(let top, let gutter):
            let first = min(top ?? (bounds.height - gutter) / 2, bounds.height - gutter)
            place(source, CGRect(x: bounds.minX, y: bounds.minY, width: bounds.width, height: first))
            place(canvas, CGRect(
                x: bounds.minX, y: bounds.minY + first + gutter,
                width: bounds.width, height: bounds.height - first - gutter))
        }
    }
}
