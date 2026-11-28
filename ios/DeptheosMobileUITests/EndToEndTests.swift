import XCTest

/// End to end on iOS: the app, driven as a user would, takes real frames
/// to exported files as an `e2e.toml` case says: the case's method and
/// settings, Stack, each export through the Export sheet, then, if the case
/// paints, a drag in Retouch and a TIFF export.
///
/// Run by `script/e2e-ios.sh CASE`, which hands over the case, frames and
/// an exports folder (`TEST_RUNNER_DEPTHEOS_E2E_CASE`, `…_FRAMES`,
/// `…_EXPORTS`) and checks the exports with `e2e-check`. The app reads
/// them through its debug-only hooks (`Model/E2E.swift`).
final class EndToEndTests: XCTestCase {
    /// The longest a stack of real frames, or an export, may take.
    private let long: TimeInterval = 600

    override func setUp() {
        continueAfterFailure = false
    }

    @MainActor
    func testStackExportRetouchExport() throws {
        let environment = ProcessInfo.processInfo.environment
        // Only under the script (`just e2e-ios`): ⌘U in Xcode skips it.
        guard let frames = environment["DEPTHEOS_E2E_FRAMES"] else {
            throw XCTSkip("end to end: run `just e2e-ios CASE`")
        }
        let exports = try XCTUnwrap(environment["DEPTHEOS_E2E_EXPORTS"], "the exports' folder")
        let json = try XCTUnwrap(environment["DEPTHEOS_E2E_CASE"], "the case")
        let testCase = try JSONDecoder().decode(Case.self, from: Data(json.utf8))
        let app = XCUIApplication()
        app.launchEnvironment["DEPTHEOS_E2E_FRAMES"] = frames
        app.launchEnvironment["DEPTHEOS_E2E_EXPORTS"] = exports
        app.launchEnvironment["DEPTHEOS_E2E_CASE"] = json
        app.launch()

        // The frames open as a new stack; Stack once they are listed.
        let stack = app.buttons["stack"].firstMatch
        waitUntilEnabled(stack, "Stack", timeout: long)
        stack.tap()
        // Stacked, and saved as render #1, under the stack's name.
        let first = app.staticTexts.matching(NSPredicate(format: "label == '#1'")).firstMatch
        XCTAssert(first.waitForExistence(timeout: long), "render #1")

        if testCase.adjust != nil || testCase.reframe != nil {
            adjust(in: app)
        }
        for export in testCase.exports {
            try exportAs(export, space: testCase.space, in: app)
        }
        guard testCase.paint else { return }
        // A retouch is checked against a plain TIFF: one made if none was asked for.
        if !testCase.exports.contains(where: { $0.hasPrefix("tiff") }) {
            try exportAs("tiff-8", space: testCase.space, in: app)
        }
        let paintedAs = testCase.exports.first { $0.hasPrefix("tiff") } ?? "tiff-8"

        // Retouch: one drag across the photo's middle.
        tapToolbar("retouch", "Retouch", in: app)
        let canvas = app.descendants(matching: .any)["paint-canvas"].firstMatch
        XCTAssert(canvas.waitForExistence(timeout: long), "Retouch's canvas")
        // Its tiles and the frame painted from load meanwhile.
        Thread.sleep(forTimeInterval: 5)
        let from = canvas.coordinate(withNormalizedOffset: CGVector(dx: 0.25, dy: 0.5))
        let to = canvas.coordinate(withNormalizedOffset: CGVector(dx: 0.75, dy: 0.5))
        // Straight into the drag: held first (0.35 s), a touch shows the
        // source instead.
        from.press(forDuration: 0.05, thenDragTo: to, withVelocity: .slow, thenHoldForDuration: 0)
        // One stroke: Undo comes on.
        waitUntilEnabled(app.buttons["Undo"].firstMatch, "Undo after the stroke", timeout: 60)

        try exportAs(paintedAs, space: testCase.space, in: app)
    }

