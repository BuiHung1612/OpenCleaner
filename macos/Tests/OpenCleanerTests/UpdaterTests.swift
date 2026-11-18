import Testing
@testable import OpenCleaner

@Test func versionOrdering() {
    #expect(Updater.isNewer("0.1.1", than: "0.1.0"))
    #expect(Updater.isNewer("1.0.10", than: "1.0.9"))
    #expect(Updater.isNewer("2.0", than: "1.9.9"))
    #expect(!Updater.isNewer("1.0", than: "1.0.0"))
    #expect(!Updater.isNewer("0.1.0", than: "0.1.0"))
    #expect(!Updater.isNewer("0.9.9", than: "1.0.0"))
}
