// The camera's replies are lists of [size][type][payload] records; a record that does not fit ends the list.
import Foundation
import XCTest
@testable import stackrig

final class RecordsTests: XCTestCase {
    private func record(_ type: UInt32, _ payload: [UInt8]) -> Data {
        var d = Data()
        d.append(u32: UInt32(8 + payload.count))
        d.append(u32: type)
        d.append(contentsOf: payload)
        return d
    }

    func testRecordsAreReadInOrder() {
        let reply = record(0xC189, [1, 2, 3, 4]) + record(1, []) + record(0xC181, [9])
        let records = reply.records()
        XCTAssertEqual(records.map(\.type), [0xC189, 1, 0xC181])
        XCTAssertEqual(records.map { [UInt8]($0.body) }, [[1, 2, 3, 4], [], [9]])
    }

    func testARecordThatDoesNotFitEndsTheList() {
        var cut = record(0xC189, [1, 2, 3, 4]) + record(1, [5, 6])
        cut.removeLast()
        XCTAssertEqual(cut.records().map(\.type), [0xC189])
        var tiny = Data()
        tiny.append(u32: 4)
        tiny.append(u32: 1)
        XCTAssertTrue(tiny.records().isEmpty)
    }

    func testASliceIsReadFromItsOwnStart() {
        let reply = Data([0xFF, 0xFF]) + record(1, [7])
        XCTAssertEqual(reply.dropFirst(2).records().map { [UInt8]($0.body) }, [[7]])
    }
}