    /// The viewer's loads as a user moves through the result: frames alone,
    /// a compare, Contribution and the lit depth map, each dragged through
    /// on the rail faster than loads land, then background and back. What
    /// lands each time is what is selected, at full size. Read through
    /// `E2EViewProbe`. Exports nothing.
    @MainActor
    func testViewLoads() throws {
        let environment = ProcessInfo.processInfo.environment
        guard let frames = environment["DEPTHEOS_E2E_FRAMES"] else {
            throw XCTSkip("end to end: run `just e2e-ios CASE`")
        }
        let json = try XCTUnwrap(environment["DEPTHEOS_E2E_CASE"], "the case")
        // Contribution and the depth map are a depth stack's.
        guard try JSONDecoder().decode(Case.self, from: Data(json.utf8)).method == "depth" else {
            throw XCTSkip("the view's loads: a depth stack's case")
        }
        let app = XCUIApplication()
        app.launchEnvironment["DEPTHEOS_E2E_FRAMES"] = frames
        app.launchEnvironment["DEPTHEOS_E2E_CASE"] = json
        app.launch()

        let stack = app.buttons["stack"].firstMatch
        waitUntilEnabled(stack, "Stack", timeout: long)
        stack.tap()
        let first = app.staticTexts.matching(NSPredicate(format: "label == '#1'")).firstMatch
        XCTAssert(first.waitForExistence(timeout: long), "render #1")

        let probe = app.descendants(matching: .any)["e2e-view"].firstMatch
        XCTAssert(probe.waitForExistence(timeout: 30), "the viewer's test element")
        let rail = app.descendants(matching: .any)["focus-rail"].firstMatch
        XCTAssert(rail.waitForExistence(timeout: 30), "the focus rail")

        // A frame alone, then a quick drag: the frame it stops on lands at
        // full size.
        scrub(rail, from: 0.05, to: 0.2)
        try waitForProbe(probe, "a frame alone at full size") { $0.frameAloneAtFullSize }
        let firstFrame = try read(probe)["selected"]
        scrub(rail, from: 0.2, to: 0.7)
        try waitForProbe(probe, "the frame dragged to, alone at full size") {
            $0.frameAloneAtFullSize && $0["selected"] != firstFrame
        }
        try toBackgroundAndBack(app)
        try waitForProbe(probe, "the frame's picture made again") { $0.frameAloneAtFullSize }

        // Compared with, in Split; then with Contribution, dragged through.
        try pickInViewMenu("Split", in: app)
        try waitForProbe(probe, "the frame compared with") { $0.compares("frame") }
        try pickInViewMenu("Contribution", in: app)
        try waitForProbe(probe, "the frame compared with, with Contribution") { $0.compares("contribution") }
        let compared = try read(probe)["selected"]
        scrub(rail, from: 0.7, to: 0.4)
        try waitForProbe(probe, "the frame dragged to, compared with Contribution") {
            $0.compares("contribution") && $0["selected"] != compared
        }
        try toBackgroundAndBack(app)
        try waitForProbe(probe, "the compared frame's picture made again") { $0.compares("contribution") }

        // The depth map lit for the frame compared with, then for the one
        // dragged to.
        try pickInViewMenu("Depth Map", in: app)
        try waitForProbe(probe, "the map lit for the frame") { $0.lightsAsked && $0["lighting"] == $0["selected"] }
        let lit = try read(probe)["lighting"]
        scrub(rail, from: 0.4, to: 0.8)
        try waitForProbe(probe, "the map lit for the frame dragged to") {
            $0.lightsAsked && $0["lighting"] != lit
        }
        try toBackgroundAndBack(app)
        try waitForProbe(probe, "the lit map's picture made again") { $0.lightsAsked }

        // The map off: the frame lit, compared with Contribution, as the
        // map came from a compare.
        try pickInViewMenu("Depth Map", in: app)
        try waitForProbe(probe, "the frame lit, compared with Contribution") { $0.compares("contribution") }
    }

    struct Probe {
        let fields: [String: String]

        subscript(_ name: String) -> String? { fields[name] }

        /// The frame selected shows alone, at its full size with its picture.
        var frameAloneAtFullSize: Bool {
            guard let selected = self["selected"], selected != "-" else { return false }
            return self["frame"] == selected && self["full"] == "loaded"
        }

