// Shots sent to the computer exist nowhere else, so a rerun must not write over them.
import Foundation
import XCTest
@testable import stackrig

final class DownloadTests: XCTestCase {
    private var dir: URL!

    override func setUpWithError() throws {
        dir = FileManager.default.temporaryDirectory.appendingPathComponent("stackrig-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    }

    override func tearDownWithError() throws {
        try FileManager.default.removeItem(at: dir)
    }

    func testAStackFolderWithShotsIsRefused() throws {
        let s002 = dir.appendingPathComponent("s002")
        try FileManager.default.createDirectory(at: s002, withIntermediateDirectories: true)
        try Data([1]).write(to: s002.appendingPathComponent("0001.JPG"))
        XCTAssertThrowsError(try refuseShotFolders(dir, (1...3).map(stackFolder))) { error in
            XCTAssertTrue("\(error)".contains("s002"), "\(error)")
            XCTAssertTrue("\(error)".contains("--start"), "\(error)")
        }
        XCTAssertNoThrow(try refuseShotFolders(dir, (3...5).map(stackFolder)))
    }

    func testMissingEmptyOrOnlyHiddenFoldersAreFine() throws {
        let s001 = dir.appendingPathComponent("s001")
        try FileManager.default.createDirectory(at: s001, withIntermediateDirectories: true)
        try Data().write(to: s001.appendingPathComponent(".DS_Store"))
        XCTAssertNoThrow(try refuseShotFolders(dir, ["s001", "s002"]))
    }

    func testAShotFileIsNeverWrittenOver() throws {
        let file = dir.appendingPathComponent("0001.JPG")
        let fh = try createExclusively(file)
        fh.write(Data([1, 2, 3]))
        try fh.close()
        XCTAssertThrowsError(try createExclusively(file))
        XCTAssertEqual(try Data(contentsOf: file), Data([1, 2, 3]))
    }
}
