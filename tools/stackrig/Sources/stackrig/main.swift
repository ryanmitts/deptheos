// stackrig: focus to the near edge, shoot a focus bracket, turn the table, repeat. `usage` below
// lists the commands and options.
//
// Close other tethering apps while this runs: they take some of the camera's file events. `run`
// waits for Enter before opening the session so the shot can be framed, and the session is closed
// on every exit path, Ctrl-C included.
//
// Every run appends stack number, angle, times and file count to ./stackrig-<date>.csv.
import Foundation

/// What `stackrig --help` prints. Computed, like `commands`: a stored global in main.swift is
/// never set under the tests.
var usage: String { """
    stackrig: focus to the near edge, shoot a focus bracket, turn the table, repeat.

    usage:
      stackrig probe                    find the camera and the turntable, print what they report
      stackrig focus near|far|evf       drive the lens to an end stop (evf = step under live view)
      stackrig shoot [--shots N]        fire one bracket and count the files
      stackrig bracket on|off [--shots N] [--step-width W]
                                        write the focus bracketing settings to the camera
      stackrig liveview FILE.jpg [--near-steps N]
                                        save a live view frame; with N, step the lens N times toward
                                        the near end first (negative = far) to check that it moves
      stackrig turn DEG                 turn the table by DEG (0.5, -1, …) and wait for it
      stackrig run --stacks N --step DEG [options]

    run options (defaults in brackets):
      --shots N          files to expect per bracket [200]; with --configure, also set on the camera
      --step-width W     with --configure, the camera's focus increment, 1 to 10 [camera's current]
      --configure        write the focus bracketing settings to the camera before the first stack
      --focus edge|evf|none   how to reset focus: the body's drive-to-end command, falling back to
                         live-view stepping [edge]; live-view stepping only; or not at all
      --near-steps N     live-view steps (largest size) to reach the near end; 0 = step until the
                         live view image stops changing [0]
      --settle S         seconds the table must report no change before shooting [1.5]
      --idle S           give up on a bracket when no file has arrived for S seconds [60]
      --device PREFIX    pick the turntable by name prefix instead of its advertised service
      --no-turntable     camera only (for testing the bracket loop)
      --no-camera        turntable only (for testing the turn loop)
      --start K          first stack number, when resuming: its folder, and its angle in the log [1]
      --go               start without the "aim the camera, press Enter" prompt
      --download DIR     send each file to DIR/sNNN/ as it is shot instead of to the card (the
                         camera's capture destination is set to the computer, and put back after);
                         also works with `shoot`, which uses DIR/shoot/; a folder that already
                         has files is refused, so a rerun can't write over earlier shots
      --also-card        with --download, keep saving to the card and copy each file from there
      --chunk MB         copy in pieces of MB megabytes [1]; larger is faster if the body takes it
    """ }

/// The commands, as the short usage line lists them.
var commands: String { "probe|focus|shoot|bracket|liveview|turn|run" }

/// How `run` puts the focus back at the near end before each stack.
enum FocusReset: String {
    /// The body's drive-to-end command, falling back to live-view stepping.
    case edge
    /// Live-view stepping only.
    case evf
    /// Not at all.
    case none
}

struct Options {
    var stacks = 1
    var step = 0.5
    var shots = 200
    var stepWidth: Int?
    var configure = false
    var focus = FocusReset.edge
    var nearSteps = 0
    var settle = 1.5
    var idle = 60.0
    var device: String?
    var noTurntable = false
    var noCamera = false
    var start = 1
    var go = false
    var download: String?
    var alsoCard = false
    var chunkMB = 1.0
}

func parse(_ args: [String]) throws -> (command: String, positional: [String], options: Options) {
    var o = Options(), positional: [String] = [], i = 0
    func value(_ name: String) throws -> String {
        i += 1
        guard i < args.count else { throw RigError.usage("\(name) needs a value") }
        return args[i]
    }
    // A number that doesn't parse is an error, not a default: `--shots abc` must not shoot 0.
    func int(_ name: String) throws -> Int {
        let v = try value(name)
        guard let n = Int(v) else { throw RigError.usage("\(name) needs a whole number, not \(v)") }
        return n
    }
    func double(_ name: String) throws -> Double {
        let v = try value(name)
        guard let x = Double(v), x.isFinite else { throw RigError.usage("\(name) needs a number, not \(v)") }
        return x
    }
    while i < args.count {
        let a = args[i]
        switch a {
        // Help wins over everything else on the line, as it does for other tools.
        case "--help", "-h": return ("help", [], o)
        case "--stacks": o.stacks = try int(a)
        case "--step": o.step = try double(a)
        case "--shots": o.shots = try int(a)
        case "--step-width": o.stepWidth = try int(a)
        case "--configure": o.configure = true
        case "--focus":
            guard let focus = FocusReset(rawValue: try value(a)) else { throw RigError.usage("--focus must be edge, evf or none") }
            o.focus = focus
        case "--near-steps": o.nearSteps = try int(a)
        case "--settle": o.settle = try double(a)
        case "--idle": o.idle = try double(a)
        case "--device": o.device = try value(a)
        case "--no-turntable": o.noTurntable = true
        case "--no-camera": o.noCamera = true
        case "--start": o.start = try int(a)
        case "--go": o.go = true
        case "--download": o.download = try value(a)
        case "--also-card": o.alsoCard = true
        case "--chunk": o.chunkMB = try double(a)
        default:
            if a.hasPrefix("--") { throw RigError.usage("unknown option \(a)") }
            positional.append(a)
        }
        i += 1
    }
    guard let command = positional.first else {
        throw RigError.usage("usage: stackrig \(commands) …; stackrig --help lists the options")
    }
    return (command, Array(positional.dropFirst()), o)
}

