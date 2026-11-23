// A body that refuses the no-focus half press gets the plain one, on every retry.
import Foundation
import XCTest
@testable import stackrig

final class HalfPressTests: XCTestCase {
    /// Presses answered in turn by `answers`; returns the result and the parameters sent.
    private func run(_ answers: [UInt16], deadline: TimeInterval = 30) -> (rc: UInt16, sent: [[UInt32]], waits: Int) {
        var sent: [[UInt32]] = [], waits = 0, left = answers
        let rc = halfPress(until: Date(timeIntervalSinceNow: deadline), wait: { waits += 1 }) { params in
            sent.append(params)
            return left.removeFirst()
        }
        return (rc, sent, waits)
    }

    func testTheFallbackIsKeptOnABusyRetry() {
        let r = run([0x2006, PTP.busy, PTP.busy, PTP.ok])
        XCTAssertEqual(r.rc, PTP.ok)
        XCTAssertEqual(r.sent, [[1, 1], [1, 0], [1, 0], [1, 0]])
        XCTAssertEqual(r.waits, 2)
    }

    func testARefusalAfterBusyFallsBackToo() {
        let r = run([PTP.busy, 0x2006, PTP.ok])
        XCTAssertEqual(r.rc, PTP.ok)
        XCTAssertEqual(r.sent, [[1, 1], [1, 1], [1, 0]])
    }

    func testABodyThatTakesTheNoFocusPressKeepsIt() {
        let r = run([PTP.busy, PTP.ok])
        XCTAssertEqual(r.sent, [[1, 1], [1, 1]])
    }

    func testAPlainRefusalIsReturned() {
        let r = run([0x2006, 0x2006])
        XCTAssertEqual(r.rc, 0x2006)
        XCTAssertEqual(r.sent, [[1, 1], [1, 0]])
    }

    func testBusyPastTheDeadlineIsReturned() {
        let r = run([PTP.busy], deadline: -1)
        XCTAssertEqual(r.rc, PTP.busy)
        XCTAssertEqual(r.waits, 0)
    }
}
