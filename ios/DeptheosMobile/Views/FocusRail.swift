import SwiftUI
import UIKit
@preconcurrency import DeptheosEngine

private let stateLog = AppLog(category: "state")

/// One frame on the rail: its share of the result (0–1; `nil` without a
/// frame map), whether it was left out and, while the depth map shows, its
/// color there (`nil` otherwise and when left out).
struct RailFrame: Equatable {
    let path: String
    let share: Float?
    let leftOut: Bool
    var legend: Color? = nil

    var name: String { (path as NSString).lastPathComponent }
}

/// The focus rail: a tick per frame in stack order, as tall as its share,
/// the selection marked, frames left out in a lane underneath and `marks`
/// as dots (Retouch's painted-from); over the depth map, the map's legend.
/// No frame is hidden or fainter than its neighbors: a short tick can be
/// the only sharp frame for a small feature (ADR-121). Drawn by
/// `RailDrawing`; a touch is a `RailTouch`. ADR-149.
///
/// A drag sets `selection` for each frame crossed; with `homeBeforeFirst`
/// past the first sets `nil`, and with `reselectClears` so does a tap on
/// the selection. One adjustable element for VoiceOver. What a selection
/// does is the binding setter's.
struct FocusRail: View {
    let frames: [RailFrame]
    @Binding var selection: Int?
    var homeBeforeFirst = false
    var reselectClears = false
    /// The ticks with a dot over them.
    var marks: IndexSet = []
    var markTint = Color.edited

    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.automationLoupe) private var automationLoupe
    @Environment(\.displayScale) private var displayScale
    /// The rail's own state, not the view's. Changed only by `setTouch`.
    @State private var touch = RailTouch.idle
    /// Bumped per frame crossed, at most once each `hapticGap`, for the
    /// selection haptic.
    @State private var crossings = 0
    /// Bumped as the rail magnifies, for its haptic.
    @State private var zooms = 0
    /// A scroll not yet a whole step: a memory, not a mode.
    @State private var wheel: CGFloat = 0

    /// A drag this far before the first tick goes home.
    private static let homeSlack: CGFloat = 12
    /// The ticks' room, the lane under them included.
    static let drawnHeight: CGFloat = 32
    /// Selection haptics, at most one this often.
    private static let hapticGap: TimeInterval = 0.03
    /// On lift, the frame under the finger this long before is kept if the
    /// finger rested on it.
    private static let restWindow: TimeInterval = 0.09
    /// Held this long, moving less than `holdSlop`, the rail magnifies.
    private static let holdDelay = Duration.milliseconds(350)
    private static let holdSlop: CGFloat = 3
    /// Magnified, a frame's room: a finger's move this far is a frame.
    static let zoomSlot: CGFloat = 10
    /// Magnified, a finger this near either end pans a frame each
    /// `panPeriod`.
    private static let panEdge: CGFloat = 20
    private static let panPeriod = Duration.milliseconds(90)
    /// A trackpad's or a wheel's scroll this far steps one frame.
    private static let scrollStep: CGFloat = 24

    var body: some View {
        HStack(spacing: 8) {
            ticks
            Text(readout)
                .font(.caption.weight(.semibold))
                .monospacedDigit()
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .fixedSize()
                .accessibilityHidden(true)
        }
        .padding(.horizontal, 12)
        .frame(minHeight: 44)
        .contentShape(.capsule)
        .accessibilityElement()
        .accessibilityLabel(Self.railLabel)
        .accessibilityValue(spokenValue)
        .accessibilityIdentifier("focus-rail")
        .accessibilityAdjustableAction { direction in
            step(direction == .increment ? 1 : -1)
        }
        .accessibilityAction(named: Text(Self.forwardTen)) { step(10) }
        .accessibilityAction(named: Text(Self.backTen)) { step(-10) }
        .accessibilityAction(named: Text(Self.firstFrame)) { jump(to: 0) }
        .accessibilityAction(named: Text(Self.lastFrame)) { jump(to: frames.count - 1) }
        .sensoryFeedback(.selection, trigger: crossings)
        .sensoryFeedback(.impact(weight: .light), trigger: zooms)
        // Labels over the photo under the loupe give way while it shows.
        .preference(key: RailScrubbing.self, value: loupeFrame != nil)
    }

    private var ticks: some View {
        GeometryReader { geometry in
            let width = geometry.size.width
            let lens = lens(width: width)
            let origin = geometry.frame(in: .global).origin
            RailDrawing(
                frames: frames, selection: selection, marks: marks, markTint: markTint,
                scale: displayScale,
                phase: CGPoint(x: Self.phase(origin.x * displayScale), y: Self.phase(origin.y * displayScale)),
                offset: lens.offset, slot: lens.slot)
                .contentShape(.rect)
                .gesture(
                    DragGesture(minimumDistance: 0)
                        .onChanged { drag(to: $0.location.x, width: width) }
                        .onEnded { lift(at: $0.location.x, width: width) })
                .onContinuousHover { hover($0, width: width) }
                .gesture(RailScroll { scroll($0) })
                .overlay(alignment: .topLeading) {
                    if let shown = loupeFrame, frames.indices.contains(shown) {
                        loupe(shown)
                            .fixedSize()
                            .position(
                                x: min(max(lens.center(shown), 70), max(width - 70, 70)), y: -22)
                            .allowsHitTesting(false)
                            .transition(.opacity)
                    }
                }
                .task(id: touch.restKey) { await holdToZoom() }
                .task(id: touch.panDirection(edge: Self.panEdge, slop: Self.holdSlop)) { await panWhileHeld() }
                .automation(Automation.flag("RailZoom"), after: .seconds(1)) { automationZoom(width: width) }
        }
        .frame(height: Self.drawnHeight)
    }

    /// `x`'s fraction of a device pixel: ticks are drawn on screen pixels.
    private static func phase(_ x: CGFloat) -> CGFloat { x - x.rounded(.down) }

    // MARK: The touch

    /// The width shared out, or magnified about the frame held. The width
    /// is the touch's, so the drawing doesn't change mode under a finger.
    private func lens(width: CGFloat) -> RailLens {
        switch touch {
        case .zoomed(let zoom): RailLens(offset: zoom.offset, slot: Self.zoomSlot)
        case .scrubbing(let scrub): RailLens(offset: 0, slot: scrub.width / CGFloat(max(frames.count, 1)))
        case .idle, .hovering: RailLens(offset: 0, slot: width / CGFloat(max(frames.count, 1)))
        }
    }

    /// Under the finger or pointer, or for automation the selection.
    private var loupeFrame: Int? {
        switch touch {
        case .hovering(let frame): frame
        case .scrubbing(let scrub): scrub.frame
        case .zoomed(let zoom): zoom.frame
        case .idle: automationLoupe ? selection : nil
        }
    }

    /// The one place `touch` changes; each change of kind is logged.
    private func setTouch(_ next: RailTouch) {
        if next.logName != touch.logName {
            stateLog.debug("focus rail: \(touch.logName, privacy: .public) → \(next.logName, privacy: .public)")
        }
        touch = next
    }

    /// It lands, scrubs, or moves over the magnified rail.
    private func drag(to x: CGFloat, width: CGFloat) {
        let now = Date.now
        switch touch {
        case .idle, .hovering:
            guard !frames.isEmpty, width > 0 else { return }
            let frame = overviewFrame(at: x, width: width)
            var scrub = RailTouch.Scrub(
                width: width, x: x, frame: frame, landedOnSelected: frame != nil && frame == selection,
                moved: false, rest: .init(x: x, since: now), trail: .init(frame, at: now),
                lastHaptic: .distantPast)
            select(frame, haptic: &scrub.lastHaptic, at: now)
            setTouch(.scrubbing(scrub))
        case .scrubbing(var scrub):
            scrub.x = x
            if abs(x - scrub.rest.x) >= Self.holdSlop { scrub.rest = .init(x: x, since: now) }
            let frame = overviewFrame(at: x, width: scrub.width)
            if frame != scrub.frame {
                scrub.moved = true
                scrub.frame = frame
                scrub.trail.enter(frame, at: now)
                select(frame, haptic: &scrub.lastHaptic, at: now)
            }
            setTouch(.scrubbing(scrub))
        case .zoomed(var zoom):
            zoom.x = x
            let frame = RailLens(offset: zoom.offset, slot: Self.zoomSlot).frame(at: x, count: frames.count)
            if frame != zoom.frame {
                zoom.frame = frame
                zoom.trail.enter(frame, at: now)
                select(frame, haptic: &zoom.lastHaptic, at: now)
            }
            setTouch(.zoomed(zoom))
        }
    }

    /// The frame rested on is kept, a tap on the selection clears it where
    /// asked, and a magnified rail goes back.
    private func lift(at x: CGFloat, width: CGFloat) {
        drag(to: x, width: width)
        let now = Date.now
        switch touch {
        case .scrubbing(let scrub):
            if reselectClears, scrub.landedOnSelected, !scrub.moved {
                selection = nil
            } else {
                keep(scrub.trail.rested(at: now, window: Self.restWindow))
            }
        case .zoomed(let zoom):
            keep(zoom.trail.rested(at: now, window: Self.restWindow))
        case .idle, .hovering:
            break
        }
        let animation: Animation? = reduceMotion ? nil : .snappy(duration: 0.25)
        withAnimation(animation) { setTouch(.idle) }
    }

    /// `nil` is home.
    private func keep(_ frame: Int?) {
        if let frame {
            if selection != frame { selection = frame }
        } else if homeBeforeFirst, selection != nil {
            selection = nil
        }
    }

    /// Or home (`nil`) before the first.
    private func overviewFrame(at x: CGFloat, width: CGFloat) -> Int? {
        if homeBeforeFirst, x < -Self.homeSlack { return nil }
        return RailLens(offset: 0, slot: width / CGFloat(max(frames.count, 1))).frame(at: x, count: frames.count)
    }

    /// Selected, with a haptic unless one was given within `hapticGap`.
    private func select(_ frame: Int?, haptic last: inout Date, at now: Date) {
        guard let frame else {
            if selection != nil { selection = nil }
            return
        }
        if selection != frame { selection = frame }
        if now.timeIntervalSince(last) >= Self.hapticGap {
            last = now
            crossings += 1
        }
    }

    /// Held still: the rail magnifies about the frame under the finger,
    /// with a light haptic, unless a frame already has a magnified room.
    private func holdToZoom() async {
        guard case .scrubbing(let held) = touch else { return }
        try? await Task.sleep(for: Self.holdDelay)
        guard !Task.isCancelled, case .scrubbing(let scrub) = touch, scrub.rest == held.rest,
              let frame = scrub.frame, frames.count > 1,
              scrub.width / CGFloat(frames.count) < Self.zoomSlot
        else { return }
        zoom(about: frame, at: scrub.x, width: scrub.width, trail: scrub.trail, lastHaptic: scrub.lastHaptic)
    }

    private func zoom(about frame: Int, at x: CGFloat, width: CGFloat, trail: RailTouch.Trail, lastHaptic: Date) {
        let zoom = RailTouch.Zoom(
            width: width, offset: x - Self.zoomSlot * (CGFloat(frame) + 0.5), anchorX: x, x: x, frame: frame,
            trail: trail, lastHaptic: lastHaptic)
        zooms += 1
        let animation: Animation? = reduceMotion ? nil : .snappy(duration: 0.25)
        withAnimation(animation) { setTouch(.zoomed(zoom)) }
    }

    /// Magnified, a finger near an end pans a frame at a time, the frame
    /// under it following, until the first or last.
    private func panWhileHeld() async {
        guard case .zoomed(let zoom) = touch else { return }
        let direction = zoom.panDirection(edge: Self.panEdge, slop: Self.holdSlop)
        guard direction != 0 else { return }
        while !Task.isCancelled {
            try? await Task.sleep(for: Self.panPeriod)
            guard !Task.isCancelled, case .zoomed(var zoom) = touch else { return }
            let next = min(max(zoom.frame + direction, 0), frames.count - 1)
            guard next != zoom.frame else { return }
            let now = Date.now
            zoom.offset -= CGFloat(direction) * Self.zoomSlot
            zoom.frame = next
            zoom.trail.enter(next, at: now)
            select(next, haptic: &zoom.lastHaptic, at: now)
            let animation: Animation? = reduceMotion ? nil : .linear(duration: 0.09)
            withAnimation(animation) { setTouch(.zoomed(zoom)) }
        }
    }

    /// A pointer over the ticks shows the loupe for the frame under it.
    private func hover(_ phase: HoverPhase, width: CGFloat) {
        switch (phase, touch) {
        case (.active(let point), .idle), (.active(let point), .hovering):
            guard !frames.isEmpty, width > 0 else { return }
            let frame = overviewFrame(at: max(point.x, 0), width: width) ?? 0
            if case .hovering(frame) = touch { return }
            setTouch(.hovering(frame: frame))
        case (.ended, .hovering):
            setTouch(.idle)
        default:
            break
        }
    }

    /// `delta` points on the larger axis: a frame each `scrollStep`, up or
    /// left going on.
    private func scroll(_ delta: CGFloat) {
        wheel += delta
        let whole = (wheel / Self.scrollStep).rounded(.towardZero)
        wheel -= whole * Self.scrollStep
        if whole != 0 { step(-Int(whole)) }
    }

    /// VoiceOver, its actions and scrolls: `by` frames, from before the
    /// first or after the last when none is selected.
    private func step(_ by: Int) {
        guard !frames.isEmpty else { return }
        let next = selection.map { $0 + by } ?? (by > 0 ? by - 1 : frames.count + by)
        selection = min(max(next, 0), frames.count - 1)
    }

    private func jump(to frame: Int) {
        guard frames.indices.contains(frame) else { return }
        selection = frame
    }

    /// Automation (`-RailZoom YES`): magnified about the selection, as a
    /// held finger leaves it, for a screenshot.
    private func automationZoom(width: CGFloat) {
        guard let frame = selection ?? (frames.isEmpty ? nil : frames.count / 2), width > 0 else { return }
        let x = (CGFloat(frame) + 0.5) * width / CGFloat(frames.count)
        zoom(about: frame, at: x, width: width, trail: .init(frame, at: .now), lastHaptic: .distantPast)
    }

    // MARK: Words

    private func loupe(_ index: Int) -> some View {
        HStack(spacing: 4) {
            if case .zoomed = touch {
                Image(systemName: "plus.magnifyingglass")
                    .font(.caption2.weight(.semibold))
                    .foregroundStyle(.secondary)
                    .accessibilityHidden(true)
            }
            Text(Self.loupeText(frames[index], number: index + 1))
        }
        .font(.caption.weight(.semibold))
        .monospacedDigit()
        .lineLimit(1)
        .padding(.horizontal, 10)
        .padding(.vertical, 5)
        .glassEffect(.regular, in: .capsule)
    }

    /// "Frame 18 · IMG_0418.DNG · 14%", without a share "Frame 18 ·
    /// IMG_0418.DNG", "Frame 18 · IMG_0418.DNG · left out".
    static func loupeText(_ frame: RailFrame, number: Int) -> LocalizedStringResource {
        let name = frame.name
        if frame.leftOut {
            return LocalizedStringResource(
                "Frame \(number) · \(name) · left out",
                comment: "Over the focus rail as a finger scrubs: a frame the stack left out. The placeholders are the frame's number in the stack and its file name.")
        }
        guard let share = frame.share else {
            return LocalizedStringResource(
                "Frame \(number) · \(name)",
                comment: "Over the focus rail as a finger scrubs, for a render that kept no record of where each part came from. The placeholders are the frame's number in the stack and its file name.")
        }
        let percent = percentText(share)
        return LocalizedStringResource(
            "Frame \(number) · \(name) · \(percent)",
            comment: "Over the focus rail as a finger scrubs. The placeholders are the frame's number in the stack, its file name, and its share of the result's pixels, formatted ('14%', '<1%').")
    }

    /// "14%", "<1%".
    private static func percentText(_ share: Float) -> String {
        share > 0 && share < 0.005
            ? "<" + (0.01).formatted(.percent.precision(.fractionLength(0)))
            : Double(share).formatted(.percent.precision(.fractionLength(0)))
    }

    /// "18/46", or "–/46" with no frame selected.
    private var readout: LocalizedStringResource {
        let count = frames.count
        guard let selection else {
            return LocalizedStringResource(
                "–/\(count)",
                comment: "Beside the focus rail while no frame is shown (the result is). The placeholder is how many frames the stack has.")
        }
        return Self.count(selection + 1, of: count)
    }

    /// "18/46": the frame shown of how many, or how many a running stack
    /// has taken in (`RunningRail`).
    static func count(_ number: Int, of count: Int) -> LocalizedStringResource {
        LocalizedStringResource(
            "\(number)/\(count)",
            comment: "Beside the focus rail: the frame shown's place, of how many frames the stack has ('18/46'); while a stack runs, how many frames it has taken in, of how many ('9/66').")
    }

    static let railLabel = LocalizedStringResource(
        "Frames", comment: "VoiceOver label of the focus rail, which steps through the stack's frames; its value is the frame shown.")

    static let forwardTen = LocalizedStringResource(
        "Forward 10 Frames", comment: "VoiceOver action of the focus rail under the photo: shows the frame ten on in the stack.")
    static let backTen = LocalizedStringResource(
        "Back 10 Frames", comment: "VoiceOver action of the focus rail under the photo: shows the frame ten back in the stack.")
    static let firstFrame = LocalizedStringResource(
        "First Frame", comment: "VoiceOver action of the focus rail under the photo: shows the stack's first frame.")
    static let lastFrame = LocalizedStringResource(
        "Last Frame", comment: "VoiceOver action of the focus rail under the photo: shows the stack's last frame.")

    /// "Frame 18 of 46, IMG_0418.DNG, 14%" ("left out", or no share without
    /// a frame map), or "No frame shown".
    private var spokenValue: LocalizedStringResource {
        guard let selection, frames.indices.contains(selection) else {
            return LocalizedStringResource(
                "No frame shown",
                comment: "VoiceOver value of the focus rail while no frame is shown (the result is, or a render compared with).")
        }
        let frame = frames[selection]
        let (number, count, name) = (selection + 1, frames.count, frame.name)
        if frame.leftOut {
            return LocalizedStringResource(
                "Frame \(number) of \(count), \(name), left out",
                comment: "VoiceOver value of the focus rail: the frame shown, which the stack left out. The placeholders are its place, how many frames there are, and its file name.")
        }
        guard let share = frame.share else {
            return LocalizedStringResource(
                "Frame \(number) of \(count), \(name)",
                comment: "VoiceOver value of the focus rail: the frame shown, its place of how many, and its file name.")
        }
        let percent = Self.percentText(share)
        return LocalizedStringResource(
            "Frame \(number) of \(count), \(name), \(percent)",
            comment: "VoiceOver value of the focus rail: the frame shown, its place of how many, its file name, and its share of the result's pixels, formatted ('14%', '<1%').")
    }
}

