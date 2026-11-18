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

@MainActor
@Test func spaceLensListsAFolderThroughTheEngine() async throws {
    let dir = FileManager.default.temporaryDirectory.appendingPathComponent("oc-space-\(UUID().uuidString)")
    let fm = FileManager.default
    try fm.createDirectory(at: dir.appendingPathComponent("big/inner"), withIntermediateDirectories: true)
    try fm.createDirectory(at: dir.appendingPathComponent("small"), withIntermediateDirectories: true)
    defer { try? fm.removeItem(at: dir) }
    try Data(count: 3_000_000).write(to: dir.appendingPathComponent("big/inner/a"))
    try Data(count: 200_000).write(to: dir.appendingPathComponent("small/b"))
    try Data(count: 50_000).write(to: dir.appendingPathComponent("c.txt"))

    let model = SpaceLensModel()
    model.open(dir.path)
    for _ in 0..<200 where model.current?.event.string != "space" || model.isLive {
        try await Task.sleep(for: .milliseconds(20))
    }
    let names = model.entries.compactMap(\.name.string)
    #expect(names == ["big", "small", "c.txt"])
    #expect((model.entries.first?.size.double ?? 0) >= 3_000_000)
    let selectable = model.entries.filter { $0.selectable.bool }.count
    #expect(selectable == 3)
    #expect(!model.isLive)
}

@Test func engineReadsSystemStatus() async {
    let first = Engine.call(["op": "status"])
    #expect(first.ok.bool)
    #expect((first.memory.total.double ?? 0) > 0)
    #expect((first.cpu.cores.double ?? 0) > 0)
    #expect(!first.disks.array.isEmpty)
    // later readings compare against the previous one: no sampling pause
    let start = Date()
    _ = Engine.call(["op": "status"])
    #expect(Date().timeIntervalSince(start) < 0.2)
}

@Test func engineListsItsCleaningRules() {
    let rules = Engine.call(["op": "rules"])
    #expect(rules.ok.bool)
    let ids = rules.groups.array.flatMap(\.rules.array).compactMap(\.id.string)
    #expect(ids.contains("user.caches"))
    // emptying the Trash is never preselected
    let trash = rules.groups.array.flatMap(\.rules.array).first { $0.id.string == "trash" }
    #expect(trash?.default.bool == false)
}

/// Uses the real Trash; harmless: /usr/bin/true cannot move and its copy is taken back.
@MainActor
@Test func trashingWhatCannotMoveFailsWithoutLeavingACopy() throws {
    let trash = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".Trash")
    let before = (try? FileManager.default.contentsOfDirectory(atPath: trash.path).count) ?? 0
    #expect(SpaceLensModel.moveToTrash("/usr/bin/true") == false)
    #expect(FileManager.default.fileExists(atPath: "/usr/bin/true"))
    let after = (try? FileManager.default.contentsOfDirectory(atPath: trash.path).count) ?? 0
    #expect(after == before)
}
