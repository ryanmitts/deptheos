import Foundation
@preconcurrency import DeptheosEngine

private nonisolated let log = AppLog(category: "files")

/// What an item picked in Files is on: decides whether its frames are
/// copied in by default, and names the place when unreachable. iOS doesn't
/// name the provider, so this goes by the volume's description and mount
/// point; what it can't place is `provider`, treated as a cloud service.
nonisolated enum FileSource: Equatable, Sendable {
    /// The app's own Documents.
    case owned
    /// On My iPhone, or another app's folder on it.
    case device
    /// A USB drive or a card, by its name.
    case removable(String?)
    /// A network share, by its name.
    case network(String?)
    case iCloud
    /// Some other provider: Dropbox, Google Drive, OneDrive…
    case provider

    /// A stack reads every frame twice, which on-demand downloads and
    /// networks suit badly.
    var copiesByDefault: Bool {
        switch self {
        case .owned, .device, .removable: false
        case .network, .iCloud, .provider: true
        }
    }

    /// Its files may not be on the device until asked for.
    var downloads: Bool {
        switch self {
        case .iCloud, .provider: true
        case .owned, .device, .removable, .network: false
        }
    }

    /// Kept with the bookmark to say what to connect when it doesn't open:
    /// a drive or server by name, or iCloud Drive; `nil` on the device and
    /// for a provider. Worded when shown.
    var place: Place? {
        switch self {
        // A provider iOS does not name.
        case .owned, .device, .provider: nil
        case .removable(let name): .drive(name)
        case .network(let name): .server(name)
        case .iCloud: .iCloud
        }
    }

    /// In `UserDefaults`.
    var defaultsKey: String {
        switch self {
        case .owned, .device: "CopyFromDevice"
        case .removable: "CopyFromRemovable"
        case .network: "CopyFromNetwork"
        case .iCloud: "CopyFromICloud"
        case .provider: "CopyFromProvider"
        }
    }

    /// As chosen last, else the default.
    var copies: Bool {
        UserDefaults.standard.object(forKey: defaultsKey) as? Bool ?? copiesByDefault
    }

    static func of(_ url: URL) -> FileSource {
        if Bookmarks.isOwned(url) { return .owned }
        let path = url.standardizedFileURL.path
        let values = try? url.resourceValues(forKeys: [
            .isUbiquitousItemKey, .volumeIsLocalKey, .volumeIsRemovableKey, .volumeIsEjectableKey,
            .volumeIsInternalKey, .volumeLocalizedNameKey,
        ])
        let source = classify(
            path: path, ubiquitous: values?.isUbiquitousItem, local: values?.volumeIsLocal,
            removable: values?.volumeIsRemovable, ejectable: values?.volumeIsEjectable,
            internal: values?.volumeIsInternal, volume: values?.volumeLocalizedName)
        // Without the path: to learn how each provider shows up.
        log.debug(
            "picked: \(source.name, privacy: .public); ubiquitous \(describe(values?.isUbiquitousItem), privacy: .public), local \(describe(values?.volumeIsLocal), privacy: .public), removable \(describe(values?.volumeIsRemovable), privacy: .public), ejectable \(describe(values?.volumeIsEjectable), privacy: .public), internal \(describe(values?.volumeIsInternal), privacy: .public), mount \(mountKind(path), privacy: .public)")
        return source
    }

    /// The rule without the file system: `path` standardized, the rest as
    /// the URL's resource values.
    static func classify(
        path: String, ubiquitous: Bool?, local: Bool?, removable: Bool?, ejectable: Bool?,
        internal isInternal: Bool?, volume: String?
    ) -> FileSource {
        if ubiquitous == true || path.contains("/Mobile Documents/") { return .iCloud }
        // Where iOS mounts what Files connects to, each under its name.
        if let name = component(after: "/LiveFiles/com.apple.filesystems.smbclientd/", in: path) {
            return .network(volume ?? name)
        }
        if let name = component(after: "/LiveFiles/com.apple.filesystems.userfsd/", in: path) {
            return .removable(volume ?? name)
        }
        if local == false { return .network(volume) }
        if removable == true || ejectable == true || isInternal == false { return .removable(volume) }
        // On My iPhone, and other apps' folders shown under it.
        if path.contains("/File Provider Storage/") || path.contains("/Containers/Data/Application/") {
            return .device
        }
        return .provider
    }

    private var name: String {
        switch self {
        case .owned: "owned"
        case .device: "device"
        case .removable: "removable"
        case .network: "network"
        case .iCloud: "iCloud"
        case .provider: "provider"
        }
    }

    private static func describe(_ value: Bool?) -> String {
        value.map { $0 ? "yes" : "no" } ?? "unknown"
    }

    /// The system folder a path is mounted under, never the user's own.
    private static func mountKind(_ path: String) -> String {
        for marker in ["/LiveFiles/", "/Mobile Documents/", "/File Provider Storage/", "/Containers/Data/Application/", "/CloudStorage/"]
        where path.contains(marker) {
            return marker
        }
        return "other"
    }

    private static func component(after marker: String, in path: String) -> String? {
        guard let range = path.range(of: marker) else { return nil }
        let name = path[range.upperBound...].prefix { $0 != "/" }
        return name.isEmpty ? nil : String(name)
    }
}

