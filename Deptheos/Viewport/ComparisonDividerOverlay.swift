import SwiftUI
#if canImport(AppKit)
import AppKit
#endif

struct ComparisonDividerOverlay: View {
    @Binding var dividerPosition: Float
    let containerSize: CGSize
    let viewMode: ViewMode

    @State private var isDragging = false
    @State private var isHovering = false

    private let hitDistance: CGFloat = 20

    var body: some View {
        ZStack {
            switch viewMode {
            case .preview, .result:
                EmptyView()
            case .compare:
                compareDivider
            case .sideBySide:
                sideBySideDivider
            case .sideBySideVertical:
                sideBySideVerticalDivider
            }
        }
        .coordinateSpace(.named("dividerContainer"))
    }

    private var compareDivider: some View {
        let screenX = containerSize.width * CGFloat(dividerPosition)

        return ZStack {
            Rectangle()
                .fill(Color.white.opacity(isDragging || isHovering ? 1.0 : 0.7))
                .frame(width: isDragging ? 4 : 3, height: containerSize.height)
                .shadow(radius: isDragging ? 4 : 2)
                .position(x: screenX, y: containerSize.height / 2)
                .allowsHitTesting(false)
        }
        .overlay(alignment: .leading) {
            Rectangle()
                .fill(Color.clear)
                .frame(width: hitDistance * 2, height: containerSize.height)
                .contentShape(Rectangle())
                .gesture(
                    DragGesture(coordinateSpace: .named("dividerContainer"))
                        .onChanged { value in
                            isDragging = true
                            dividerPosition = Float(max(0.05, min(0.95, value.location.x / containerSize.width)))
                        }
                        .onEnded { _ in isDragging = false }
                )
                .onContinuousHover { phase in
                    switch phase {
                    case .active:
                        isHovering = true
                        #if canImport(AppKit)
                        NSCursor.resizeLeftRight.set()
                        #endif
                    case .ended:
                        isHovering = false
                        #if canImport(AppKit)
                        NSCursor.arrow.set()
                        #endif
                    }
                }
                .offset(x: screenX - hitDistance)
        }
    }

    private var sideBySideDivider: some View {
        Rectangle()
            .fill(Color.gray.opacity(0.5))
            .frame(width: 2, height: containerSize.height)
            .position(x: containerSize.width / 2, y: containerSize.height / 2)
            .allowsHitTesting(false)
    }

    private var sideBySideVerticalDivider: some View {
        Rectangle()
            .fill(Color.gray.opacity(0.5))
            .frame(width: containerSize.width, height: 2)
            .position(x: containerSize.width / 2, y: containerSize.height / 2)
            .allowsHitTesting(false)
    }
}