        /// Compared with the frame selected as `kind` (`frame`,
        /// `contribution`), lined up and with its picture.
        func compares(_ kind: String) -> Bool {
            guard let selected = self["selected"], selected != "-" else { return false }
            return self["compare"] == "\(kind):\(selected):aligned:picture"
        }

        /// The depth map shows the lit frame's picture.
        var lightsAsked: Bool {
            guard let lighting = self["lighting"], lighting != "-" else { return false }
            return self["lit"] == "\(lighting):picture"
        }
    }

    @MainActor
    private func read(_ probe: XCUIElement) throws -> Probe {
        let value = try XCTUnwrap(probe.value as? String, "the viewer's test element's value")
        let pairs = value.split(separator: ";").compactMap { field -> (String, String)? in
            let parts = field.split(separator: "=", maxSplits: 1)
            return parts.count == 2 ? (String(parts[0]), String(parts[1])) : nil
        }
        return Probe(fields: Dictionary(pairs, uniquingKeysWith: { _, last in last }))
    }

    /// Wait until the viewer's test element reads as `done` says.
    @MainActor
    private func waitForProbe(
        _ probe: XCUIElement, _ what: String, timeout: TimeInterval = 60, _ done: (Probe) -> Bool
    ) throws {
        let deadline = Date.now.addingTimeInterval(timeout)
        var last = ""
        while Date.now < deadline {
            let read = try read(probe)
            if done(read) { return }
            last = (probe.value as? String) ?? ""
            Thread.sleep(forTimeInterval: 0.25)
        }
        XCTFail("\(what): the viewer has \(last)")
        throw XCTSkip("stopped: \(what)")
    }

    /// From `start` to `end` (fractions of the width), faster than loads
    /// land.
    @MainActor
    private func scrub(_ rail: XCUIElement, from start: CGFloat, to end: CGFloat) {
        let from = rail.coordinate(withNormalizedOffset: CGVector(dx: start, dy: 0.5))
        let to = rail.coordinate(withNormalizedOffset: CGVector(dx: end, dy: 0.5))
        from.press(forDuration: 0.05, thenDragTo: to, withVelocity: .fast, thenHoldForDuration: 0.1)
    }

    /// The view menu under the photo, then its item `title`.
    @MainActor
    private func pickInViewMenu(_ title: String, in app: XCUIApplication) throws {
        let menu = app.buttons.matching(NSPredicate(format: "label == 'View'")).firstMatch
        XCTAssert(menu.waitForExistence(timeout: 10), "the view menu")
        menu.tap()
        let item = app.buttons.matching(NSPredicate(format: "label == %@", title)).firstMatch
        guard item.waitForExistence(timeout: 10) else {
            add(XCTAttachment(string: app.debugDescription))
            XCTFail("\(title) isn't in the view menu")
            throw XCTSkip("stopped: \(title)")
        }
        item.tap()
    }

    /// To the background, where full-size pictures go, and back.
    @MainActor
    private func toBackgroundAndBack(_ app: XCUIApplication) throws {
        XCUIDevice.shared.press(.home)
        Thread.sleep(forTimeInterval: 3)
        app.activate()
        XCTAssert(app.wait(for: .runningForeground, timeout: 30), "back in front")
    }

    /// What the test reads of the case (`e2e-check case`).
    struct Case: Decodable {
        let method: String
        let exports: [String]
        let paint: Bool
        /// "display-p3": a camera RAW stack's output space, when set.
        let space: String?
        /// Set in Adjust by the app's hook (`E2EAdjustHook`).
        let adjust: [String: Double?]?
        let reframe: Present?

        /// Only whether it's there.
        struct Present: Decodable {}
    }

    /// Adjust: the case's sliders and reframe set by the app's hook as it
    /// opens (`E2EAdjustHook`), then Done once the render is saved under them.
    @MainActor
    private func adjust(in app: XCUIApplication) {
        tapToolbar("adjust", "Adjust", in: app)
        waitUntilEnabled(app.buttons["adjust-undo"].firstMatch, "Undo after the adjustments", timeout: long)
        // Settled and saved before Done.
        Thread.sleep(forTimeInterval: 5)
        let done = app.buttons["adjust-done"].firstMatch
        done.tap()
        XCTAssert(done.waitForNonExistence(timeout: long), "Adjust closed")
    }