/// The rail while a stack runs: the live map's key, read-only. A flat tick
/// per frame, in its legend color once taken in (`StackingState.railKey`,
/// the engine's `runningLegend`): the finished map's colors, so nothing
/// changes as the result lands. Nothing is selected and a touch does
/// nothing. ADR-149.
struct RunningRail: View {
    /// `nil` before it is taken in.
    let colors: [LegendColor?]
    let taken: Int

    @Environment(\.displayScale) private var displayScale

    var body: some View {
        HStack(spacing: 8) {
            ticks
            Text(FocusRail.count(taken, of: colors.count))
                .accessibilityHidden(true)
                .font(.caption.weight(.semibold))
                .monospacedDigit()
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .fixedSize()
        }
        .padding(.horizontal, 12)
        .frame(minHeight: 44)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(LocalizedStringResource(
            "\(taken) of \(colors.count) frames stacked",
            comment: "VoiceOver label of the focus rail while a stack runs, the key to the depth map as it forms: how many of the stack's frames are in so far, of how many. The placeholders are the two counts."))
    }

    private var ticks: some View {
        GeometryReader { geometry in
            let origin = geometry.frame(in: .global).origin
            RailDrawing(
                frames: frames, selection: nil, marks: [], markTint: .clear,
                scale: displayScale,
                phase: CGPoint(
                    x: Self.phase(origin.x * displayScale), y: Self.phase(origin.y * displayScale)),
                offset: 0, slot: geometry.size.width / CGFloat(max(colors.count, 1)))
        }
        .frame(height: FocusRail.drawnHeight)
        .allowsHitTesting(false)
    }

