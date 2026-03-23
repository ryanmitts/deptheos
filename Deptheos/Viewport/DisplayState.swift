import Metal

/// The user's preferred comparison style when viewing a source image alongside the result.
enum ComparisonStyle: String, CaseIterable, Identifiable, Sendable {
    case compare = "Compare"
    case sideBySide = "Side by Side"
    case sideBySideVertical = "Side by Side Vertical"

    var id: String { rawValue }

    var icon: String {
        switch self {
        case .compare: "rectangle.lefthalf.filled"
        case .sideBySide: "square.split.2x1"
        case .sideBySideVertical: "square.split.1x2"
        }
    }
}

/// View modes derived from app state. Computed on AppState, read by DisplayState via closure.
enum ViewMode: String, Sendable {
    case preview = "Preview"
    case result = "Result"
    case compare = "Compare"
    case sideBySide = "Side by Side"
    case sideBySideVertical = "Side by Side Vertical"

    var icon: String {
        switch self {
        case .preview, .result: "rectangle"
        case .compare: "rectangle.lefthalf.filled"
        case .sideBySide: "square.split.2x1"
        case .sideBySideVertical: "square.split.1x2"
        }
    }
}

/// Single owner for all "what is currently displayed" state.
///
/// The viewport layer is generic — it knows about textures in slots and display modes,
/// not about stacking, projects, or DNG color science.
@Observable
@MainActor
final class DisplayState {

    // MARK: - Texture Slots

    /// Primary display texture (sRGB, ready for screen).
    private(set) var mainTexture: MTLTexture? {
        didSet { onDirty?() }
    }

    /// Secondary texture for compare/side-by-side modes.
    private(set) var compareTexture: MTLTexture? {
        didSet { onDirty?() }
    }

    // MARK: - View Mode (computed via closure from AppState)

    /// Returns the current view mode. Provided by AppState at init time.
    @ObservationIgnored
    var viewModeProvider: (() -> ViewMode)?

    /// The current view mode — delegates to AppState's computed property.
    var viewMode: ViewMode {
        viewModeProvider?() ?? .preview
    }

    /// Split position for compare mode (0.0 = all compare, 1.0 = all main).
    var splitPosition: Float = 0.5 {
        didSet { onDirty?() }
    }

    /// Whether the loupe tool is currently active (right-mouse held).
    var isLoupeActive: Bool = false {
        didSet { onDirty?() }
    }

    // MARK: - Image Dimensions

    private(set) var imageWidth: Int = 0
    private(set) var imageHeight: Int = 0

    // MARK: - Dirty Callback

    /// Called when display state changes that require a re-render.
    @ObservationIgnored
    var onDirty: (() -> Void)?

    // MARK: - Mutators

    func setMainTexture(_ texture: MTLTexture?, width: Int, height: Int) {
        self.mainTexture = texture
        self.imageWidth = width
        self.imageHeight = height
    }

    func setCompareTexture(_ texture: MTLTexture?) {
        self.compareTexture = texture
    }

    func clear() {
        mainTexture = nil
        compareTexture = nil
        imageWidth = 0
        imageHeight = 0
    }
}
