// The turntable side: a Revopoint dual-axis turntable over Bluetooth LE. The table offers GATT
// service FFE0 with one characteristic, FFE1. A command is ASCII text written to it, such as
// "+CT,TURNANGLE=0.5;" to turn half a degree, and the table's reply is read back from the same
// characteristic ("+DATA=12;" for "+QT,CHANGEANGLE;", the angle turned since power-on).
import CoreBluetooth
import Foundation

final class Turntable: NSObject, CBCentralManagerDelegate, CBPeripheralDelegate {
    private let serviceUUID = CBUUID(string: "FFE0")
    private let charUUID = CBUUID(string: "FFE1")
    private var central: CBCentralManager!
    private var lastValue: Data?
    private var writeDone = false
    private var seen: [String] = []
    private var namePrefix: String?

    /// The Bluetooth link to the table.
    ///
    /// Lifecycle: off, on, scanning, connecting, ready, or lost; changed only by `connect()` and
    /// the CoreBluetooth delegate callbacks below, each change logged under `STACKRIG_DEBUG`.
    ///
    ///     off ──powered on──▶ on ──connect()──▶ scanning ──found──▶ connecting ──FFE1 found──▶ ready
    ///                                              ▲                    │
    ///                                              └──connect failed────┘
    ///     any state but off ──the table dropped, or Bluetooth went off──▶ lost
    ///
    /// Commands go out only in `ready`. `lost` is final, so a run stops instead of shooting more
    /// stacks at one angle.
    private enum Link {
        case off, on, scanning
        case connecting(CBPeripheral)
        case ready(CBPeripheral, CBCharacteristic)
        case lost(String)
    }
    private var link = Link.off {
        didSet { if debug { log("  turntable link: \(name(oldValue)) → \(name(link))") } }
    }

    private func name(_ link: Link) -> String {
        switch link {
        case .off: "off"
        case .on: "on"
        case .scanning: "scanning"
        case .connecting: "connecting"
        case .ready: "ready"
        case .lost(let why): "lost (\(why))"
        }
    }

    private var isReady: Bool { if case .ready = link { true } else { false } }
    private var lostReason: String? { if case .lost(let why) = link { why } else { nil } }

    /// Connect to the first device advertising FFE0, or the first whose name starts with
    /// `namePrefix`. Prints every device seen while scanning so a hidden one is easy to name.
    func connect(namePrefix: String? = nil, timeout: TimeInterval = 20) throws {
        self.namePrefix = namePrefix
        central = CBCentralManager(delegate: self, queue: nil)
        try pumpUntil(5, "waiting for Bluetooth to switch on") { if case .on = self.link { true } else { false } }
        log("scanning for the turntable\(namePrefix.map { " whose name starts with \($0)" } ?? "")…")
        link = .scanning
        central.scanForPeripherals(withServices: nil, options: [CBCentralManagerScanOptionAllowDuplicatesKey: false])
        do {
            try pumpUntil(timeout, self.notFound()) { self.isReady || self.lostReason != nil }
        } catch {
            central.stopScan()
            throw error
        }
        guard case .ready(let p, _) = link else { throw RigError.bluetooth(lostReason ?? "not connected") }
        log("turntable connected: \(p.name ?? "?")")
    }

    /// Why the scan gave up, with the devices it saw.
    func notFound() -> String {
        seen.isEmpty
            ? "turntable not found; no Bluetooth devices seen"
            : "turntable not found; seen: \(seen.joined(separator: ", "))"
    }

    func centralManagerDidUpdateState(_ c: CBCentralManager) {
        if c.state == .unauthorized { warn("Bluetooth: access denied; allow it for the terminal app in System Settings ▸ Privacy & Security ▸ Bluetooth") }
        switch (c.state == .poweredOn, link) {
        case (true, .off): link = .on
        case (false, .off), (true, _): break
        // Bluetooth went away under a scan or a connection.
        case (false, _): link = .lost("switched off")
        }
    }

    func centralManager(_ c: CBCentralManager, didDisconnectPeripheral p: CBPeripheral, error: Error?) {
        guard lostReason == nil else { return }
        log("turntable disconnected: \(error?.localizedDescription ?? "it closed the connection")")
        link = .lost("the turntable disconnected")
    }

    func centralManager(_ c: CBCentralManager, didDiscover p: CBPeripheral, advertisementData: [String: Any], rssi: NSNumber) {
        let name = p.name ?? (advertisementData[CBAdvertisementDataLocalNameKey] as? String) ?? "unnamed device"
        if !seen.contains(name) { seen.append(name); log("  seen \(name)") }
        let services = advertisementData[CBAdvertisementDataServiceUUIDsKey] as? [CBUUID] ?? []
        // The Revopoint table advertises as REVO_DUAL_AXIS_TABLE without listing its service.
        let match = services.contains(serviceUUID) || name.hasPrefix(namePrefix ?? "REVO")
        guard match, case .scanning = link else { return }
        link = .connecting(p)
        p.delegate = self
        c.stopScan()
        c.connect(p)
    }