/// What a picked item is on (`Place.of`). Each sentence naming it is whole
/// per kind, so no English noun phrase is slotted into another.
nonisolated enum Place: Equatable, Sendable {
    /// A USB drive or card.
    case drive(String?)
    /// A network share.
    case server(String?)
    case iCloud

    /// "T7 SSD".
    private var name: String? {
        switch self {
        case .drive(let name), .server(let name): name
        case .iCloud: nil
        }
    }

    /// "On T7 SSD", the Frames list's head badge.
    var onLine: LocalizedStringResource {
        if let place = name {
            return LocalizedStringResource(
                "On \(place)",
                comment: "The badge at the head of the Frames list: where its frames are read from. The placeholder is a drive's or a server's name.")
        }
        switch self {
        case .server:
            return LocalizedStringResource(
                "On the server", comment: "The badge at the head of the Frames list: its frames are read from a server with no name.")
        case .iCloud:
            return LocalizedStringResource(
                "On iCloud Drive", comment: "The badge at the head of the Frames list: its frames are read from iCloud Drive.")
        case .drive:
            return LocalizedStringResource(
                "On the drive", comment: "The badge at the head of the Frames list: its frames are read from a drive with no name.")
        }
    }

    /// "T7 SSD Not Connected", the Frames list's head badge while its
    /// frames can't be reached.
    var notConnected: LocalizedStringResource {
        if let place = name {
            return LocalizedStringResource(
                "\(place) Not Connected",
                comment: "The badge at the head of the Frames list: the drive or server its frames are on isn't connected. The placeholder is its name.")
        }
        switch self {
        case .server:
            return LocalizedStringResource(
                "Server Not Connected", comment: "The badge at the head of the Frames list: the server with no name its frames are on isn't connected.")
        case .iCloud:
            return LocalizedStringResource(
                "iCloud Drive Not Connected", comment: "The badge at the head of the Frames list: iCloud Drive, where its frames are, can't be reached.")
        case .drive:
            return LocalizedStringResource(
                "Drive Not Connected", comment: "The badge at the head of the Frames list: the drive with no name its frames are on isn't connected.")
        }
    }

    var symbol: String {
        switch self {
        case .drive: "externaldrive"
        case .server: "server.rack"
        case .iCloud: "icloud"
        }
    }

    /// "on T7 SSD", in the Add Frames sheet ("214 frames · 14.2 GB · on T7
    /// SSD").
    var onInLine: LocalizedStringResource {
        if let place = name {
            return LocalizedStringResource(
                "on \(place)",
                comment: "Add Frames sheet: where a picked folder is, in the line under its name ('214 frames · 14.2 GB · on T7 SSD'). The placeholder is a drive's or server's name.")
        }
        switch self {
        case .server:
            return LocalizedStringResource(
                "on the server",
                comment: "Add Frames sheet: a picked folder is on a server with no name, in the line under its name ('214 frames · 14.2 GB · on the server').")
        case .iCloud:
            return LocalizedStringResource(
                "on iCloud Drive",
                comment: "Add Frames sheet: a picked folder is in iCloud Drive, in the line under its name ('214 frames · 14.2 GB · on iCloud Drive').")
        case .drive:
            return LocalizedStringResource(
                "on the drive",
                comment: "Add Frames sheet: a picked folder is on a drive with no name, in the line under its name ('214 frames · 14.2 GB · on the drive').")
        }
    }

    /// The Add Frames sheet's footer while Copy into Deptheos is off.
    var readsFrom: LocalizedStringResource {
        if let place = name {
            return LocalizedStringResource(
                "Deptheos reads the frames from \(place) each time. Stacking again and retouching need them there.",
                comment: "Add Frames sheet: the footer under the folders while Copy into Deptheos is off. The placeholder is a drive's or server's name.")
        }
        switch self {
        case .server:
            return LocalizedStringResource(
                "Deptheos reads the frames from the server each time. Stacking again and retouching need them there.",
                comment: "Add Frames sheet: the footer under the folders while Copy into Deptheos is off, for folders on a server with no name.")
        case .iCloud:
            return LocalizedStringResource(
                "Deptheos reads the frames from iCloud Drive each time. Stacking again and retouching need them there.",
                comment: "Add Frames sheet: the footer under the folders while Copy into Deptheos is off, for folders in iCloud Drive.")
        case .drive:
            return LocalizedStringResource(
                "Deptheos reads the frames from the drive each time. Stacking again and retouching need them there.",
                comment: "Add Frames sheet: the footer under the folders while Copy into Deptheos is off, for folders on a drive with no name.")
        }
    }

    /// Alert when Stack is tapped with `count` unconnected frames on it.
    func cantBeReached(_ count: Int) -> LocalizedStringResource {
        if let place = name {
            return LocalizedStringResource(
                "\(count) frames can't be reached. Connect \(place), or use Locate… if the folder has moved.",
                comment: "Alert text when Stack is tapped: frames are on a drive or server that isn't connected. The placeholders are how many frames and the drive's or server's name.")
        }
        switch self {
        case .server:
            return LocalizedStringResource(
                "\(count) frames can't be reached. Connect the server, or use Locate… if the folder has moved.",
                comment: "Alert text when Stack is tapped: frames are on a server with no name that isn't connected. The placeholder is how many frames.")
        case .iCloud:
            return LocalizedStringResource(
                "\(count) frames can't be reached. Connect iCloud Drive, or use Locate… if the folder has moved.",
                comment: "Alert text when Stack is tapped: frames are in iCloud Drive, which can't be reached. The placeholder is how many frames.")
        case .drive:
            return LocalizedStringResource(
                "\(count) frames can't be reached. Connect the drive, or use Locate… if the folder has moved.",
                comment: "Alert text when Stack is tapped: frames are on a drive with no name that isn't connected. The placeholder is how many frames.")
        }
    }

    /// The viewer's text under Not Connected.
    var connectToView: LocalizedStringResource {
        if let place = name {
            return LocalizedStringResource(
                "Connect \(place), or use Locate… if the frames moved.",
                comment: "Viewer empty-state text under Not Connected. The placeholder is the drive's or server's name.")
        }
        switch self {
        case .server:
            return LocalizedStringResource(
                "Connect the server, or use Locate… if the frames moved.",
                comment: "Viewer empty-state text under Not Connected, for a server with no name.")
        case .iCloud:
            return LocalizedStringResource(
                "Connect iCloud Drive, or use Locate… if the frames moved.",
                comment: "Viewer empty-state text under Not Connected, for frames in iCloud Drive.")
        case .drive:
            return LocalizedStringResource(
                "Connect the drive, or use Locate… if the frames moved.",
                comment: "Viewer empty-state text under Not Connected, for a drive with no name.")
        }
    }

    /// The Frames head badge's menu while `count` frames can't be reached.
    func connectInList(_ count: Int) -> LocalizedStringResource {
        if let place = name {
            return LocalizedStringResource(
                "Connect \(place), or use Locate… if \(count) frames moved.",
                comment: "The menu of the badge at the head of the Frames list, while frames can't be reached. The placeholders are the drive's or server's name and how many frames; in English the number only picks 'it' or 'they' and isn't shown.")
        }
        switch self {
        case .server:
            return LocalizedStringResource(
                "Connect the server, or use Locate… if \(count) frames moved.",
                comment: "The menu of the badge at the head of the Frames list, while frames can't be reached, on a server with no name. The placeholder is how many frames; in English the number only picks 'it' or 'they' and isn't shown.")
        case .iCloud:
            return LocalizedStringResource(
                "Connect iCloud Drive, or use Locate… if \(count) frames moved.",
                comment: "The menu of the badge at the head of the Frames list, while frames can't be reached, in iCloud Drive. The placeholder is how many frames; in English the number only picks 'it' or 'they' and isn't shown.")
        case .drive:
            return LocalizedStringResource(
                "Connect the drive, or use Locate… if \(count) frames moved.",
                comment: "The menu of the badge at the head of the Frames list, while frames can't be reached, on a drive with no name. The placeholder is how many frames; in English the number only picks 'it' or 'they' and isn't shown.")
        }
    }
}

extension Place {
    /// What `root` is on (ADR-145): a share by its volume's name, else by
    /// its path (a drive, iCloud Drive); nothing on the device.
    nonisolated static func of(_ root: RootInfo) -> Place? {
        let named = root.volume.isEmpty ? nil : root.volume
        if root.share { return .server(named) }
        return FileSource.classify(
            path: root.folder, ubiquitous: nil, local: nil, removable: nil, ejectable: nil, internal: nil,
            volume: named
        ).place
    }
}