    /// All one height (no shares yet), none left out.
    private var frames: [RailFrame] {
        colors.map { color in
            RailFrame(path: "", share: nil, leftOut: false, legend: color?.color)
        }
    }

    private static func phase(_ x: CGFloat) -> CGFloat { x - x.rounded(.down) }
}

/// A finger or pointer on the rail, the rail's own state.
///
/// Lifecycle: idle, hovering, scrubbing or zoomed (magnified under a
/// resting finger). Changed only by `FocusRail.setTouch`.
///
/// - `idle` → `hovering`: a pointer comes over the ticks; back as it
///   leaves.
/// - `idle` or `hovering` → `scrubbing`: a finger or click lands, selecting
///   the frame under it; the rail's width is kept with the touch.
/// - `scrubbing` → `zoomed`: the finger rests (under 3 points) 350 ms; the
///   rail magnifies to 10 points a frame about it, and the finger moves
///   relative from then, panning near either end.
/// - `scrubbing` or `zoomed` → `idle`: it lifts; the frame rested on is
///   kept (`Trail.rested`), and a magnified rail goes back.
enum RailTouch {
    case idle
    /// A pointer over the frame.
    case hovering(frame: Int)
    case scrubbing(Scrub)
    case zoomed(Zoom)

    struct Scrub {
        /// As the finger landed.
        let width: CGFloat
        var x: CGFloat
        /// `nil` before the first: home.
        var frame: Int?
        /// Whether it landed on the selection, and has left it since, so a
        /// tap is told from a drag.
        let landedOnSelected: Bool
        var moved: Bool
        /// For the hold.
        var rest: Rest
        var trail: Trail
        var lastHaptic: Date
    }

