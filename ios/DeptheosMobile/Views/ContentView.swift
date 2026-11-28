import SwiftUI
@preconcurrency import DeptheosEngine

/// The app's two screens (`AppScreen`): the Library, home, and Stacking over
/// it while a stack is open. The Library stays under, so it is as left.
struct ContentView: View {
    @Environment(AppModel.self) private var model
    /// So Stacking is built for it from its first frame, not corrected while
    /// sliding in.
    @State private var width: CGFloat = 0
    /// As it was when the Library was last on screen.
    @State private var libraryLocked = false

    private var photoOnly: Bool {
        model.screen == .stacking && model.panel.hidesControls
    }

    var body: some View {
        @Bindable var model = model
        ZStack {
            LibraryView()
                .accessibilityHidden(model.screen == .stacking)
                // Covered, or opening a tap, it takes no touches. What it
                // grays out is kept while covered: nothing dims on the tap
                // that opens a stack, nor redraws while Stacking slides over.
                .allowsHitTesting(model.screen == .library && !model.busy)
                .environment(\.libraryLocked, libraryLocked)
                .onChange(of: model.screen == .library ? model.libraryHeld : nil, initial: true) {
                    _, held in
                    if let held { libraryLocked = held }
                }
            if model.screen == .stacking {
                StackingView(width: width, framesEmpty: model.rows.isEmpty)
                    // It fades, never slides: a bar the system stands on its
                    // side (the Duo) flashes through a move (ADR-163).
                    .transition(.opacity)
                    .zIndex(1)
            }
        }
        // The screen's colors, which pictures are drawn in.
        .background { ScreenGamutReader() }
        // Stacking's tap on the photo hides the status bar and home
        // indicator too. Set here: from inside Stacking, beside the
        // Library's navigation, the system never read it.
        .statusBarHidden(photoOnly)
        .persistentSystemOverlays(photoOnly ? .hidden : .automatic)
        .measure("window width", for: CGFloat.self) { $0.size.width } action: { width = $0 }
        .animation(.snappy(duration: 0.3), value: model.screen)
        .fullScreenCover(isPresented: Binding(
            get: { model.showCamera }, set: { if !$0 { model.dismiss(.camera) } })
        ) {
            CameraView()
                .errorAlert()
        }
        // `-OpenCamera YES`: the camera at launch (after `-ShowPanel`, so
        // over a sheet as Take Burst there).
        .automation(Automation.flag("OpenCamera"), after: .seconds(3)) {
            model.present(.camera)
        }
        .errorAlert()
    }
}

/// Stacking's way back (`cancellationAction`): a symbol in the bar, a title
/// for the overflow menu, VoiceOver and the pointer, so a side bar (iPhone
/// Duo) takes it.
struct LibraryBackButton: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        Button {
            model.setScreen(.library)
        } label: {
            Label(Self.library, systemImage: "chevron.backward")
        }
        .accessibilityHint(LocalizedStringResource(
            "Shows every stack", comment: "VoiceOver hint of the back button to the Library."))
        .help(LocalizedStringResource(
            "Back to the Library", comment: "Pointer tooltip of the back button to the Library."))
    }

    private static let library = LocalizedStringResource(
        "Library", comment: "Back button at the leading edge of the Stacking screen's bar: back to the Library.")
}

extension EnvironmentValues {
    /// The Library's commands grayed out (`AppModel.libraryHeld`), as when
    /// it was last on screen. Read in place of `AppModel.busy` so a change
    /// under Stacking redraws nothing; a tile opening only blocks touches,
    /// so nothing dims on the tap.
    @Entry var libraryLocked = false
}
