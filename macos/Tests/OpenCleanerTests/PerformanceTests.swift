import Foundation
import Testing
@testable import OpenCleaner

@Test func logTailKeepsLastLines() {
    let log = (1...5000).map { "line \($0)" }.joined(separator: "\n") + "\n"
    let t = Session.tail(log, lines: 2000)
    #expect(t.hasPrefix("…\nline 3001\n"))
    #expect(t.hasSuffix("line 5000\n"))
    #expect(Session.tail("a\nb\n", lines: 2000) == "a\nb\n")
}

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