    struct Zoom {
        let width: CGFloat
        /// Frame i's center is at `offset + 10 · (i + 0.5)`; a pan moves it
        /// a frame.
        var offset: CGFloat
        /// The finger's place as the rail magnified, and now.
        let anchorX: CGFloat
        var x: CGFloat
        var frame: Int
        var trail: Trail
        var lastHaptic: Date

        /// −1 or 1 while the finger, moved since magnifying, is held near
        /// the first or last end; 0 otherwise.
        func panDirection(edge: CGFloat, slop: CGFloat) -> Int {
            guard abs(x - anchorX) >= slop else { return 0 }
            if x < edge, x < anchorX { return -1 }
            if x > width - edge, x > anchorX { return 1 }
            return 0
        }
    }

    struct Rest: Equatable {
        let x: CGFloat
        let since: Date
    }

    /// The frames under the finger lately, with when.
    struct Trail {
        private var runs: [(frame: Int?, since: Date)]

        init(_ frame: Int?, at time: Date) { runs = [(frame, time)] }

        mutating func enter(_ frame: Int?, at time: Date) {
            runs.append((frame, time))
            if runs.count > 16 { runs.removeFirst(runs.count - 16) }
        }

        /// The frame to keep at a lift: the one under the finger `window`
        /// before if it stayed at least `window` and was left since (a
        /// finger rolling as it lifts); else the last, so a flick ends where
        /// it ends.
        func rested(at time: Date, window: TimeInterval) -> Int? {
            let last = runs[runs.count - 1].frame
            let then = time.addingTimeInterval(-window)
            guard let index = runs.lastIndex(where: { $0.since <= then }), index < runs.count - 1 else { return last }
            let held = runs[index + 1].since.timeIntervalSince(runs[index].since)
            return held >= window ? runs[index].frame : last
        }
    }

