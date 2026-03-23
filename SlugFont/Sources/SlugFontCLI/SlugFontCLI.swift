#if canImport(AppKit)
import AppKit
import MetalKit
import SlugFont
import ArgumentParser

@main
struct SlugFontCLI: @preconcurrency ParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "slug-font-cli",
        abstract: "Render text in a window using the Slug GPU font algorithm"
    )

    @Argument(help: "Text to render")
    var text: String

    @Option(name: .shortAndLong, help: "Path to .sluggish font file (uses bundled Roboto if omitted)")
    var font: String?

    @Option(name: .shortAndLong, help: "Font size in pixels")
    var size: Float = 64

    @MainActor func run() throws {
        try renderSlugInWindow(text: text, fontPath: font, fontSize: size)
    }
}

// MARK: - Window rendering

@MainActor
func renderSlugInWindow(text: String, fontPath: String?, fontSize: Float) throws {
    let context = try SlugMetalContext()

    let slugFont: SlugFont
    if let path = fontPath {
        slugFont = try SlugFont(device: context.device, url: URL(fileURLWithPath: path))
    } else {
        slugFont = try SlugFont(device: context.device)
    }

    let renderer = try SlugFontRenderer(context: context, font: slugFont, pixelFormat: .bgra8Unorm)

    // Estimate window size from text using real advance widths
    let m = slugFont.metrics
    let emFontUnits = Float(m.ascender - m.descender)
    let pxPerUnit = fontSize / emFontUnits
    var totalWidth: Float = 0
    for scalar in text.unicodeScalars {
        if let glyph = slugFont.glyphs[scalar.value] {
            totalWidth += Float(glyph.advanceWidth) * pxPerUnit
        } else {
            totalWidth += emFontUnits * 0.25 * pxPerUnit
        }
    }
    let padding: Float = 40
    let windowW = max(totalWidth + padding * 2, 200)
    let windowH = fontSize + padding * 2

    let app = NSApplication.shared
    app.setActivationPolicy(.regular)

    let window = NSWindow(
        contentRect: NSRect(x: 0, y: 0, width: CGFloat(windowW), height: CGFloat(windowH)),
        styleMask: [.titled, .closable, .resizable],
        backing: .buffered,
        defer: false
    )
    window.title = "SlugFont: \(text)"
    window.center()

    let mtkView = MTKView(frame: window.contentView!.bounds, device: context.device)
    mtkView.colorPixelFormat = .bgra8Unorm
    mtkView.autoresizingMask = [.width, .height]
    mtkView.clearColor = MTLClearColor(red: 0.12, green: 0.12, blue: 0.14, alpha: 1.0)
    mtkView.isPaused = false
    mtkView.enableSetNeedsDisplay = false

    let delegate = SlugViewDelegate(
        renderer: renderer,
        context: context,
        text: text,
        fontSize: fontSize,
        padding: padding
    )
    mtkView.delegate = delegate

    window.contentView!.addSubview(mtkView)
    window.makeKeyAndOrderFront(nil)

    let terminationDelegate = TerminationDelegate()
    window.delegate = terminationDelegate

    withExtendedLifetime((delegate, terminationDelegate)) {
        app.activate(ignoringOtherApps: true)
        app.run()
    }
}

final class SlugViewDelegate: NSObject, MTKViewDelegate, @unchecked Sendable {
    private let renderer: SlugFontRenderer
    private let context: SlugMetalContext
    private let text: String
    private let fontSize: Float
    private let padding: Float

    init(renderer: SlugFontRenderer, context: SlugMetalContext, text: String, fontSize: Float, padding: Float) {
        self.renderer = renderer
        self.context = context
        self.text = text
        self.fontSize = fontSize
        self.padding = padding
    }

    func mtkView(_ view: MTKView, drawableSizeWillChange size: CGSize) {}

    func draw(in view: MTKView) {
        guard let drawable = view.currentDrawable,
              let rpd = view.currentRenderPassDescriptor else { return }
        guard let commandBuffer = context.commandQueue.makeCommandBuffer() else { return }

        let viewportSize = SIMD2<Float>(Float(view.drawableSize.width), Float(view.drawableSize.height))
        let scale = Float(view.drawableSize.height) / Float(view.bounds.height)

        renderer.encodeRender(
            commandBuffer: commandBuffer,
            text: text,
            origin: SIMD2<Float>(padding * scale, padding * scale),
            size: fontSize * scale,
            color: SIMD4<Float>(1, 1, 1, 1),
            renderPassDescriptor: rpd,
            viewportSize: viewportSize
        )

        commandBuffer.present(drawable)
        commandBuffer.commit()
    }
}

@MainActor
private final class TerminationDelegate: NSObject, NSWindowDelegate {
    func windowWillClose(_ notification: Notification) {
        NSApplication.shared.terminate(nil)
    }
}

#else
@main
struct SlugFontCLI {
    static func main() {
        print("SlugFontCLI requires macOS with AppKit")
    }
}
#endif
