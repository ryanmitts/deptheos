// A file reported by both sources is copied once, whichever told of it first.
import XCTest
@testable import stackrig

final class ShotFilesTests: XCTestCase {
    func testAFileImageCaptureCoreToldOfFirstIsStillQueuedByItsEvent() {
        // Sending to the computer: the notification only counts it, the event queues it.
        var f = ShotFiles()
        f.note(7, download: false)
        f.note(7, download: true)
        XCTAssertEqual(f.count, 1)
        XCTAssertEqual(f.next(), 7)
        XCTAssertNil(f.next())
    }

    func testAFileBothSourcesQueueIsCopiedOnce() {
        // On the card and copied: either source queues it, the second report does nothing.
        var f = ShotFiles()
        f.note(1, download: true)
        f.note(2, download: true)
        f.note(1, download: true)
        XCTAssertEqual(f.count, 2)
        XCTAssertEqual(f.next(), 1)
        XCTAssertEqual(f.next(), 2)
        XCTAssertNil(f.next())
    }

    func testATakenFileReportedAgainIsNotQueuedAgain() {
        var f = ShotFiles()
        f.note(3, download: true)
        XCTAssertEqual(f.next(), 3)
        f.note(3, download: true)
        XCTAssertNil(f.next())
    }

    func testWithoutDownloadingFilesAreOnlyCounted() {
        var f = ShotFiles()
        f.note(4, download: false)
        f.note(5, download: false)
        XCTAssertEqual(f.count, 2)
        XCTAssertTrue(f.pending.isEmpty)
    }
}