    /// The hold's key: a scrubbing finger's rest.
    var restKey: Date? {
        if case .scrubbing(let scrub) = self { scrub.rest.since } else { nil }
    }

    /// 0 for none.
    func panDirection(edge: CGFloat, slop: CGFloat) -> Int {
        if case .zoomed(let zoom) = self { zoom.panDirection(edge: edge, slop: slop) } else { 0 }
    }

    var logName: String {
        switch self {
        case .idle: "idle"
        case .hovering: "hovering"
        case .scrubbing: "scrubbing"
        case .zoomed(let zoom): "zoomed on frame \(zoom.frame + 1)"
        }
    }
}

/// Frame i's room starts at `offset + slot · i` points.
struct RailLens {
    var offset: CGFloat
    var slot: CGFloat

    func center(_ index: Int) -> CGFloat { offset + slot * (CGFloat(index) + 0.5) }

    /// The first or last past either end.
    func frame(at x: CGFloat, count: Int) -> Int {
        guard slot > 0, count > 0 else { return 0 }
        return min(max(Int(((x - offset) / slot).rounded(.down)), 0), count - 1)
    }
}

/// The rail's ticks, on screen pixels, animatable between full width and
/// magnified (`offset`, `slot`). By the device pixels per frame, p:
/// - p ≥ 6: a tick half the room (round(p/2) pixels, 3 points at most),
///   centered;
/// - 3 ≤ p < 6: the room less a pixel gap;
/// - p < 3: a column per pixel, no gaps, as tall as its tallest frame
///   (never a mean), a little lighter.
/// Frames left out are an amber run (a pixel each at least) in a lane
/// underneath, where an amber tick would hide a sharp neighbor; dots that
/// would overlap merge into a capsule; the selection is marked over all.
private struct RailDrawing: View, Animatable {
    let frames: [RailFrame]
    let selection: Int?
    let marks: IndexSet
    let markTint: Color
    let scale: CGFloat
    /// The canvas origin's fraction of a screen pixel.
    let phase: CGPoint
    var offset: CGFloat
    var slot: CGFloat

