// The camera side: raw PTP to a Canon EOS body over USB through ImageCaptureCore. Every command is
// a PTP container sent with `requestSendPTPCommand`; the EOS codes are in `PTP`.
//
// Session: SetRemoteMode(1) + SetEventMode(1) once it opens, then GetEvent is polled for new files
// and property values. SetEventMode(0) and the session close hand the body back.
import AppKit
import CoreGraphics
import Foundation
import ImageCaptureCore
import ImageIO

enum RigError: Error, CustomStringConvertible {
    case ptp(String, UInt16)
    case transport(String)
    case noCamera
    /// The camera went away in the middle of a run.
    case cameraGone
    case timeout(String)
    case bluetooth(String)
    /// A shot's file on this Mac couldn't be created or written.
    case file(String)
    case usage(String)

    var description: String {
        switch self {
        case .ptp(let what, let rc): return "\(what): PTP response 0x\(String(rc, radix: 16))\(PTP.describe(rc))"
        case .transport(let what): return "camera: \(what)"
        case .noCamera: return "no camera found (USB connected, powered on, other tethering apps closed?)"
        case .cameraGone: return "camera: disconnected during the run"
        case .timeout(let what): return "timed out: \(what)"
        case .bluetooth(let what): return "Bluetooth: \(what)"
        case .file(let what): return what
        case .usage(let what): return what
        }
    }
}

/// Set by Ctrl-C. Interruptible waits (the default) stop when it is set, so the command unwinds
/// to the exit path; the camera's closing waits are not, so the body can still be put back.
var interrupted = false

/// Spin the main run loop for `seconds`; ImageCaptureCore delivers its completions there.
func pump(_ seconds: TimeInterval, interruptible: Bool = true) {
    let until = Date(timeIntervalSinceNow: seconds)
    repeat {
        RunLoop.main.run(mode: .default, before: Date(timeIntervalSinceNow: 0.02))
    } while Date() < until && !(interruptible && interrupted)
}

/// Pump until `done()` or the timeout; `what` is evaluated only when the timeout fires.
func pumpUntil(_ timeout: TimeInterval, _ what: @autoclosure () -> String, interruptible: Bool = true, _ done: () -> Bool) throws {
    let deadline = Date(timeIntervalSinceNow: timeout)
    // One run-loop turn at a time, so an arrived reply is seen at once.
    while !done() {
        if interruptible && interrupted { throw RigError.usage("interrupted") }
        if Date() > deadline { throw RigError.timeout(what()) }
        RunLoop.main.run(mode: .default, before: Date(timeIntervalSinceNow: 0.01))
    }
}

/// STACKRIG_DEBUG=1 prints every PTP response and event record.
let debug = ProcessInfo.processInfo.environment["STACKRIG_DEBUG"] != nil

/// The time at the head of each line: "14:02:31.207".
func timestamp() -> String {
    let f = DateFormatter(); f.dateFormat = "HH:mm:ss.SSS"
    return f.string(from: Date())
}

/// A line of the run's progress, on stdout.
func log(_ s: String) {
    print("\(timestamp())  \(s)")
    fflush(stdout)
}

/// Something that went wrong without stopping the command, on stderr.
func warn(_ s: String) {
    fflush(stdout)
    FileHandle.standardError.write(Data("\(timestamp())  warning: \(s)\n".utf8))
}

/// What stopped the command, on stderr.
func fail(_ s: String) {
    fflush(stdout)
    FileHandle.standardError.write(Data("error: \(s)\n".utf8))
}

/// A new file for a shot, failing if one is there: a shot sent to the computer exists nowhere else.
func createExclusively(_ url: URL) throws -> FileHandle {
    let fd = open(url.path, O_WRONLY | O_CREAT | O_EXCL, 0o644)
    guard fd >= 0 else {
        let why = errno == EEXIST ? "already exists; not writing over it" : String(cString: strerror(errno))
        throw RigError.file("can't create \(url.path): \(why)")
    }
    return FileHandle(fileDescriptor: fd, closeOnDealloc: true)
}

/// Name and size from a PTP ObjectInfo dataset. The name is the camera's, so it is cut to the
/// dataset's length and to its last path component: it becomes a file in the download folder.
func parseObjectInfo(_ d: Data, handle: UInt32) throws -> (name: String, size: Int) {
    guard d.count > 53 else { throw RigError.transport("the camera sent incomplete file info for object 0x\(String(handle, radix: 16))") }
    let size = Int(d.u32(8))
    let n = min(Int(d[d.startIndex + 52]), (d.count - 53) / 2)
    var name = ""
    for i in 0..<n {
        let c = d.u16(53 + 2 * i)
        if c != 0, let u = UnicodeScalar(c) { name.append(Character(u)) }
    }
    name = (name as NSString).lastPathComponent
    if name == "." || name == ".." || name == "/" { name = "" }
    return (name.isEmpty ? "IMG_\(String(handle, radix: 16)).bin" : name, size)
}

