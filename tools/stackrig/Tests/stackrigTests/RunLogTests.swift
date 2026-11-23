// The run's CSV keeps earlier lines, and a resumed run logs the angles the first run would have.
import Foundation
import XCTest
@testable import stackrig

final class RunLogTests: XCTestCase {
    func testTheLogIsAppendedToWithOneHeader() throws {
        let path = FileManager.default.temporaryDirectory.appendingPathComponent("stackrig-\(UUID().uuidString).csv").path
        defer { try? FileManager.default.removeItem(atPath: path) }
        for line in ["1,0\n", "2,0.5\n"] {
            let file = try openRunLog(path)
            file.write(Data(line.utf8))
            try file.close()
        }
        XCTAssertEqual(try String(contentsOfFile: path, encoding: .utf8), "stack,angle_deg,start,end,files,folder\n1,0\n2,0.5\n")
    }

    func testAResumedStackIsAtItsOwnAngle() {
        XCTAssertEqual(stackAngle(1, step: 0.5), 0)
        XCTAssertEqual(stackAngle(5, step: 0.5), 2)
        XCTAssertEqual(stackAngle(3, step: -1), -2)
    }
}