    var animatableData: AnimatablePair<CGFloat, CGFloat> {
        get { AnimatablePair(offset, slot) }
        set { (offset, slot) = (newValue.first, newValue.second) }
    }

    /// Ticks' shortest and tallest, and the selected one's height.
    static let tickMin: CGFloat = 4
    static let tickMax: CGFloat = 18
    static let selectedHeight: CGFloat = 24
    /// The lane of frames left out, under the ticks, and its gap above.
    static let laneHeight: CGFloat = 2
    static let laneGap: CGFloat = 1
    static let markSide: CGFloat = 4
    static let markLeast: CGFloat = 3

    var body: some View {
        Canvas { context, size in draw(in: &context, size: size) }
    }

    private func draw(in context: inout GraphicsContext, size: CGSize) {
        let count = frames.count
        guard count > 0, slot > 0, scale > 0 else { return }
        let s = scale
        let p = slot * s
        // A pixel column k of the canvas starts at x(k) points.
        func x(_ k: Int) -> CGFloat { (CGFloat(k) - phase.x) / s }
        func edge(_ i: Int) -> Int { Int(((offset + slot * CGFloat(i)) * s + phase.x).rounded()) }
        let baseK = Int(((size.height - Self.laneHeight - Self.laneGap) * s + phase.y).rounded())
        let base = (CGFloat(baseK) - phase.y) / s
        func height(_ h: CGFloat) -> CGFloat { max((h * s).rounded(), 1) / s }
        let columns = Int((size.width * s).rounded(.up)) + 1
        let first = max(Int(((-offset) / slot).rounded(.down)) - 1, 0)
        let last = min(Int(((size.width - offset) / slot).rounded(.up)) + 1, count - 1)
        guard first <= last else { return }
        let top = frames.compactMap(\.share).max() ?? 0
        func tickHeight(_ frame: RailFrame) -> CGFloat {
            height(Self.height(share: frame.share, top: top))
        }

        let tickWidth: CGFloat
        if p < 3 {
            tickWidth = 1 / s
            var tallest = [CGFloat](repeating: 0, count: columns)
            var owner = [Int](repeating: -1, count: columns)
            for i in first...last where !frames[i].leftOut {
                let a = edge(i)
                let b = max(edge(i + 1), a + 1)
                let h = tickHeight(frames[i])
                for k in max(a, 0)..<min(max(b, 0), columns) where h > tallest[k] {
                    tallest[k] = h
                    owner[k] = i
                }
            }
            var plain = Path()
            for k in 0..<columns where tallest[k] > 0 {
                let rect = CGRect(x: x(k), y: base - tallest[k], width: 1 / s, height: tallest[k])
                if let legend = frames[owner[k]].legend {
                    context.fill(Path(rect), with: .color(legend.opacity(0.9)))
                } else {
                    plain.addRect(rect)
                }
            }
            context.fill(plain, with: .color(.primary.opacity(0.5)))
        } else {
            let wide = p >= 6
            let least = Int(min((0.5 * p).rounded(), (3 * s).rounded(.down)))
            tickWidth = wide ? CGFloat(least) / s : max(p - 1, 1) / s
            var plain = Path()
            for i in first...last where i != selection && !frames[i].leftOut {
                let a = edge(i)
                let span = edge(i + 1) - a
                let (k, w) = wide ? (a + (span - least) / 2, least) : (a, max(span - 1, 1))
                let h = tickHeight(frames[i])
                let rect = CGRect(x: x(k), y: base - h, width: CGFloat(w) / s, height: h)
                let path = wide ? Path(roundedRect: rect, cornerRadius: min(rect.width / 2, 1.5)) : Path(rect)
                if let legend = frames[i].legend {
                    context.fill(path, with: .color(legend))
                } else {
                    plain.addPath(path)
                }
            }
            context.fill(plain, with: .color(.primary.opacity(0.55)))
        }

        // The frames left out: their pixels in the lane, a pixel at least.
        var lane = Path()
        let laneTop = base + Self.laneGap
        for i in first...last where frames[i].leftOut {
            let a = edge(i)
            let b = max(edge(i + 1), a + 1)
            lane.addRect(CGRect(x: x(a), y: laneTop, width: CGFloat(b - a) / s, height: Self.laneHeight))
        }
        context.fill(lane, with: .color(.orange))

        // The marks: dots that would overlap, one capsule.
        var dots = Path()
        var run: (from: CGFloat, to: CGFloat)?
        func close(_ run: (from: CGFloat, to: CGFloat)) {
            let width = max(run.to - run.from, Self.markLeast)
            let mid = (run.from + run.to) / 2
            dots.addRoundedRect(
                in: CGRect(x: mid - width / 2, y: 0, width: width, height: Self.markSide),
                cornerSize: CGSize(width: Self.markSide / 2, height: Self.markSide / 2))
        }
        for i in marks where frames.indices.contains(i) {
            let center = offset + slot * (CGFloat(i) + 0.5)
            let (from, to) = (center - Self.markSide / 2, center + Self.markSide / 2)
            if let open = run, from <= open.to {
                run = (open.from, to)
            } else {
                if let open = run { close(open) }
                run = (from, to)
            }
        }
        if let open = run { close(open) }
        context.fill(dots, with: .color(markTint))

        // The frame selected, at its own center, over everything.
        if let selection, frames.indices.contains(selection) {
            let center = offset + slot * (CGFloat(selection) + 0.5)
            let frame = frames[selection]
            let h = min(Self.selectedHeight, base - Self.markSide - 1)
            if !frame.leftOut, let legend = frame.legend {
                // Its own color, ringed: the accent among the legend's colors
                // would read as one more frame's.
                let width = max(tickWidth, 3) + 4
                let ring = CGRect(x: center - width / 2, y: base - h, width: width, height: h)
                let inner = ring.insetBy(dx: 2, dy: 2)
                context.fill(Path(roundedRect: ring, cornerRadius: width / 2), with: .color(.primary))
                context.fill(Path(roundedRect: inner, cornerRadius: inner.width / 2), with: .color(legend))
            } else {
                let width = max(tickWidth + 1, 2)
                let rect = CGRect(x: center - width / 2, y: base - h, width: width, height: h)
                context.fill(Path(roundedRect: rect, cornerRadius: width / 2), with: .color(.accentColor))
            }
        }
    }