/// The open camera, if any, so the exit path can close the session (exit() skips deinit).
var activeCamera: Camera?

func openCamera(_ o: Options) throws -> Camera {
    let cam = Camera()
    if let d = o.download { cam.destination = o.alsoCard ? .cardAndCopy(URL(fileURLWithPath: d)) : .computer(URL(fileURLWithPath: d)) }
    cam.chunkSize = UInt32(max(0.0625, min(o.chunkMB, 64)) * 1_048_576)
    try cam.open()
    log("camera: \(cam.name)")
    activeCamera = cam
    return cam
}

func closeCamera() {
    activeCamera?.close()
    activeCamera = nil
}

func waitForEnter(_ prompt: String) {
    print(prompt, terminator: ""); fflush(stdout)
    _ = readLine()
}

func openTurntable(_ o: Options) throws -> Turntable {
    let t = Turntable()
    try t.connect(namePrefix: o.device)
    return t
}

func resetFocus(_ cam: Camera, _ o: Options) throws {
    switch o.focus {
    case .edge: try cam.focusToEdge(near: true, steps: o.nearSteps)
    case .evf: try cam.focusToNearByStepping(steps: o.nearSteps)
    case .none: break
    }
}

func describeFocusShift(_ cam: Camera) {
    do {
        let s = try cam.focusShift()
        log("focus bracketing: \(s.focusShiftFunction == 1 ? "on" : "off"), \(s.shootingNumber) shots, step \(s.stepWidth), smoothing \(s.exposureSmoothing), composite \(s.focusStackingFunction) (settings v\(s.version), raw \(s.raw))")
    } catch {
        warn("focus bracketing settings couldn't be read: \(error)")
    }
}

/// A stack's folder under the download folder: s001, s002, …
func stackFolder(_ stack: Int) -> String { String(format: "s%03d", stack) }

/// Fails if any of `folders` under `dir` already holds files: a rerun would write over shots that,
/// sent to the computer, exist nowhere else. Hidden files (.DS_Store) don't count.
func refuseShotFolders(_ dir: URL, _ folders: [String]) throws {
    for folder in folders {
        let url = dir.appendingPathComponent(folder)
        let names = (try? FileManager.default.contentsOfDirectory(atPath: url.path)) ?? []
        if names.contains(where: { !$0.hasPrefix(".") }) {
            throw RigError.usage("\(url.path) already has files; move them, pick another --download folder, or resume after the last stack shot with --start")
        }
    }
}

/// The run's CSV, appended to so a resume in the same minute keeps the last run's lines.
func openRunLog(_ path: String) throws -> FileHandle {
    if !FileManager.default.fileExists(atPath: path) {
        try Data("stack,angle_deg,start,end,files,folder\n".utf8).write(to: URL(fileURLWithPath: path))
    }
    guard let file = FileHandle(forWritingAtPath: path) else { throw RigError.usage("can't write the log \(path)") }
    file.seekToEndOfFile()
    return file
}

/// The table's angle at a stack, counting from stack 1 at 0°, so a run resumed with `--start K`
/// logs the angles the first run would have.
func stackAngle(_ stack: Int, step: Double) -> Double { Double(stack - 1) * step }

