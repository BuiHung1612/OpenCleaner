import Foundation
import Testing
@testable import OpenCleaner

@MainActor
private func finished(_ model: SpaceLensModel) async throws {
    for _ in 0..<300 where model.isLoading { try await Task.sleep(for: .milliseconds(10)) }
    #expect(!model.isLoading && model.error == nil)
}

@MainActor
@Test func spaceLensRevalidatesCachedResultsAfterExternalChanges() async throws {
    let fm = FileManager.default
    let dir = fm.temporaryDirectory.appendingPathComponent("oc-fresh-\(UUID().uuidString)")
    try fm.createDirectory(at: dir, withIntermediateDirectories: true)
    let model = SpaceLensModel()
    defer {
        model.pause()
        try? fm.removeItem(at: dir)
        try? fm.removeItem(at: ResultCache.url(["space", dir.path]))
    }
    try Data([1]).write(to: dir.appendingPathComponent("old"))
    model.open(dir.path)
    try await finished(model)
    #expect(model.entries.map(\.name.string) == ["old"])
    model.selection = [dir.appendingPathComponent("old").path]
    model.pause()
    try fm.removeItem(at: dir.appendingPathComponent("old"))
    try Data([2]).write(to: dir.appendingPathComponent("new"))
    await model.resume()
    // The old result stays visible, but it is explicitly marked as old until remeasured.
    #expect(model.staleDate != nil)
    try await finished(model)
    #expect(model.entries.map(\.name.string) == ["new"])
    #expect(model.selection.isEmpty)
    #expect(model.staleDate == nil && !model.isLive)
}

@MainActor
@Test func spaceLensStopsOnExitAndOnlyTheLatestScanCanPublish() async throws {
    let fm = FileManager.default
    let dir = fm.temporaryDirectory.appendingPathComponent("oc-lifecycle-\(UUID().uuidString)")
    let a = dir.appendingPathComponent("a"), b = dir.appendingPathComponent("b")
    for folder in [a, b] { try fm.createDirectory(at: folder, withIntermediateDirectories: true) }
    try Data([1]).write(to: a.appendingPathComponent("first"))
    try Data([2]).write(to: b.appendingPathComponent("last"))
    let model = SpaceLensModel()
    defer {
        model.pause()
        try? fm.removeItem(at: dir)
        for path in [a.path, b.path] { try? fm.removeItem(at: ResultCache.url(["space", path])) }
    }
    await model.resume()
    #expect(!model.isLoading && model.current == nil)
    model.open(a.path)
    await Task.yield()
    model.pause()
    #expect(!model.isLoading)
    await model.resume()
    model.open(b.path)
    try await finished(model)
    #expect(model.entries.map(\.name.string) == ["last"])
    #expect(model.stack.last! == b.path)
}

@MainActor
@Test func trashProgressLeavesTheUIResponsiveAndKeepsFailures() async throws {
    let fm = FileManager.default
    let dir = fm.temporaryDirectory.appendingPathComponent("oc-trash-progress-\(UUID().uuidString)")
    try fm.createDirectory(at: dir, withIntermediateDirectories: true)
    for name in ["a", "b"] { try Data([1]).write(to: dir.appendingPathComponent(name)) }
    let model = SpaceLensModel()
    defer {
        model.pause()
        try? fm.removeItem(at: dir)
        try? fm.removeItem(at: ResultCache.url(["space", dir.path]))
    }
    model.open(dir.path)
    try await finished(model)
    model.selection = Set(model.entries.compactMap(\.path.string))
    let began = DispatchSemaphore(value: 0), release = DispatchSemaphore(value: 0)
    let operation = Task {
        await model.removeSelected { path in
            // A gated fake mover: no real files are trashed, and scheduler speed does not matter.
            began.signal()
            guard !Thread.isMainThread else { return false }
            guard release.wait(timeout: .now() + 5) == .success else { return false }
            return !path.hasSuffix("/b")
        }
    }
    let started = await withCheckedContinuation { reply in
        DispatchQueue.global().async { reply.resume(returning: began.wait(timeout: .now() + 5) == .success) }
    }
    #expect(started)
    #expect(model.removing && model.trashTotal == 2 && model.trashProcessed == 0)
    model.open("/should-not-navigate-during-removal")
    #expect(model.stack.last! == dir.path)
    // Leaving the page stops scanning, but never interrupts a move already in progress.
    model.pause()
    release.signal()
    release.signal()
    await operation.value
    #expect(!model.removing && !model.isLoading)
    #expect(model.trashProcessed == 2 && model.trashed == 1)
    #expect(model.trashError?.contains("b") == true)
}
