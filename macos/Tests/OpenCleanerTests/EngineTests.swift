import Foundation
import Testing
@testable import OpenCleaner

@Test func engineAnswersThroughTheCABI() {
    #expect(Engine.version?.isEmpty == false)
    #expect(Engine.call(["op": "nope"]).ok.bool == false)
}

@Test func engineJobStreamsEventsUntilDone() async throws {
    let dir = FileManager.default.temporaryDirectory.appendingPathComponent("oc-engine-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: dir) }
    try Data(count: 100_000).write(to: dir.appendingPathComponent("f"))

    var events: [JSON] = []
    for await event in Engine.run(["op": "size", "paths": [dir.path]]) { events.append(event) }
    #expect(events.count == 1)
    #expect((events.first?.report.bytes.double ?? 0) >= 100_000)
    #expect(events.first?.report.files.double == 1)
}
