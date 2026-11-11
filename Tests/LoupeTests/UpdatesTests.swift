import Testing
import Foundation
@testable import Loupe

@Test func updaterNeedsAppBundleFeedAndKey() {
    let app = URL(fileURLWithPath: "/Applications/Loupe.app")
    let feed = "https://github.com/exTypen/loupe/releases/latest/download/appcast.xml"
    #expect(Updates.isConfigured(app, ["SUFeedURL": feed, "SUPublicEDKey": "abc="]))
    // No key (bundle.sh warned): updating is hidden rather than Sparkle failing to start.
    #expect(!Updates.isConfigured(app, ["SUFeedURL": feed]))
    #expect(!Updates.isConfigured(app, ["SUFeedURL": feed, "SUPublicEDKey": "  "]))
    // `swift run` binary, not an .app bundle.
    #expect(!Updates.isConfigured(URL(fileURLWithPath: "/tmp/.build/debug"), ["SUFeedURL": feed, "SUPublicEDKey": "abc="]))
    #expect(!Updates.isConfigured(app, ["SUFeedURL": "http://example.com/appcast.xml", "SUPublicEDKey": "abc="]))
}