/// PTP and Canon EOS codes.
enum PTP {
    static let ok: UInt16 = 0x2001, busy: UInt16 = 0x2019
    // operations
    static let getNumObjects: UInt16 = 0x1006
    static let getObjectInfo: UInt16 = 0x1008
    static let getPartialObject: UInt16 = 0x101B
    static let setDevicePropValueEx: UInt16 = 0x9110
    static let setRemoteMode: UInt16 = 0x9114
    static let setEventMode: UInt16 = 0x9115
    static let getEvent: UInt16 = 0x9116
    static let transferComplete: UInt16 = 0x9117
    static let cancelTransfer: UInt16 = 0x9118
    static let pcHDDCapacity: UInt16 = 0x911A
    static let requestDevicePropValue: UInt16 = 0x9127
    static let keepDeviceOn: UInt16 = 0x911D
    static let remoteReleaseOn: UInt16 = 0x9128
    static let remoteReleaseOff: UInt16 = 0x9129
    static let terminateViewfinder: UInt16 = 0x9152
    static let getViewFinderData: UInt16 = 0x9153
    static let driveLens: UInt16 = 0x9155
    static let driveFocusToEdge: UInt16 = 0x9201
    // events (records in the GetEvent reply)
    static let objectAddedEx: UInt32 = 0xC181
    static let requestObjectTransfer: UInt32 = 0xC186
    static let propValueChanged: UInt32 = 0xC189
    static let willSoonShutdown: UInt32 = 0xC18D
    static let objectAddedEx64: UInt32 = 0xC1A7
    static let requestObjectTransfer64: UInt32 = 0xC1A9
    static let requestObjectTransfer64LFN: UInt32 = 0xC1B8
    /// The R5 Mark II's new-file event (75 bytes: handle, storage, format…).
    static let objectAddedR5m2: UInt32 = 0xC1B6
    // properties
    /// Where the body writes a shot: 1 = card, 4 = the computer.
    static let captureDestination: UInt32 = 0xD11C
    static let destCard: UInt32 = 1, destComputer: UInt32 = 4
    static let evfOutputDevice: UInt32 = 0xD1B0
    static let evfMode: UInt32 = 0xD1B1
    static let focusShiftSetting: UInt32 = 0xD208

    static func describe(_ rc: UInt16) -> String {
        switch rc {
        case 0x2002: return " (general error)"
        case 0x2005: return " (operation not supported)"
        case 0x2006: return " (parameter not supported)"
        case 0x2019: return " (device busy)"
        case 0x201D: return " (invalid parameter)"
        default: return ""
        }
    }
}

extension Data {
    func u16(_ at: Int) -> UInt16 { UInt16(self[startIndex + at]) | UInt16(self[startIndex + at + 1]) << 8 }
    func u32(_ at: Int) -> UInt32 { UInt32(u16(at)) | UInt32(u16(at + 2)) << 16 }
    mutating func append(u16 v: UInt16) { var x = v.littleEndian; append(Data(bytes: &x, count: 2)) }
    mutating func append(u32 v: UInt32) { var x = v.littleEndian; append(Data(bytes: &x, count: 4)) }

    /// The [size u32][type u32][payload] records a GetEvent or GetViewFinderData reply holds, up to
    /// the first one whose size does not fit.
    func records() -> [(type: UInt32, body: Data)] {
        var out: [(type: UInt32, body: Data)] = [], o = 0
        while o + 8 <= count {
            let size = Int(u32(o)), type = u32(o + 4)
            if size < 8 || o + size > count { break }
            out.append((type, subdata(in: (startIndex + o + 8)..<(startIndex + o + size))))
            o += size
        }
        return out
    }
}

/// The camera's focus bracketing settings (EOS property 0xD208, the SDK's EdsFocusShiftSet
/// behind a size word): size, version, function on/off, shots, step width, exposure smoothing,
/// in-camera composite, trimming, flash interval. The R5 Mark II reports 36 bytes, version 3.
struct FocusShift {
    var raw: [UInt32]
    var version: UInt32 { raw[1] }
    var focusShiftFunction: UInt32 { raw[2] }
    var shootingNumber: UInt32 { raw[3] }
    var stepWidth: UInt32 { raw[4] }
    var exposureSmoothing: UInt32 { raw[5] }
    var focusStackingFunction: UInt32 { raw[6] }

    init?(_ d: Data) {
        guard d.count >= 28, d.u32(0) == UInt32(d.count) else { return nil }
        raw = (0..<(d.count / 4)).map { d.u32($0 * 4) }
    }
}

/// The half press through `press` (RemoteReleaseOn's parameters in, response code out): (1, 1)
/// asks the body not to focus; a body that refuses it (0x2006) gets (1, 0) on every retry. A busy
/// body (still writing the last bracket) is asked again after each `wait` until `deadline`.
/// Returns the last response code.
func halfPress(until deadline: Date, wait: () throws -> Void, _ press: ([UInt32]) throws -> UInt16) rethrows -> UInt16 {
    var params: [UInt32] = [1, 1]
    var rc = try press(params)
    while true {
        if rc == 0x2006 && params == [1, 1] {
            params = [1, 0]
        } else if rc == PTP.busy && Date() < deadline {
            try wait()
        } else {
            return rc
        }
        rc = try press(params)
    }
}

/// The files written since the session opened, and which wait to be copied. Two sources report a
/// file (the event queue and ImageCaptureCore's notifications), in either order; a file is queued
/// the first time a source that can copy it reports it.
struct ShotFiles {
    /// Every file reported, by object handle.
    private(set) var seen = Set<UInt32>()
    /// Every file ever queued, so a second report doesn't queue it again.
    private var queued = Set<UInt32>()
    /// Queued and not yet taken, in the order reported.
    private(set) var pending: [UInt32] = []

