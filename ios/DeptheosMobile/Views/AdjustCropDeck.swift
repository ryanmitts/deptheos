import SwiftUI
@preconcurrency import DeptheosEngine

/// Adjust's Crop tool: the crop's shape, turn, flip and reset; the crop
/// itself is dragged on the photo. Each change is one undoable step, the
/// engine's (`reframeTurned`, `reframeFlipped`, `CropFrame`).
struct AdjustCropDeck: View {
    @Environment(AppModel.self) private var model
    @Binding var aspect: CropAspect

    private var adjust: AdjustModel { model.adjust }

    var body: some View {
        let reframe = adjust.value?.reframe() ?? Reframe(crop: nil, flip: .none, turn: .none)
        VStack(spacing: 10) {
            ScrollView(.horizontal) {
                HStack(spacing: 6) {
                    ForEach(cropAspects(), id: \.self) { choice in
                        Button(Self.name(choice)) { pick(choice, reframe: reframe) }
                            .buttonStyle(.bordered)
                            .tint(aspect == choice || aspect == cropAspectTurned(aspect: choice) ? .accentFill : .secondary)
                            .controlSize(.small)
                    }
                    if case .ratio = aspect {
                        Button {
                            pick(cropAspectTurned(aspect: aspect), reframe: reframe)
                        } label: {
                            Label(
                                LocalizedStringResource("Swap Width and Height", comment: "Adjust's Crop tool: turns the crop's shape, 4:5 to 5:4."),
                                systemImage: "arrow.left.arrow.right")
                                .labelStyle(.iconOnly)
                        }
                        .buttonStyle(.bordered)
                        .controlSize(.small)
                        .keyboardShortcut("x", modifiers: [])
                    }
                }
            }
            .scrollIndicators(.hidden)
            HStack(spacing: 8) {
                button(
                    LocalizedStringResource("Turn Left", comment: "Adjust's Crop tool: a quarter turn counterclockwise."),
                    symbol: "rotate.left", id: "adjust-turn-left"
                ) { turn(right: false, reframe) }
                    .keyboardShortcut("[", modifiers: .command)
                button(
                    LocalizedStringResource("Turn Right", comment: "Adjust's Crop tool: a quarter turn clockwise."),
                    symbol: "rotate.right", id: "adjust-turn-right"
                ) { turn(right: true, reframe) }
                    .keyboardShortcut("]", modifiers: .command)
                button(
                    LocalizedStringResource("Flip Horizontal", comment: "Adjust's Crop tool: mirrors the photo left to right."),
                    symbol: "arrow.left.and.right.righttriangle.left.righttriangle.right", id: "adjust-flip-horizontal"
                ) { change(reframe, to: reframeFlipped(reframe: reframe, vertical: false)) }
                button(
                    LocalizedStringResource("Flip Vertical", comment: "Adjust's Crop tool: mirrors the photo top to bottom."),
                    symbol: "arrow.up.and.down.righttriangle.up.righttriangle.down", id: "adjust-flip-vertical"
                ) { change(reframe, to: reframeFlipped(reframe: reframe, vertical: true)) }
                Spacer(minLength: 0)
                Button(LocalizedStringResource("Reset Crop", comment: "Adjust's Crop tool: the whole photo, unturned and unflipped.")) {
                    aspect = .original
                    change(reframe, to: Reframe(crop: nil, flip: .none, turn: .none))
                }
                .disabled(reframe.crop == nil && reframe.flip == .none && reframe.turn == .none)
                .accessibilityIdentifier("adjust-reset-crop")
            }
            .buttonStyle(.bordered)
        }
    }

    /// "Original", "Free", "4:5".
    static func name(_ aspect: CropAspect) -> LocalizedStringResource {
        switch aspect {
        case .original: LocalizedStringResource("Original", comment: "Adjust's Crop tool: the crop keeps the photo's own shape.")
        case .free: LocalizedStringResource("Free", comment: "Adjust's Crop tool: the crop takes any shape.")
        case .ratio(let width, let height):
            LocalizedStringResource(
                "\(width):\(height)",
                comment: "Adjust's Crop tool: a crop shape. The placeholders are its sides, as in 4:5.")
        }
    }

    private func button(
        _ title: LocalizedStringResource, symbol: String, id: String, action: @escaping () -> Void
    ) -> some View {
        Button(action: action) {
            Label(title, systemImage: symbol).labelStyle(.iconOnly)
        }
        .help(title)
        .accessibilityIdentifier(id)
    }

    /// A quarter turn as shown, the shape turned with the picture.
    private func turn(right: Bool, _ reframe: Reframe) {
        aspect = cropAspectTurned(aspect: aspect)
        change(reframe, to: reframeTurned(reframe: reframe, right: right))
    }

    /// The crop the largest of `choice`'s shape inside the one now.
    private func pick(_ choice: CropAspect, reframe: Reframe) {
        aspect = choice
        guard let stack = model.result?.stack else { return }
        let stored = stack.storedSize
        let frame = CropFrame(width: stored.width, height: stored.height, reframe: reframe)
        change(reframe, to: frame.reframed(area: frame.shaped(area: frame.area(), aspect: choice)))
    }

    private func change(_ reframe: Reframe, to next: Reframe) {
        guard next != reframe else { return }
        adjust.setReframe(next)
        adjust.commit()
    }
}