    /// Its share against the largest, or mid-range without shares.
    static func height(share: Float?, top: Float) -> CGFloat {
        guard let share else { return (tickMin + tickMax) / 2 }
        guard top > 0 else { return tickMin }
        return tickMin + (tickMax - tickMin) * CGFloat(min(max(share / top, 0), 1))
    }
}

/// A trackpad scroll or wheel over the rail, as points on its larger axis.
/// Touches pass to the rail's drag: this takes scrolls alone.
private struct RailScroll: UIGestureRecognizerRepresentable {
    let scrolled: (CGFloat) -> Void

    init(_ scrolled: @escaping (CGFloat) -> Void) { self.scrolled = scrolled }

    final class Coordinator {
        var last = CGPoint.zero
    }

    func makeCoordinator(converter: CoordinateSpaceConverter) -> Coordinator { Coordinator() }

    func makeUIGestureRecognizer(context: Context) -> UIPanGestureRecognizer {
        let pan = UIPanGestureRecognizer()
        pan.allowedScrollTypesMask = .all
        pan.allowedTouchTypes = []
        return pan
    }

    func handleUIGestureRecognizerAction(_ recognizer: UIPanGestureRecognizer, context: Context) {
        switch recognizer.state {
        case .began:
            context.coordinator.last = .zero
        case .changed:
            let now = recognizer.translation(in: recognizer.view)
            let last = context.coordinator.last
            context.coordinator.last = now
            let (dx, dy) = (now.x - last.x, now.y - last.y)
            scrolled(abs(dx) > abs(dy) ? dx : dy)
        default:
            break
        }
    }
}

/// A finger scrubs a rail, its loupe showing: views over the photo give
/// way.
struct RailScrubbing: PreferenceKey {
    static let defaultValue = false
    static func reduce(value: inout Bool, nextValue: () -> Bool) { value = value || nextValue() }
}

extension EnvironmentValues {
    /// Automation: the loupe over the selection, as during a drag, for a
    /// screenshot.
    @Entry var automationLoupe = false
}

extension LegendColor {
    /// As the rail and the map draw it.
    nonisolated var color: Color {
        Color(.sRGB, red: Double(red) / 255, green: Double(green) / 255, blue: Double(blue) / 255)
    }
}