    var count: Int { seen.count }

    /// A file reported; with `download`, queued to copy unless it was queued before.
    mutating func note(_ handle: UInt32, download: Bool) {
        seen.insert(handle)
        if download && queued.insert(handle).inserted { pending.append(handle) }
    }

    /// The next file to copy, taken off the queue.
    mutating func next() -> UInt32? { pending.isEmpty ? nil : pending.removeFirst() }
}

final class Camera: NSObject, ICDeviceBrowserDelegate, ICCameraDeviceDelegate {
    /// Where the ImageCaptureCore session is. Commands go out while it is open or closing.
    ///
    /// Lifecycle: searching, opening, open, closing, closed; or failed to open, removed or
    /// disconnected. Changed only by `Camera`'s `open`, `close` and ImageCaptureCore delegate
    /// callbacks, each change logged under `STACKRIG_DEBUG`.
    ///
    /// - `searching` → `opening` once a camera is found (`open`).
    /// - `opening` → `open(.card)`, or `openFailed` with ImageCaptureCore's reason.
    /// - `open(.card)` → `open(.computer)` when the destination is the computer (`open`).
    /// - `open` → `closing` (`close`) → `closed`, when the camera answers or the close gives up.
    /// - `removed`: gone while no session was open; `disconnected`: gone while open or closing.
    ///   Both are final.
    enum Session: Equatable { case searching, opening, openFailed(String), open(Shots), closing(Shots), closed, removed, disconnected }

    /// Where the body sends its shots while the session is open: to its card, or to the computer
    /// only, with the capture destination to put back at close.
    enum Shots: Equatable { case card, computer(restore: UInt32) }

    /// Where each shot goes; set before `open`.
    enum Destination {
        /// Left on the card.
        case card
        /// Written to the card, then copied from there into the folder.
        case cardAndCopy(URL)
        /// Sent to the computer only, into the folder; the capture destination is put back at close.
        case computer(URL)

        /// The folder shots are copied into, if any.
        var folder: URL? {
            switch self {
            case .card: return nil
            case .cardAndCopy(let folder), .computer(let folder): return folder
            }
        }
    }

    private let browser = ICDeviceBrowser()
    private var device: ICCameraDevice?
    private(set) var session = Session.searching {
        didSet { if debug && session != oldValue { log("  camera session: \(oldValue) → \(session)") } }
    }
    /// Closing ignores Ctrl-C so the body is put back; this bounds it, so a silent camera can't
    /// hold up the exit.
    private static let closeBudget: TimeInterval = 30
    private var closeDeadline = Date.distantFuture
    /// Waits stop on Ctrl-C except while closing.
    private var interruptible: Bool { !isClosing }
    private var isClosing: Bool { if case .closing = session { return true }; return false }
    /// The shots of the session open or closing; nil while none is.
    private var shots: Shots? {
        switch session {
        case .open(let shots), .closing(let shots): return shots
        default: return nil
        }
    }
    /// Shots go to the computer in this session, not the card.
    private var shotsToComputer: Bool { if case .computer = shots { return true }; return false }
    private var txid: UInt32 = 1
    private var liveView = false
    /// Last value of each EOS property the camera reported, by property code.
    private(set) var props: [UInt32: Data] = [:]
    /// Fed by GetEvent and ImageCaptureCore's notifications. Neither is complete: macOS's camera
    /// daemon drains the same event queue, so GetEvent misses some files, and its notifications lag.
    private var shotFiles = ShotFiles()
    private var filesCreated: Int { shotFiles.count }
    /// Files written by `shoot` calls in this session.
    private(set) var filesShot = 0
    var destination = Destination.card
    /// The subfolder of the destination's folder for the current stack.
    var downloadFolder = ""
    /// Where to copy each file as it is written (nil: leave it on the card).
    var downloadDir: URL? { destination.folder }
    /// Local paths of the files downloaded by the last `shoot`.
    private(set) var downloaded: [String] = []
    /// Bytes per GetPartialObject; each transaction has a fixed cost, so bigger is faster.
    var chunkSize: UInt32 = 1 << 20
    /// Files the body is holding in its buffer for us; each needs TransferComplete once copied.
    private var inCameraBuffer = Set<UInt32>()
    /// Size, format and name from a shot's transfer announcement, since GetObjectInfo fails
    /// (0x2002) for a shot still in the camera's buffer. After the 8-byte header: handle u32,
    /// format u32, u32, size u64, then the file name if any.
    private var bufferInfo: [UInt32: (name: String?, format: UInt32, size: Int, seq: Int)] = [:]
    /// Shots announced since the current `shoot` began, in capture order, so this numbers the
    /// files: 0001.JPG is the bracket's first shot.
    private var shotSeq = 0
    var name = ""