    func centralManager(_ c: CBCentralManager, didConnect p: CBPeripheral) {
        p.discoverServices([serviceUUID])
    }

    func centralManager(_ c: CBCentralManager, didFailToConnect p: CBPeripheral, error: Error?) {
        warn("turntable: connection failed, scanning again: \(error?.localizedDescription ?? "no reason given")")
        guard case .connecting = link else { return }
        link = .scanning
        c.scanForPeripherals(withServices: nil)
    }

    func peripheral(_ p: CBPeripheral, didDiscoverServices error: Error?) {
        for s in p.services ?? [] where s.uuid == serviceUUID { p.discoverCharacteristics([charUUID], for: s) }
    }

    func peripheral(_ p: CBPeripheral, didDiscoverCharacteristicsFor s: CBService, error: Error?) {
        guard case .connecting = link, let ch = s.characteristics?.first(where: { $0.uuid == charUUID }) else { return }
        if ch.properties.contains(.notify) { p.setNotifyValue(true, for: ch) }
        link = .ready(p, ch)
    }

    func peripheral(_ p: CBPeripheral, didUpdateValueFor ch: CBCharacteristic, error: Error?) {
        lastValue = ch.value
    }

    func peripheral(_ p: CBPeripheral, didWriteValueFor ch: CBCharacteristic, error: Error?) {
        writeDone = true
    }

    /// Throws once the link is lost: nothing more reaches the table.
    private func checkLink() throws {
        if let why = lostReason { throw RigError.bluetooth(why) }
    }

    /// Write one command and return the reply text. Throws if the link is not ready, or is
    /// lost before the reply.
    @discardableResult
    func send(_ cmd: String, timeout: TimeInterval = 3) throws -> String {
        guard case .ready(let p, let ch) = link else { throw RigError.bluetooth(lostReason ?? "not connected") }
        lastValue = nil; writeDone = false
        let type: CBCharacteristicWriteType = ch.properties.contains(.write) ? .withResponse : .withoutResponse
        p.writeValue(Data(cmd.utf8), for: ch, type: type)
        if type == .withResponse { try pumpUntil(timeout, "write \(cmd)") { self.writeDone || !self.isReady } }
        pump(0.05)
        try checkLink()
        if lastValue == nil {
            p.readValue(for: ch)
            try? pumpUntil(timeout, "reply to \(cmd)") { self.lastValue != nil || !self.isReady }
            try checkLink()
        }
        return String(decoding: lastValue ?? Data(), as: UTF8.self)
    }

    /// The "+QT,CHANGEANGLE;" reply as a number: "+OK,TURNANGLE=1.01" (degrees since power-on) or
    /// "+DATA=0.0;"; a plain "+OK;" is the previous command's late acknowledgment. Throws only when
    /// the link is lost; a timeout is no answer.
    func changeAngle() throws -> Double? {
        let sent = try? send("+QT,CHANGEANGLE;")
        try checkLink()
        guard let r = sent else { return nil }
        if debug { log("  turntable CHANGEANGLE → \(r.isEmpty ? "(no reply)" : r)") }
        guard let eq = r.lastIndex(of: "=") else { return nil }
        let digits = r[r.index(after: eq)...].replacingOccurrences(of: ";", with: "")
        return Double(digits.trimmingCharacters(in: .whitespacesAndNewlines))
    }

    /// Turn by `degrees` (negative = the other way) and wait until the table's reported angle has
    /// been still for `settle` seconds. If the table never reports a number, waits `settle` plus
    /// the time a slow turn of that size takes.
    func turn(degrees: Double, settle: TimeInterval, timeout: TimeInterval = 60) throws {
        let cmd = "+CT,TURNANGLE=\(formatDegrees(degrees));"
        let reply = try send(cmd)
        log("turntable \(cmd) → \(reply.isEmpty ? "(no reply)" : reply)")
        let start = Date()
        var last: Double?, still = Date()
        var lost: Error?
        try pumpUntil(timeout, "turntable settling") {
            pump(0.3)
            let angle: Double?
            do { angle = try self.changeAngle() } catch {
                // The table is gone: where it stopped is unknown.
                lost = error
                return true
            }
            if let a = angle {
                if a != last { last = a; still = Date() }
            } else if last == nil && Date().timeIntervalSince(start) > 3 {
                log("turntable: no angle readback, waiting \(String(format: "%.1f", settle + abs(degrees) * 0.5)) s")
                pump(settle + abs(degrees) * 0.5)
                return true
            }
            return last != nil && Date().timeIntervalSince(still) >= settle && Date().timeIntervalSince(start) >= settle
        }
        if let lost { throw lost }
        if let last { log("turntable settled at \(last)° after \(String(format: "%.1f", Date().timeIntervalSince(start))) s") }
    }

    private func formatDegrees(_ d: Double) -> String {
        d == d.rounded() ? String(Int(d)) : String(format: "%g", d)
    }
}
