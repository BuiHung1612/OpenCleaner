import Foundation
import Testing
@testable import OpenCleaner

@Test func resultCacheRoundTrip() throws {
    let args = ["test-\(UUID().uuidString)"]
    defer { try? FileManager.default.removeItem(at: ResultCache.url(args)) }
    #expect(ResultCache.load(args) == nil)
    try ResultCache.save(Data(#"{"entries":[{"name":"x"}]}"#.utf8), for: args)
    let hit = try #require(ResultCache.load(args))
    #expect(hit.json.entries.array.first?.name.string == "x")
    #expect(abs(hit.date.timeIntervalSinceNow) < 60)
    #expect(ResultCache.url(args) != ResultCache.url(args + ["/tmp"]))
}