    /// Find the first PTP camera on USB, open the session and put the body in remote mode.
    func open() throws {
        browser.delegate = self
        browser.browsedDeviceTypeMask = ICDeviceTypeMask(rawValue: ICDeviceTypeMask.camera.rawValue | ICDeviceLocationTypeMask.local.rawValue)!
        warnAboutOtherCameraApps()
        browser.start()
        // A camera that was in a session a moment ago takes a few seconds to come back.
        do { try pumpUntil(20, "no camera") { self.device != nil } } catch { throw RigError.noCamera }
        let d = device!
        name = d.name ?? "camera"
        d.delegate = self
        session = .opening
        d.requestOpenSession()
        try pumpUntil(15, "opening the session") { self.session != .opening }
        if session == .removed { throw RigError.transport("the camera disappeared while the session was opening; another app (EOS Utility, a tethering app) may have taken it") }
        if case .openFailed(let reason) = session { throw RigError.transport("the session didn't open: \(reason)") }
        try send(PTP.setRemoteMode, [1], "SetRemoteMode")
        try send(PTP.setEventMode, [1], "SetEventMode")
        try events()
        let dest = props[PTP.captureDestination].flatMap { $0.count >= 4 ? $0.u32(0) : nil }
        if case .computer = destination {
            // The body checks the computer's free space before sending: report plenty, 4 KB clusters.
            try send(PTP.pcHDDCapacity, [0x0FFF_FFFF, 0x1000, 1], "PCHDDCapacity")
            try setProp(PTP.captureDestination, u32: PTP.destComputer, "capture destination")
            // The card, if the computer was left over from a session that could not put it back.
            let restore = dest.flatMap { $0 == PTP.destComputer ? nil : $0 } ?? PTP.destCard
            session = .open(.computer(restore: restore))
            log("camera: shots go to the computer only, not the card")
        } else if dest == PTP.destComputer {
            // Left over from a session that could not put the card back.
            log("camera: capture destination was left on the computer; setting it back to the card")
            restoreDestination(PTP.destCard)
        }
    }

    deinit { close() }

    /// Set the capture destination, retrying for 15 s while the body is busy (it is while it
    /// still holds shots for the computer).
    private func restoreDestination(_ value: UInt32) {
        var d = Data(); d.append(u32: value)
        let deadline = Date(timeIntervalSinceNow: 15)
        var rc: UInt16 = 0
        repeat {
            rc = (try? transact(PTP.setDevicePropValueEx, [], data: propData(PTP.captureDestination, d)).rc) ?? 0
            if rc != PTP.busy { break }
            pump(0.5, interruptible: interruptible); try? events()
        } while Date() < deadline
        if rc == PTP.ok { log("camera: capture destination back to the card") }
        else { warn("the capture destination couldn't be set back to the card (0x\(String(rc, radix: 16))); check the camera before shooting by hand") }
    }

    /// Another tethering app on the same macOS daemon drains the camera's event queue, so files are
    /// missed. EOS Utility is harmless.
    private func warnAboutOtherCameraApps() {
        log("camera: other tethering apps must be closed (they take some of the camera's file events); EOS Utility can stay open")
    }

    /// Leave remote mode and close the session. Safe to call twice.
    func close() {
        guard let d = device, case .open(let shots) = session else { return }
        session = .closing(shots)
        closeDeadline = Date(timeIntervalSinceNow: Camera.closeBudget)
        if liveView { try? setLiveView(false) }
        if case .computer(let restore) = shots {
            // Release anything still held in the camera's buffer, then put the card back.
            for h in inCameraBuffer { _ = try? send(PTP.cancelTransfer, [h], "CancelTransfer") }
            switch inCameraBuffer.count {
            case 0: break
            case 1: warn("camera: 1 shot was never copied and is lost")
            case let n: warn("camera: \(n) shots were never copied and are lost")
            }
            inCameraBuffer = []
            restoreDestination(restore)
        }
        _ = try? send(PTP.setEventMode, [0], "SetEventMode")
        if isClosing {
            d.requestCloseSession()
            try? pumpUntil(min(5, closeDeadline.timeIntervalSinceNow), "closing the session", interruptible: false) { !self.isClosing }
        }
        // A session that never answered the close is given up on.
        if isClosing { session = .closed }
        device = nil
        log("camera session closed")
    }

    // MARK: transport

    /// One PTP transaction: response code, data phase and response parameters. A camera that goes
    /// away before answering throws `cameraGone`, not a timeout.
    @discardableResult
    func transact(_ code: UInt16, _ params: [UInt32] = [], data: Data? = nil) throws -> (rc: UInt16, data: Data, params: [UInt32]) {
        guard let d = device, shots != nil else {
            throw session == .disconnected ? RigError.cameraGone : RigError.transport("no session")
        }
        var cmd = Data()
        cmd.append(u32: UInt32(12 + 4 * params.count)); cmd.append(u16: 1); cmd.append(u16: code); cmd.append(u32: txid)
        txid += 1
        for p in params { cmd.append(u32: p) }
        var result: (Data, Data, Error?)?
        d.requestSendPTPCommand(cmd, outData: data) { a, b, e in result = (a, b, e) }
        let wait = isClosing ? min(30, closeDeadline.timeIntervalSinceNow) : 30
        try pumpUntil(wait, "PTP 0x\(String(code, radix: 16)) reply", interruptible: interruptible) { result != nil || self.session == .disconnected }
        if session == .disconnected { throw RigError.cameraGone }
        let (a, b, e) = result!
        if let e { throw RigError.transport("PTP 0x\(String(code, radix: 16)): \(e.localizedDescription)") }
        // The completion's two buffers are the data phase and the response container (type 3,
        // code at offset 6), in no fixed order.
        let (payload, response) = b.count >= 12 && b.u16(4) == 3 ? (a, b) : (b, a)
        let rc = response.count >= 8 ? response.u16(6) : PTP.ok
        let rparams = response.count >= 12 ? (0..<((response.count - 12) / 4)).map { response.u32(12 + 4 * $0) } : []
        if debug && (code != PTP.getEvent || rc != PTP.ok || payload.count > 8) { log("  ptp 0x\(String(code, radix: 16)) \(params) → 0x\(String(rc, radix: 16)) \(rparams), \(payload.count) bytes") }
        return (rc, payload, rparams)
    }

