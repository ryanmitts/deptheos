import SwiftUI
import UIKit
@preconcurrency import DeptheosEngine

private let log = AppLog(category: "display")

/// Tells the engine the space the window's screen shows: Display P3 on a
/// wide-gamut screen, else sRGB, as the window first appears and whenever
/// its screen's gamut changes (an external display). Pictures shown before
/// keep their tags, which the system converts.
struct ScreenGamutReader: UIViewRepresentable {
    func makeUIView(context: Context) -> GamutView { GamutView() }
    func updateUIView(_ view: GamutView, context: Context) {}

    final class GamutView: UIView {
        private var told: ScreenSpace?

        override init(frame: CGRect) {
            super.init(frame: frame)
            isUserInteractionEnabled = false
            isHidden = true
            registerForTraitChanges([UITraitDisplayGamut.self]) { (view: GamutView, _) in view.tell() }
        }

        required init?(coder: NSCoder) { nil }

        override func didMoveToWindow() {
            super.didMoveToWindow()
            tell()
        }

        private func tell() {
            guard window != nil else { return }
            let space: ScreenSpace = traitCollection.displayGamut == .P3 ? .displayP3 : .srgb
            guard space != told else { return }
            log.info("screen: \(String(describing: told), privacy: .public) → \(String(describing: space), privacy: .public)")
            told = space
            setScreenSpace(space: space)
        }
    }
}
