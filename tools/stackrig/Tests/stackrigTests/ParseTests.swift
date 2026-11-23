// The command line is checked before anything connects: a bad option fails at once.
import XCTest
@testable import stackrig

final class ParseTests: XCTestCase {
    func testFocusTakesItsThreeValues() throws {
        XCTAssertEqual(try parse(["run"]).options.focus, .edge)
        XCTAssertEqual(try parse(["run", "--focus", "evf"]).options.focus, .evf)
        XCTAssertEqual(try parse(["run", "--focus", "none"]).options.focus, .none)
    }

    func testNumbersAreRead() throws {
        let o = try parse(["run", "--stacks", "3", "--step", "-0.5", "--shots", "40", "--step-width", "2", "--settle", "2", "--start", "7", "--chunk", "0.5"]).options
        XCTAssertEqual(o.stacks, 3)
        XCTAssertEqual(o.step, -0.5)
        XCTAssertEqual(o.shots, 40)
        XCTAssertEqual(o.stepWidth, 2)
        XCTAssertEqual(o.settle, 2)
        XCTAssertEqual(o.start, 7)
        XCTAssertEqual(o.chunkMB, 0.5)
    }

    func testAMalformedNumberFailsInParse() {
        for args in [["run", "--shots", "abc"], ["run", "--step", "half"], ["run", "--start", "1.5"], ["run", "--idle", "nan"], ["run", "--step-width", "x"]] {
            XCTAssertThrowsError(try parse(args), "\(args)") { error in
                guard case RigError.usage = error else { return XCTFail("expected a usage error, got \(error)") }
            }
        }
    }

    func testHelpIsACommandAnywhereOnTheLine() throws {
        XCTAssertEqual(try parse(["--help"]).command, "help")
        XCTAssertEqual(try parse(["-h"]).command, "help")
        XCTAssertEqual(try parse(["run", "--stacks", "3", "--help"]).command, "help")
        XCTAssertTrue(usage.contains("stackrig liveview"))
    }

    func testTheShortUsageListsEveryCommand() {
        XCTAssertThrowsError(try parse([])) { error in
            XCTAssertTrue("\(error)".contains("probe|focus|shoot|bracket|liveview|turn|run"), "\(error)")
        }
    }

    func testACameraLostMidRunIsNotATimeout() {
        XCTAssertEqual("\(RigError.cameraGone)", "camera: disconnected during the run")
        XCTAssertFalse("\(RigError.noCamera)".contains("EOS Utility"))
    }

    func testAnUnknownFocusFailsInParse() {
        XCTAssertThrowsError(try parse(["run", "--focus", "near"])) { error in
            guard case RigError.usage = error else { return XCTFail("expected a usage error, got \(error)") }
        }
    }
}
