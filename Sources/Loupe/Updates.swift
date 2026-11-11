import SwiftUI
import Sparkle

// MARK: - Auto-update (Sparkle 2)

/// Owns the Sparkle updater. It exists only in a bundled Loupe.app whose Info.plist carries a feed and an EdDSA public
/// key (scripts/bundle.sh writes both); a `swift run` binary or a build without a key has no updater, and the menu item
/// and Settings section are hidden instead of Sparkle failing to start with an alert.
@MainActor
final class Updates: ObservableObject {
    static let shared = Updates()

    let updater: SPUUpdater?
    private let controller: SPUStandardUpdaterController?
    @Published private(set) var canCheck = false
    @Published var automaticallyChecks = false { didSet { if updater?.automaticallyChecksForUpdates != automaticallyChecks { updater?.automaticallyChecksForUpdates = automaticallyChecks } } }
    @Published var automaticallyDownloads = false { didSet { if updater?.automaticallyDownloadsUpdates != automaticallyDownloads { updater?.automaticallyDownloadsUpdates = automaticallyDownloads } } }

    private init() {
        guard Self.isConfigured(Bundle.main.bundleURL, Bundle.main.infoDictionary ?? [:]) else { updater = nil; controller = nil; return }
        let c = SPUStandardUpdaterController(startingUpdater: true, updaterDelegate: nil, userDriverDelegate: nil)
        controller = c
        updater = c.updater
        c.updater.publisher(for: \.canCheckForUpdates).receive(on: DispatchQueue.main).assign(to: &$canCheck)
        automaticallyChecks = c.updater.automaticallyChecksForUpdates
        automaticallyDownloads = c.updater.automaticallyDownloadsUpdates
    }

    var isAvailable: Bool { updater != nil }

    func checkForUpdates() { controller?.checkForUpdates(nil) }

    /// Updating needs an .app bundle (Sparkle replaces it in place), a feed, and a non-empty public key to verify against.
    nonisolated static func isConfigured(_ bundleURL: URL, _ info: [String: Any]) -> Bool {
        guard bundleURL.pathExtension == "app",
              let feed = info["SUFeedURL"] as? String, URL(string: feed)?.scheme == "https",
              let key = info["SUPublicEDKey"] as? String else { return false }
        return !key.trimmingCharacters(in: .whitespaces).isEmpty
    }
}

/// The app menu's "Check for Updates…", under About.
struct CheckForUpdatesButton: View {
    @ObservedObject var updates = Updates.shared
    var body: some View {
        if updates.isAvailable {
            Button("Check for Updates…") { updates.checkForUpdates() }.disabled(!updates.canCheck)
        }
    }
}

/// Settings › General.
struct UpdateSettings: View {
    @ObservedObject var updates = Updates.shared
    var body: some View {
        if updates.isAvailable {
            Section("Updates") {
                Toggle("Automatically check for updates", isOn: $updates.automaticallyChecks)
                Toggle("Automatically download and install updates", isOn: $updates.automaticallyDownloads)
                    .disabled(!updates.automaticallyChecks)
            }
        }
    }
}