    /// A transaction that must succeed.
    @discardableResult
    func send(_ code: UInt16, _ params: [UInt32] = [], _ what: String, data: Data? = nil) throws -> Data {
        let r = try transact(code, params, data: data)
        if r.rc != PTP.ok { throw RigError.ptp(what, r.rc) }
        return r.data
    }

    /// Set an EOS property: SetDevicePropValueEx with a [size][code][value] data phase.
    func setProp(_ code: UInt32, _ value: Data, _ what: String) throws {
        let r = try transact(PTP.setDevicePropValueEx, [], data: propData(code, value))
        if r.rc != PTP.ok { throw RigError.ptp(what, r.rc) }
    }
    func setProp(_ code: UInt32, u32 v: UInt32, _ what: String) throws {
        var d = Data(); d.append(u32: v); try setProp(code, d, what)
    }
    private func propData(_ code: UInt32, _ value: Data) -> Data {
        var d = Data(); d.append(u32: UInt32(8 + value.count)); d.append(u32: code); d.append(value); return d
    }

    /// Drain the camera's event queue: count files, remember property values, answer shutdown
    /// warnings and transfer requests.
    func events() throws {
        let r = try transact(PTP.getEvent)
        if r.rc != PTP.ok && debug { log("  GetEvent answered 0x\(String(r.rc, radix: 16)) with \(r.data.count) bytes") }
        for (type, body) in r.data.records() {
            if debug && type != PTP.propValueChanged {
                log("  event 0x\(String(type, radix: 16)) \(body.count + 8) bytes: \(body.prefix(64).map { String(format: "%02x", $0) }.joined(separator: " "))")
            }
            switch type {
            case PTP.objectAddedEx, PTP.objectAddedEx64, PTP.objectAddedR5m2,
                 PTP.requestObjectTransfer, PTP.requestObjectTransfer64, PTP.requestObjectTransfer64LFN:
                guard body.count >= 4 else { break }
                let h = body.u32(0)
                let isRequest = type != PTP.objectAddedEx && type != PTP.objectAddedEx64 && type != PTP.objectAddedR5m2
                if shotsToComputer {
                    // Every new-file event is a transfer request (bodies differ in the code),
                    // and an unconfirmed file stays in the camera's buffer: copy it, then
                    // TransferComplete frees it.
                    if inCameraBuffer.insert(h).inserted {
                        if body.count >= 20 {
                            let size = Int(body.u32(12)) | Int(body.u32(16)) << 32
                            var name: String?
                            if body.count > 20 {
                                let bytes = body.subdata(in: (body.startIndex + 20)..<body.endIndex).prefix { $0 != 0 }
                                if let n = String(bytes: bytes, encoding: .ascii), n.contains("."),
                                   n.allSatisfy({ $0.isLetter || $0.isNumber || "._-".contains($0) }) { name = n }
                            }
                            shotSeq += 1
                            bufferInfo[h] = (name, body.u32(4), size, shotSeq)
                        }
                        noteFile(h)
                    }
                } else {
                    noteFile(h)
                    if isRequest { _ = try? send(PTP.cancelTransfer, [h], "CancelTransfer") }
                }
            case PTP.propValueChanged:
                if body.count >= 4 {
                    let code = body.u32(0), value = body.subdata(in: (body.startIndex + 4)..<body.endIndex)
                    if debug, let old = props[code], old != value {
                        log("  prop 0x\(String(code, radix: 16)) → \(value.prefix(16).map { String(format: "%02x", $0) }.joined(separator: " "))")
                    }
                    props[code] = value
                }
            case PTP.willSoonShutdown:
                keepAwake()
            default: break
            }
        }
    }

    private func noteFile(_ handle: UInt32) {
        shotFiles.note(handle, download: downloadDir != nil)
    }

    // MARK: downloading

    /// Name and size from the standard PTP ObjectInfo dataset.
    func objectInfo(_ handle: UInt32) throws -> (name: String, size: Int) {
        try parseObjectInfo(try send(PTP.getObjectInfo, [handle], "GetObjectInfo"), handle: handle)
    }

