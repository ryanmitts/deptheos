// The ObjectInfo dataset comes from the camera: its name length is not trusted, and its name
// becomes a file in the download folder.
import Foundation
import XCTest
@testable import stackrig

final class ObjectInfoTests: XCTestCase {
    /// An ObjectInfo with the size at 8 and the name string (length byte, UTF-16) at 52.
    private func info(size: UInt32, name: String, claimedLength: Int? = nil) -> Data {
        var d = Data(count: 8)
        d.append(u32: size)
        d.append(Data(count: 52 - d.count))
        let units = Array(name.utf16)
        d.append(UInt8(claimedLength ?? units.count))
        for u in units { d.append(u16: u) }
        return d
    }

    func testNameAndSizeAreRead() throws {
        let r = try parseObjectInfo(info(size: 1234, name: "IMG_0001.CR3\0"), handle: 1)
        XCTAssertEqual(r.name, "IMG_0001.CR3")
        XCTAssertEqual(r.size, 1234)
    }

    func testANameLongerThanTheDatasetIsCut() throws {
        let r = try parseObjectInfo(info(size: 1, name: "AB", claimedLength: 200), handle: 1)
        XCTAssertEqual(r.name, "AB")
    }

    func testOnlyTheNamesLastComponentIsUsed() throws {
        XCTAssertEqual(try parseObjectInfo(info(size: 1, name: "../../etc/x.JPG"), handle: 1).name, "x.JPG")
        XCTAssertEqual(try parseObjectInfo(info(size: 1, name: ".."), handle: 0xab).name, "IMG_ab.bin")
        XCTAssertEqual(try parseObjectInfo(info(size: 1, name: "/"), handle: 0xab).name, "IMG_ab.bin")
    }
}
