import SwiftUI

/// The camera's shutter, showing on its face a bracket's delay, a sweep's
/// frames as saved, or a bracket's progress around its ring.
struct Shutter: View {
    enum Face: Equatable {
        case idle
        /// With the seconds left.
        case counting(Int)
        case sweep(frames: Int)
        case bracket(taken: Int, planned: Int)
        /// The burst's last frames are landing.
        case saving

        var active: Bool {
            switch self {
            case .sweep, .bracket: true
            case .idle, .counting, .saving: false
            }
        }
    }

    let face: Face
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    /// Its outer ring's diameter.
    static let side = CameraView.ScreenLayout.shutterSide

    var body: some View {
        ZStack {
            if case .sweep = face, !reduceMotion {
                // Pulses while the sweep runs; with Reduce Motion the red
                // button and its count say so.
                Circle()
                    .stroke(.red.opacity(0.8), lineWidth: 4)
                    .frame(width: Self.side, height: Self.side)
                    .phaseAnimator([false, true]) { ring, expanded in
                        ring.scaleEffect(expanded ? 1.3 : 1).opacity(expanded ? 0 : 1)
                    } animation: { expanded in
                        expanded ? .easeOut(duration: 0.7) : nil
                    }
            }
            Circle().stroke(.white, lineWidth: 5).frame(width: Self.side, height: Self.side)
            if case .bracket(let taken, let planned) = face, planned > 0 {
                // How far the bracket has come, over the white ring.
                Circle()
                    .trim(from: 0, to: CGFloat(taken) / CGFloat(planned))
                    .stroke(.red, style: StrokeStyle(lineWidth: 5, lineCap: .round))
                    .rotationEffect(.degrees(-90))
                    .frame(width: Self.side, height: Self.side)
                    .animation(reduceMotion ? nil : .easeOut(duration: 0.15), value: taken)
            }
            let isBracket = if case .bracket = face { true } else { false }
            let innerSide: CGFloat = isBracket ? 48 : face.active ? 54 : 64
            let innerRadius: CGFloat = isBracket ? 14 : innerSide / 2
            RoundedRectangle(cornerRadius: innerRadius, style: .continuous)
                .fill(face.active ? Color.red : Color.white)
                .frame(width: innerSide, height: innerSide)
                .animation(reduceMotion ? nil : .snappy(duration: 0.18), value: face.active)
                .animation(reduceMotion ? nil : .snappy(duration: 0.18), value: isBracket)
            label
        }
        .contentShape(Circle())
    }

    @ViewBuilder private var label: some View {
        switch face {
        case .counting(let left):
            Text(left, format: .number)
                .font(.title.weight(.semibold).monospacedDigit())
                .foregroundStyle(.black)
                .contentTransition(.numericText(countsDown: true))
                .animation(reduceMotion ? nil : .snappy(duration: 0.15), value: left)
        case .sweep(let frames), .bracket(let frames, _):
            Text(frames, format: .number)
                .font(.title3.weight(.semibold).monospacedDigit())
                .foregroundStyle(.white)
                .contentTransition(.numericText())
                .animation(reduceMotion ? nil : .snappy(duration: 0.12), value: frames)
        case .saving:
            ProgressView()
                .tint(.black)
        case .idle:
            EmptyView()
        }
    }
}