    /// Copy one file into the destination's folder, `chunkSize` bytes at a time: off the card (where
    /// it stays) or, with shots going to the computer, out of the camera's buffer.
    func download(_ handle: UInt32) throws -> String {
        guard let dir = downloadDir else { throw RigError.usage("no download folder") }
        let info: (name: String, size: Int)
        if let b = bufferInfo[handle] {
            let announced = b.name.map { ($0 as NSString).pathExtension }.flatMap { $0.isEmpty ? nil : $0.uppercased() }
            let ext = announced ?? (b.format == 0x3801 ? "JPG" : b.format == 0xB108 ? "CR3" : "bin")
            info = (String(format: "%04d.%@", b.seq, ext), b.size)
        } else {
            info = try objectInfo(handle)
        }
        let folder = dir.appendingPathComponent(downloadFolder)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        let path = folder.appendingPathComponent(info.name)
        let fh = try createExclusively(path)
        defer { try? fh.close() }
        var got = 0
        let chunk = chunkSize
        // With no size known (0), read until a short piece.
        while info.size == 0 || got < info.size {
            let r = try transact(PTP.getPartialObject, [handle, UInt32(got), chunk])
            if r.rc != PTP.ok { throw RigError.ptp("GetPartialObject \(info.name) at \(got)", r.rc) }
            if r.data.isEmpty {
                if info.size == 0 && got > 0 { break }
                throw RigError.transport("\(info.name): empty piece at \(got) of \(info.size)")
            }
            do {
                try fh.write(contentsOf: r.data)
            } catch {
                throw RigError.file("can't write \(path.path): \(error.localizedDescription)")
            }
            got += r.data.count
            if info.size == 0 && r.data.count < Int(chunk) { break }
        }
        bufferInfo[handle] = nil
        return path.path
    }

    /// Files noticed but not yet copied.
    var pendingDownloads: Int { shotFiles.pending.count }

    /// Download up to `limit` of the files written since the last call. Returns the seconds spent.
    @discardableResult
    func downloadPending(limit: Int = .max) throws -> TimeInterval {
        guard downloadDir != nil, pendingDownloads > 0 else { return 0 }
        let start = Date()
        var n = 0
        while pendingDownloads > 0 && n < limit {
            if interrupted { throw RigError.usage("interrupted") }
            guard let h = shotFiles.next() else { break }
            let path = try download(h)
            if inCameraBuffer.remove(h) != nil {
                _ = try? send(PTP.transferComplete, [h], "TransferComplete")
            }
            downloaded.append(path)
            n += 1
            if debug { log("  downloaded \(path)") }
            if limit == .max && downloaded.count % 25 == 0 { log("  copied \(downloaded.count), \(pendingDownloads) to go") }
        }
        return Date().timeIntervalSince(start)
    }

    // MARK: operations

    /// Number of files on all the camera's cards (standard PTP GetNumObjects).
    func numObjects() throws -> Int {
        let r = try transact(PTP.getNumObjects, [0xFFFF_FFFF, 0, 0])
        guard r.rc == PTP.ok, let n = r.params.first else { throw RigError.ptp("GetNumObjects", r.rc) }
        return Int(n)
    }

    func focusShift() throws -> FocusShift {
        // Ask for a fresh value; the camera answers with a PropValueChanged record.
        _ = try? transact(PTP.requestDevicePropValue, [PTP.focusShiftSetting])
        pump(0.1)
        try events()
        guard let d = props[PTP.focusShiftSetting], let s = FocusShift(d) else {
            throw RigError.usage("the camera did not report its focus bracketing settings (property 0xD208)")
        }
        return s
    }

    /// Turn the camera's focus bracketing on (or off) and set the shot count and, optionally,
    /// the increment.
    func configureFocusShift(on: Bool = true, shots: Int?, step: Int?) throws {
        var s = try focusShift()
        s.raw[2] = on ? 1 : 0
        if let shots { s.raw[3] = UInt32(shots) }
        if let step { s.raw[4] = UInt32(step) }
        var d = Data(); for v in s.raw { d.append(u32: v) }
        try setProp(PTP.focusShiftSetting, d, "set focus bracketing")
        log("focus bracketing set: \(on ? "on" : "off")\(shots.map { ", \($0) shots" } ?? "")\(step.map { ", step \($0)" } ?? "")")
        pump(0.3); try events()
    }

    /// Drive the lens to an end stop with the body's own command (the SDK's DriveFocusToEdge);
    /// if the body rejects it, step there under live view instead.
    func focusToEdge(near: Bool, steps: Int = 0) throws {
        let r = try transact(PTP.driveFocusToEdge, [near ? 1 : 2])
        if r.rc == PTP.ok {
            log("focus: driving to the \(near ? "near" : "far") end")
            pump(4)
            try events()
            return
        }
        log("focus: DriveFocusToEdge answered 0x\(String(r.rc, radix: 16))\(PTP.describe(r.rc)); stepping under live view instead")
        try focusToNearByStepping(far: !near, steps: steps)
    }

    /// Live view on (output to the computer, which is what lets the lens be driven) or off.
    func setLiveView(_ on: Bool) throws {
        if on {
            var mode = Data(); mode.append(u16: 1)
            let r = try transact(PTP.setDevicePropValueEx, [], data: propData(PTP.evfMode, mode))
            if r.rc != PTP.ok && r.rc != PTP.busy { throw RigError.ptp("live view mode", r.rc) }
            try setProp(PTP.evfOutputDevice, u32: 2, "live view output to PC")
            liveView = true
            pump(0.5)
        } else {
            _ = try? setProp(PTP.evfOutputDevice, u32: 0, "live view off")
            _ = try? transact(PTP.terminateViewfinder)
            liveView = false
        }
        try events()
    }

