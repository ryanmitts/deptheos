# SlugFont

GPU-accelerated vector font rendering for Metal using the [Slug algorithm](https://jcgt.org/published/0006/02/02/) by Eric Lengyel. Renders resolution-independent text directly on the GPU by tracing rays against quadratic Bezier curves stored in textures.

Font conversion to the format this library uses is based on my fork of Sluggish at https://github.com/ryanmitts/Sluggish.

## Package Structure

- **SlugFont** — Library for rendering text onto Metal textures
- **slug-font-cli** — CLI tool that renders text in a window

## Usage

### Library

```swift
import SlugFont

// One-time setup (reuse across frames)
let context = try SlugMetalContext(device: device)
let font = try SlugFont(device: device)  // bundled Roboto-Regular
let renderer = try SlugFontRenderer(
    context: context, font: font,
    pixelFormat: .bgra8Unorm  // must match your target texture
)

// Render onto an existing texture (preserves content, alpha-blended)
let commandBuffer = commandQueue.makeCommandBuffer()!
renderer.renderToTexture(
    commandBuffer: commandBuffer,
    text: "Hello World",
    origin: SIMD2<Float>(100, 50),    // pixels from top-left
    size: 32,                          // font size in pixels
    color: SIMD4<Float>(1, 1, 1, 1),  // RGBA
    texture: targetTexture
)
commandBuffer.commit()
```

For more control, encode directly into your own render pass:

```swift
renderer.encode(
    encoder: renderEncoder,
    text: "overlay",
    origin: SIMD2<Float>(10, 10),
    size: 24,
    color: SIMD4<Float>(1, 0.8, 0, 1),
    viewportSize: SIMD2<Float>(Float(width), Float(height))
)
```

### Loading Custom Fonts

```swift
let font = try SlugFont(device: device, url: URL(fileURLWithPath: "/path/to/font.sluggish"))
```

Generate `.sluggish` files from TrueType fonts using the generator in `/Sluggish/`:

```bash
cd /path/to/Sluggish
make fontgen
./bin/fontgen MyFont.ttf
```

### CLI

```bash
swift run slug-font-cli "Hello World"
swift run slug-font-cli "Hello" --font /path/to/font.sluggish --size 128
```

## API

### SlugMetalContext

Shared Metal device, command queue, and shader library. Handles runtime shader compilation for SPM.

```swift
let context = try SlugMetalContext()                    // default device
let context = try SlugMetalContext(device: myDevice)    // specific device
```

### SlugFont

Loads a `.sluggish` font file and creates GPU textures (curves + bands). Provides glyph metrics and kerning lookup.

```swift
let font = try SlugFont(device: device)                        // bundled Roboto
let font = try SlugFont(device: device, url: fontURL)          // custom font

font.metrics          // .ascender, .descender, .unitsPerEm, .lineGap
font.glyphs[0x41]    // GlyphInfo for 'A': advanceWidth, bearingY, etc.
font.kern(left: 0x57, right: 0x6F)  // kern adjustment for "Wo"
```

### SlugFontRenderer

Renders text using instanced draw calls. Create one per target pixel format.

| Method | Use case |
|--------|----------|
| `encode(encoder:text:origin:size:color:viewportSize:)` | Inject into your own render pass |
| `encodeRender(commandBuffer:text:origin:size:color:renderPassDescriptor:viewportSize:)` | Render into a render pass descriptor |
| `renderToTexture(commandBuffer:text:origin:size:color:texture:)` | Render onto any texture (loadAction: .load) |

## Requirements

- macOS 14+
- Target texture must have `.renderTarget` usage flag

## How It Works

The Slug algorithm stores font outlines as quadratic Bezier curves in two GPU textures:

- **Curves texture** (RGBA32F) — Bezier control points
- **Bands texture** (RG16Uint) — Spatial index for efficient per-pixel curve lookup

The fragment shader traces horizontal and vertical rays against curves in the relevant bands, solving quadratic equations to determine coverage. This produces resolution-independent, analytically antialiased text in a single draw call per text string.

## .sluggish File Format

Binary format storing pre-processed font data:

| Section | Contents |
|---------|----------|
| Header | `"SLUGGISH"` magic (8 bytes) |
| Font metrics | unitsPerEm, ascender, descender, lineGap |
| Glyph table | Per-glyph: bounding box, band info, advance width, bearings |
| Kern pairs | Non-zero kerning adjustments between glyph pairs |
| Curves texture | RGBA32F Bezier control point data (4096 wide) |
| Bands texture | RG16Uint spatial index data (4096 wide) |

## License

Slug algorithm: MIT or Apache-2.0 (Eric Lengyel)