    /// Export as `export` says ("jpeg", "png-8", "tiff-16", "dng"…): the
    /// format, the case's `space` but for DNG, 16 Bits where offered, Save
    /// to Test Folder; done when the sheet closes.
    @MainActor
    private func exportAs(_ export: String, space: String?, in app: XCUIApplication) throws {
        let parts = export.split(separator: "-")
        let labels = ["jpeg": "JPEG", "png": "PNG", "tiff": "TIFF", "dng": "DNG"]
        let format = try XCTUnwrap(labels[String(parts[0])], "export \(export)")
        let sixteen = parts.count > 1 && parts[1] == "16"
        tapToolbar("export", "Export…", in: app)
        let picker = app.buttons["export-format"].firstMatch
        XCTAssert(picker.waitForExistence(timeout: 30), "the Export sheet's Format")
        picker.tap()
        let item = app.buttons[format].firstMatch
        XCTAssert(item.waitForExistence(timeout: 10), "\(format) in Format")
        item.tap()
        // The space, before the bits: ProPhoto holds them at 16.
        if let space, format != "DNG" {
            let names = [
                "srgb": "sRGB", "display-p3": "Display P3", "adobe-rgb": "Adobe RGB", "prophoto": "ProPhoto RGB",
            ]
            let name = try XCTUnwrap(names[space], "space \(space)")
            // Shown once the sheet has read that the render is adjusted.
            let spaces = app.buttons["export-space"].firstMatch
            XCTAssert(spaces.waitForExistence(timeout: 30), "the Export sheet's Color")
            spaces.tap()
            let choice = app.buttons[name].firstMatch
            XCTAssert(choice.waitForExistence(timeout: 10), "\(name) in Color")
            choice.tap()
        }
        // 16 Bits, where the format and the stack leave it to choose.
        let bits = app.switches["export-16"].firstMatch
        if bits.waitForExistence(timeout: 2), bits.isEnabled {
            let on = (bits.value as? String) == "1"
            if on != sixteen {
                // The switch itself, at the row's end: the row's middle is its label.
                bits.coordinate(withNormalizedOffset: CGVector(dx: 0.95, dy: 0.5)).tap()
                XCTAssertEqual((bits.value as? String) == "1", sixteen, "16 Bits switched")
            }
        }
        let save = app.buttons["export-test-folder"].firstMatch
        waitUntilEnabled(save, "Save to Test Folder", timeout: 30)
        save.tap()
        XCTAssert(save.waitForNonExistence(timeout: long), "the \(format) export")
    }

    /// In the bar, or at iPhone width in the overflow menu, where Retouch
    /// and Export go first.
    @MainActor
    private func tapToolbar(_ id: String, _ title: String, in app: XCUIApplication) {
        let inBar = app.buttons[id].firstMatch
        if inBar.waitForExistence(timeout: 3), inBar.isHittable {
            inBar.tap()
            return
        }
        let more = app.navigationBars.buttons["More"].firstMatch
        guard more.waitForExistence(timeout: 10) else {
            add(XCTAttachment(string: app.debugDescription))
            XCTFail("\(title) is in neither the toolbar nor its overflow menu")
            return
        }
        more.tap()
        let item = app.buttons[title].firstMatch
        guard item.waitForExistence(timeout: 10) else {
            add(XCTAttachment(string: app.debugDescription))
            XCTFail("\(title) isn't in the overflow menu")
            return
        }
        waitUntilEnabled(item, title, timeout: 30)
        item.tap()
    }

    @MainActor
    private func waitUntilEnabled(_ element: XCUIElement, _ what: String, timeout: TimeInterval) {
        let enabled = expectation(
            for: NSPredicate(format: "exists == true AND isEnabled == true"), evaluatedWith: element)
        enabled.expectationDescription = what
        wait(for: [enabled], timeout: timeout)
    }
}