    /// One live view frame as JPEG (live view must be on). The reply is a list of
    /// [size][type][payload] records; type 1 is the JPEG.
    func liveViewFrame() throws -> Data? {
        for _ in 0..<40 {
            let r = try transact(PTP.getViewFinderData, [0x00200000, 0, 0])
            if r.rc == PTP.busy || r.rc == 0xA102 { pump(0.1); continue }
            if r.rc != PTP.ok { throw RigError.ptp("live view frame", r.rc) }
            return r.data.records().first { $0.type == 1 }?.body
        }
        return nil
    }

    /// Step the lens to the near (or far) end with DriveLens in its largest steps. The body doesn't
    /// say when it reaches the stop, so with `steps` = 0 live view is compared every ten steps and
    /// the drive stops after two still comparisons in a row. Otherwise exactly `steps` are sent.
    func focusToNearByStepping(far: Bool = false, steps: Int = 0, maxSteps: Int = 600) throws {
        try setLiveView(true)
        defer { try? setLiveView(false) }
        let step: UInt32 = far ? 0x8003 : 3
        var n = 0, stillCount = 0, biggest = 0.0
        var last = steps == 0 ? try liveViewFrame().flatMap(thumbnail) : nil
        let start = Date()
        while n < (steps == 0 ? maxSteps : steps) {
            if interrupted { throw RigError.usage("interrupted") }
            let r = try transact(PTP.driveLens, [step])
            if r.rc == PTP.busy { pump(0.05); continue }
            if r.rc != PTP.ok { log("focus: DriveLens answered 0x\(String(r.rc, radix: 16))\(PTP.describe(r.rc)) after \(n == 1 ? "1 step" : "\(n) steps")"); break }
            n += 1
            pump(0.03)
            if steps == 0 && n % 10 == 0 {
                pump(0.2)
                if let t = try liveViewFrame().flatMap(thumbnail) {
                    let d = last.map { difference($0, t) } ?? 255
                    biggest = max(biggest, d)
                    if debug { log("  focus: step \(n), image change \(String(format: "%.2f", d))") }
                    // "Still" is relative to how much the picture moved earlier in this drive,
                    // with a floor for live view noise.
                    stillCount = d < max(0.5, 0.2 * biggest) ? stillCount + 1 : 0
                    last = t
                    if stillCount >= 2 { break }
                }
            }
        }
        try events()
        log("focus: \(n == 1 ? "1 step" : "\(n) steps") toward the \(far ? "far" : "near") end in \(String(format: "%.1f", Date().timeIntervalSince(start))) s\(steps == 0 && stillCount < 2 ? " (image still changing)" : "")")
    }

    /// A 48×32 gray version of a JPEG, for telling whether the picture changed.
    private func thumbnail(_ jpeg: Data) -> [UInt8]? {
        guard let src = CGImageSourceCreateWithData(jpeg as CFData, nil), let img = CGImageSourceCreateImageAtIndex(src, 0, nil) else { return nil }
        let w = 48, h = 32
        var px = [UInt8](repeating: 0, count: w * h)
        guard let ctx = CGContext(data: &px, width: w, height: h, bitsPerComponent: 8, bytesPerRow: w, space: CGColorSpaceCreateDeviceGray(), bitmapInfo: CGImageAlphaInfo.none.rawValue) else { return nil }
        ctx.interpolationQuality = .medium
        ctx.draw(img, in: CGRect(x: 0, y: 0, width: w, height: h))
        return px
    }

    /// Mean absolute difference between two thumbnails, 0–255.
    private func difference(_ a: [UInt8], _ b: [UInt8]) -> Double {
        guard a.count == b.count, !a.isEmpty else { return 255 }
        return Double(zip(a, b).reduce(0) { $0 + abs(Int($1.0) - Int($1.1)) }) / Double(a.count)
    }