func runLoop(_ o: Options) throws {
    guard o.stacks > 0, o.step != 0 || o.noTurntable else {
        throw RigError.usage("run needs --stacks above 0, and a --step other than 0 unless --no-turntable is given")
    }
    if let d = o.download, !o.noCamera {
        try refuseShotFolders(URL(fileURLWithPath: d), (o.start..<o.start + o.stacks).map(stackFolder))
    }
    let table = o.noTurntable ? nil : try openTurntable(o)
    if !o.noCamera && !o.go {
        waitForEnter("Frame and expose the shot now, then press Enter to connect the camera and begin… ")
    }
    let cam = o.noCamera ? nil : try openCamera(o)
    if let cam {
        describeFocusShift(cam)
        if o.configure {
            try cam.configureFocusShift(shots: o.shots, step: o.stepWidth)
            describeFocusShift(cam)
        }
    }
    let df = DateFormatter(); df.dateFormat = "yyyy-MM-dd-HHmm"
    let logPath = "stackrig-\(df.string(from: Date())).csv"
    let logFile = try openRunLog(logPath)
    let iso = ISO8601DateFormatter(); iso.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
    let timeout = o.idle

    let shots = o.shots == 1 ? "1 file" : "\(o.shots) files"
    if o.stacks == 1 {
        log("run: 1 stack of \(shots), log in \(logPath)")
    } else {
        log("run: \(o.stacks) stacks of \(shots), \(o.step)° between, log in \(logPath)")
    }
    for i in 0..<o.stacks {
        let stack = o.start + i
        let angle = stackAngle(stack, step: o.step)
        log("stack \(stack) at \(angle)°")
        if let cam {
            cam.keepAwake()
            try resetFocus(cam, o)
            cam.downloadFolder = stackFolder(stack)
            let before = cam.filesShot, start = Date()
            // A camera gone mid-bracket still gets the stack's line, with the files it wrote.
            do { try cam.shoot(expectedFiles: o.shots, timeout: timeout) } catch RigError.cameraGone {}
            let files = cam.filesShot - before
            log("  \(files == 1 ? "1 file" : "\(files) files") in \(String(format: "%.1f", Date().timeIntervalSince(start))) s")
            let folder = cam.downloadDir.map { $0.appendingPathComponent(cam.downloadFolder).path } ?? ""
            logFile.write(Data("\(stack),\(angle),\(iso.string(from: start)),\(iso.string(from: Date())),\(files),\(folder)\n".utf8))
            if cam.session == .disconnected { throw RigError.cameraGone }
        } else {
            logFile.write(Data("\(stack),\(angle),\(iso.string(from: Date())),,0,\n".utf8))
        }
        if let table, i + 1 < o.stacks {
            try table.turn(degrees: o.step, settle: o.settle)
        }
    }
    let angle = stackAngle(o.start + o.stacks - 1, step: o.step)
    log("done: \(o.stacks == 1 ? "1 stack" : "\(o.stacks) stacks"), table now at \(angle)°")
}

signal(SIGINT) { _ in interrupted = true }
atexit { closeCamera() }

do {
    let (command, positional, o) = try parse(Array(CommandLine.arguments.dropFirst()))
    switch command {
    case "help":
        print(usage)
    case "probe":
        if !o.noCamera {
            do { let cam = try openCamera(o); describeFocusShift(cam) } catch { warn("\(error)") }
        }
        if !o.noTurntable {
            do {
                let t = try openTurntable(o)
                let r = try t.send("+QT,CHANGEANGLE;")
                log("turntable CHANGEANGLE → \(r.isEmpty ? "(no reply)" : r)")
            } catch { warn("turntable: \(error)") }
        }
    case "focus":
        let cam = try openCamera(o)
        switch positional.first {
        case "evf": try cam.focusToNearByStepping(steps: o.nearSteps)
        default: try cam.focusToEdge(near: positional.first != "far", steps: o.nearSteps)
        }
        describeFocusShift(cam)
    case "shoot":
        if let d = o.download { try refuseShotFolders(URL(fileURLWithPath: d), ["shoot"]) }
        let cam = try openCamera(o)
        cam.downloadFolder = "shoot"
        describeFocusShift(cam)
        if o.configure { try cam.configureFocusShift(shots: o.shots, step: o.stepWidth) }
        let start = Date()
        try cam.shoot(expectedFiles: o.shots, timeout: o.idle)
        log("\(cam.filesShot == 1 ? "1 file" : "\(cam.filesShot) files") in \(String(format: "%.1f", Date().timeIntervalSince(start))) s")
    case "bracket":
        guard let state = positional.first, state == "on" || state == "off" else { throw RigError.usage("bracket needs on or off") }
        let cam = try openCamera(o)
        try cam.configureFocusShift(on: state == "on", shots: o.shots, step: o.stepWidth)
        describeFocusShift(cam)
    case "liveview":
        guard let file = positional.first else { throw RigError.usage("liveview needs an output file") }
        let cam = try openCamera(o)
        // With --near-steps, step the lens first (far if negative) to see that it moves.
        if o.nearSteps != 0 { try cam.focusToNearByStepping(far: o.nearSteps < 0, steps: abs(o.nearSteps)) }
        try cam.setLiveView(true)
        defer { try? cam.setLiveView(false) }
        if let frame = try cam.liveViewFrame() {
            try frame.write(to: URL(fileURLWithPath: file))
            log("live view frame: \(frame.count) bytes → \(file)")
        } else {
            log("live view: no frame returned")
        }
    case "turn":
        guard let deg = positional.first.flatMap(Double.init) else { throw RigError.usage("turn needs degrees, such as: stackrig turn 0.5") }
        let t = try openTurntable(o)
        try t.turn(degrees: deg, settle: o.settle)
    case "run":
        try runLoop(o)
    default:
        throw RigError.usage("unknown command \(command); use probe, focus, shoot, bracket, liveview, turn or run")
    }
    closeCamera()
} catch {
    fail("\(error)")
    closeCamera()
    exit(1)
}
