// The camera's closing waits must outlast a Ctrl-C; its other waits must stop at one.
import Foundation
import XCTest
@testable import stackrig

final class WaitTests: XCTestCase {
    override func tearDown() { interrupted = false }

    func testCtrlCStopsAnInterruptibleWait() {
        interrupted = true
        XCTAssertThrowsError(try pumpUntil(5, "never") { false })
    }

    func testCtrlCDoesNotStopAClosingWait() throws {
        interrupted = true
        var turns = 0
        try pumpUntil(5, "ten turns", interruptible: false) { turns += 1; return turns >= 10 }
        XCTAssertEqual(turns, 10)
    }

    func testAClosingWaitStillTimesOut() {
        interrupted = true
        XCTAssertThrowsError(try pumpUntil(0.05, "never", interruptible: false) { false }) { error in
            guard case RigError.timeout = error else { return XCTFail("expected a timeout, got \(error)") }
        }
    }

    func testCtrlCDoesNotCutAClosingPumpShort() {
        interrupted = true
        let start = Date()
        pump(0.1, interruptible: false)
        XCTAssertGreaterThanOrEqual(Date().timeIntervalSince(start), 0.1)
    }
}