    /// Press the shutter: half press without AF, full press, release. With focus bracketing on, the
    /// body runs the whole sequence; this waits until `expectedFiles` have been written.
    func shoot(expectedFiles: Int, timeout: TimeInterval) throws {
        try events()
        let before = try numObjects(), eventsBefore = filesCreated
        var files = 0
        downloaded = []
        shotSeq = 0
        var downloadTime: TimeInterval = 0
        // Sending to the computer, the body re-checks free space before each shot.
        if shotsToComputer {
            _ = try? send(PTP.pcHDDCapacity, [0x0FFF_FFFF, 0x1000, 1], "PCHDDCapacity")
            try events()
        }
        let half = try halfPress(until: Date(timeIntervalSinceNow: 30), wait: { pump(0.5); try events() }) {
            try transact(PTP.remoteReleaseOn, $0).rc
        }
        if half != PTP.ok { throw RigError.ptp("half press", half) }
        // The full press can come back busy just after the half press; hold it and retry for 10 s.
        var r = try transact(PTP.remoteReleaseOn, [2, 0])
        let fullDeadline = Date(timeIntervalSinceNow: 10)
        var busyTries = 0
        while r.rc == PTP.busy && Date() < fullDeadline {
            busyTries += 1
            pump(0.3); try events()
            r = try transact(PTP.remoteReleaseOn, [2, 0])
        }
        if busyTries > 0 { log("  full press was busy \(busyTries == 1 ? "once" : "\(busyTries) times")\(r.rc == PTP.ok ? ", then went through" : "")") }
        if r.rc != PTP.ok {
            _ = try? transact(PTP.remoteReleaseOff, [1])
            throw RigError.ptp("full press", r.rc)
        }
        _ = try? transact(PTP.remoteReleaseOff, [2])
        _ = try? transact(PTP.remoteReleaseOff, [1])
        log("shutter pressed, waiting for \(expectedFiles == 1 ? "1 file" : "\(expectedFiles) files")")
        // `timeout` is per file, not per bracket, which long exposures make as long as they take.
        var lastLogged = 0, lastFile = Date()
        try pumpUntil(.infinity, "") {
            // Rest between polls only when there is nothing to copy.
            if pendingDownloads == 0 { pump(0.3) }
            try? events()
            // Sending to the computer, the card's file count is a wasted round trip.
            let onCard = shotsToComputer ? 0 : ((try? numObjects()).map { $0 - before } ?? 0)
            let n = max(filesCreated - eventsBefore, onCard)
            if n > files { files = n; lastFile = Date() }
            if n / 25 > lastLogged / 25 { log("  \(n) files\(downloadDir != nil ? ", \(downloaded.count) copied" : "")"); lastLogged = n }
            // One copy per pass, so events are read between copies; copying time doesn't count
            // toward the idle limit.
            var copying: TimeInterval = 0
            // (Ctrl-C ends the wait through pumpUntil's own check.)
            do { copying = try downloadPending(limit: 1) } catch RigError.cameraGone {} catch { if !interrupted { warn("copy failed: \(error)") } }
            downloadTime += copying
            lastFile += copying
            if Date().timeIntervalSince(lastFile) > timeout { return true }
            return n >= expectedFiles || session == .disconnected
        }
        filesShot += files
        if session == .disconnected { throw RigError.cameraGone }
        if downloadDir != nil {
            // Downloading, files are counted through events only: wait for the last one's.
            pump(0.5); try events()
            if pendingDownloads > 0 { log("  camera done (\(files) files), copying the last \(pendingDownloads)") }
            downloadTime += try downloadPending()
            log("  downloaded \(downloaded.count == 1 ? "1 file" : "\(downloaded.count) files"), \(String(format: "%.1f", downloadTime)) s transferring, into \(downloadDir!.appendingPathComponent(downloadFolder).path)")
        }
        if files < expectedFiles {
            throw RigError.timeout("no file for \(Int(timeout)) s (\(files) of \(expectedFiles) arrived)")
        }
    }

    /// Reset the body's auto power-off timer.
    func keepAwake() {
        _ = try? transact(PTP.keepDeviceOn)
    }

    // MARK: ImageCaptureCore delegate

    func deviceBrowser(_ b: ICDeviceBrowser, didAdd d: ICDevice, moreComing: Bool) {
        guard device == nil, let c = d as? ICCameraDevice, c.capabilities.contains(ICDeviceCapability.cameraDeviceCanAcceptPTPCommands.rawValue) else { return }
        device = c
    }
    func deviceBrowser(_ b: ICDeviceBrowser, didRemove d: ICDevice, moreGoing: Bool) {}
    func didRemove(_ d: ICDevice) {
        if shots != nil {
            log("camera: disconnected")
            session = .disconnected
        } else if session != .disconnected {
            session = .removed
        }
    }
    func device(_ d: ICDevice, didOpenSessionWithError error: Error?) {
        guard session == .opening else { return }
        session = error.map { .openFailed($0.localizedDescription) } ?? .open(.card)
    }
    func device(_ d: ICDevice, didCloseSessionWithError error: Error?) {
        if session != .removed && session != .disconnected { session = .closed }
    }
    func deviceDidBecomeReady(_ d: ICDevice) {}
    func cameraDevice(_ c: ICCameraDevice, didAdd items: [ICCameraItem]) {
        for i in items where i.wasAddedAfterContentCatalogCompleted {
            if debug { log("  file \(i.name ?? "?") handle 0x\(String(i.ptpObjectHandle, radix: 16))") }
            // Sending to the computer, only the buffer event can copy a shot (it carries the size
            // and name), so this only counts it. On the card, it is queued here too.
            shotFiles.note(i.ptpObjectHandle, download: downloadDir != nil && !shotsToComputer)
        }
    }
    func cameraDevice(_ c: ICCameraDevice, didRemove items: [ICCameraItem]) {}
    func cameraDevice(_ c: ICCameraDevice, didReceiveThumbnail t: CGImage?, for i: ICCameraItem, error: Error?) {}
    func cameraDevice(_ c: ICCameraDevice, didReceiveMetadata m: [AnyHashable: Any]?, for i: ICCameraItem, error: Error?) {}
    func cameraDevice(_ c: ICCameraDevice, didRenameItems items: [ICCameraItem]) {}
    func cameraDeviceDidChangeCapability(_ c: ICCameraDevice) {}
    func cameraDevice(_ c: ICCameraDevice, didReceivePTPEvent e: Data) {}
    func deviceDidBecomeReady(withCompleteContentCatalog c: ICCameraDevice) {}
    func cameraDeviceDidRemoveAccessRestriction(_ c: ICDevice) {}
    func cameraDeviceDidEnableAccessRestriction(_ c: ICDevice) {}
}
