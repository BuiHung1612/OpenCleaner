import Foundation
import Testing
@testable import OpenCleaner

@Test func engineAnswersThroughTheCABI() {
    #expect(Engine.version?.isEmpty == false)
    #expect(Engine.call(["op": "nope"]).ok.bool == false)
}

@Test func engineListsStartupItemsAndAnswersPermissions() {
    #expect(Engine.call(["op": "startup_items"]).ok.bool)
    // with or without Full Disk Access, an answer, never a crash
    #expect(Engine.call(["op": "permissions"]).ok.bool)
    #expect(Engine.call(["op": "disable_startup", "path": "/etc/hosts"]).ok.bool == false)
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
    let names = { Set((try? FileManager.default.contentsOfDirectory(atPath: trash.path)) ?? []) }
    let before = names()
    #expect(SpaceLensModel.moveToTrash("/usr/bin/true") == false)
    #expect(FileManager.default.fileExists(atPath: "/usr/bin/true"))
    // the Trash may be created (with its own files) on a fresh machine; what must not
    // appear is a copy of what could not be moved
    let copies = names().subtracting(before).filter { $0.hasPrefix("true") }
    #expect(copies.isEmpty, "copy left in the Trash: \(copies)")
}

@MainActor
@Test func uninstallerListsAppsAndFillsInSizes() async throws {
    let model = AppsModel()
    model.load()
    // a cold size cache on a busy runner can take a while
    let deadline = Date().addingTimeInterval(60)
    while Date() < deadline, model.apps == nil || model.sizes.allSatisfy({ $0 == 0 }) {
        try await Task.sleep(for: .milliseconds(50))
    }
    let apps = try #require(model.apps)
    #expect(!apps.isEmpty)
    #expect(apps.allSatisfy { $0.path.string?.hasSuffix(".app") == true })
    #expect(model.sizes.contains { $0 > 0 })
}

/// The scripts the engine hands over must survive AppleScript quoting (compiled, not run:
/// no password prompt).
@MainActor
@Test func privilegedScriptsCompileAsAppleScript() throws {
    let touchOn = try #require(Engine.call(["op": "touchid_script", "enable": true]).script.string)
    let optimize = Engine.call(["op": "optimize", "tasks": ["dns", "memory"]]).admin_script.string ?? "true"
    for script in [touchOn, optimize, #"echo "quoted \ backslash" 'single'"#] {
        var error: NSDictionary?
        let compiled = NSAppleScript(source: Privileged.source(for: script))?.compileAndReturnError(&error) ?? false
        #expect(compiled, "\(script): \(String(describing: error))")
    }
}

@Test func dashboardSuggestsOnlyWhatIsWorthIt() {
    let quiet = JSON(["trash_size": 500_000_000, "disks": [["mount": "/", "total": 1e12, "used": 5e11]]])
    #expect(DashboardPage.suggestions(status: quiet, junkBytes: 1e9, devtoolsBytes: 0).isEmpty)
    let busy = JSON(["trash_size": 8e9, "disks": [["mount": "/", "total": 1e12, "used": 9.5e11]]])
    let tips = DashboardPage.suggestions(status: busy, junkBytes: 5e9, devtoolsBytes: 3e9)
    #expect(tips.count == 3)
    #expect(tips.first?.module == .spaceLens)
    #expect(tips.map(\.module).contains(.tool("clean")))
    #expect(tips.first { $0.symbol == "trash" }?.group == "trash")
}

@Test func dashboardReadsThisMonthFromHistory() {
    let history = JSON(["months": [["month": "2026-10", "bytes": 1e9], ["month": "2026-11", "bytes": 5e9]]])
    let now = ISO8601DateFormatter().date(from: "2026-11-20T10:00:00Z")!
    #expect(DashboardPage.freedThisMonth(history, now: now) == 5e9)
    #expect(DashboardPage.freedThisMonth(JSON(["months": []]), now: now) == nil)
}

/// Read-only ops (the listing runs `docker system df` and `simctl list`, never a cleanup).
@Test func historyHasMonthsAndDevtoolsAnswers() {
    #expect(Engine.call(["op": "history"]).months.raw is [Any])
    #expect(Engine.call(["op": "devtools"]).ok.bool)
}
