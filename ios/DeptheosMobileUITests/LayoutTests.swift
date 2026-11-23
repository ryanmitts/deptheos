import XCTest

/// The viewer's controls hide, the photo zooms, and a tap brings the
/// controls back, with an inner camera faked (`-FakeOcclusion`) at the
/// viewer's top and down either side, as an iPad Pro reports it in
/// landscape. A layout that feeds its own reading never settles, and the
/// test fails to find the app idle (ADR-169). Run by `just ios-layout`.
final class LayoutTests: XCTestCase {
    override func setUp() {
        continueAfterFailure = false
    }

    @MainActor
    func testControlsComeBackWithoutCamera() throws {
        try hideZoomAndShow(camera: nil)
    }

    @MainActor
    func testControlsComeBackWithCameraAtTop() throws {
        try hideZoomAndShow { viewer in
            CGRect(x: viewer.midX - 90, y: viewer.minY, width: 180, height: 32)
        }
    }

    @MainActor
    func testControlsComeBackWithCameraDownLeadingSide() throws {
        try hideZoomAndShow { viewer in
            CGRect(x: viewer.minX, y: viewer.minY - 32, width: 32, height: viewer.height + 64)
        }
    }

    @MainActor
    func testControlsComeBackWithCameraDownTrailingSide() throws {
        try hideZoomAndShow { viewer in
            CGRect(x: viewer.maxX - 32, y: viewer.minY - 32, width: 32, height: viewer.height + 64)
        }
    }

    /// `camera` places the occlusion from the viewer's frame on screen.
    @MainActor
    private func hideZoomAndShow(camera: ((CGRect) -> CGRect)?) throws {
        var viewer: CGRect?
        if let camera {
            // A first launch finds where the viewer is.
            let app = launch([])
            viewer = try photo(in: app).frame
            app.terminate()
            let rect = camera(try XCTUnwrap(viewer))
            _ = launch(["-FakeOcclusion", "\(Int(rect.minX)),\(Int(rect.minY)),\(Int(rect.width)),\(Int(rect.height))"])
        }
        let app = XCUIApplication()
        let rail = app.descendants(matching: .any)["focus-rail"].firstMatch
        XCTAssert(rail.waitForExistence(timeout: 30), "the focus rail")
        let photo = try photo(in: app)
        let tap = photo.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.4))

        tap.tap()
        XCTAssert(rail.waitForNonExistence(timeout: 5), "the controls hide")
        photo.pinch(withScale: 3, velocity: 3)
        photo.pinch(withScale: 3, velocity: 3)
        tap.tap()
        XCTAssert(rail.waitForExistence(timeout: 5), "the controls come back")
        XCTAssertEqual(try self.photo(in: app).frame, viewer ?? photo.frame, "the viewer kept its place")
    }

    @MainActor
    private func launch(_ arguments: [String]) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments = ["-TrySample", "YES"] + arguments
        app.launch()
        // A forced end of an earlier run is reported once.
        let ok = app.alerts.buttons.firstMatch
        if ok.waitForExistence(timeout: 3) { ok.tap() }
        return app
    }

    /// The photo's scroll view: the widest under the full window.
    @MainActor
    private func photo(in app: XCUIApplication) throws -> XCUIElement {
        let rail = app.descendants(matching: .any)["focus-rail"].firstMatch
        XCTAssert(rail.waitForExistence(timeout: 30), "the focus rail")
        let window = app.windows.firstMatch.frame
        let photo = app.scrollViews.allElementsBoundByIndex
            .filter { $0.frame.width < window.width && $0.frame.contains(CGPoint(x: rail.frame.midX, y: rail.frame.midY)) }
            .max { $0.frame.width < $1.frame.width }
        return try XCTUnwrap(photo, "the photo")
    }
}
