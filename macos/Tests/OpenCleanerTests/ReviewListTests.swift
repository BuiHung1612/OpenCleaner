import AppKit
import Foundation
import Testing
@testable import OpenCleaner

@Test func listFilterMatchesAndSorts() {
    var f = ListFilter()
    #expect(f.matches("anything"))
    f.query = " cache "
    #expect(f.matches("x", "/Users/a/Library/Caches/x"))
    #expect(!f.matches("x", "/tmp/x"))
    let rows = [("b10", 1.0), ("b9", 3.0), ("A", 2.0)]
    #expect(f.sorted(rows, name: { $0.0 }, size: { $0.1 }).map(\.0) == ["b9", "A", "b10"])
    f.sort = .name
    #expect(f.sorted(rows, name: { $0.0 }, size: { $0.1 }).map(\.0) == ["A", "b9", "b10"])
}

@Test func droppedFolderIsShownOnItsOwnVolume() {
    let v = { (p: String) in Volume(url: URL(fileURLWithPath: p), name: p, total: 1, free: 0) }
    let all = [v("/"), v("/Volumes/X"), v("/Volumes/XY")]
    #expect(Volume.holding("/Volumes/XY/a", in: all)?.url.path == "/Volumes/XY")
    #expect(Volume.holding("/Volumes/X", in: all)?.url.path == "/Volumes/X")
    #expect(Volume.holding("/Users/me", in: all)?.url.path == "/")
}

@MainActor
@Test func pickDetailsAreWordedFromTheEngineFields() {
    let pick = { (s: String) in JSON(data: Data(s.utf8)) }
    #expect(PickText.detail(pick(#"{"detail":"x","idle_days":200}"#)) == "Not opened for 200 days")
    #expect(PickText.detail(pick(#"{"detail":"x","kind":"PKG","age_days":3}"#)) == "PKG · 3 days old")
    #expect(PickText.detail(pick(#"{"detail":"x","kind":"npm packages","flags":["active"]}"#)) == "npm packages · active this week")
    #expect(PickText.detail(pick(#"{"detail":"x","place":"~/Downloads","flags":["kept"]}"#)) == "~/Downloads · oldest, kept")
    #expect(PickText.detail(pick(#"{"detail":"x","kind":"PKG","age_days":0}"#)) == "PKG · today")
    #expect(PickText.detail(pick(#"{"place":"~/Downloads","bytes":0}"#)).hasSuffix("removing it frees no space"))
    #expect(PickText.detail(pick(#"{"place":"~/Downloads","bytes":5}"#)) == "~/Downloads")
    #expect(UninstallerPage.title(pick(#"{"name":"Codex","twin":true,"path":"/Applications/Codex Beta.app"}"#)) == "Codex Beta")
    #expect(UninstallerPage.title(pick(#"{"name":"Codex","path":"/Applications/Codex Beta.app"}"#)) == "Codex")
    // older engines: the English sentence as sent
    #expect(PickText.detail(pick(#"{"detail":"Cache"}"#)) == "Cache")
    #expect(PickText.group("a.zip · 2 copies", first: pick(#"{"copies":2}"#)) == "a.zip · 2 copies")
    let groups = PicksModel.grouped([pick(#"{"group":"b"}"#), pick(#"{"group":"a"}"#), pick(#"{"group":"b"}"#)])
    #expect(groups.map(\.indexes) == [[0, 2], [1]])
}

@MainActor
@Test func sizesAreWordedForPeople() {
    #expect(bytes(0) == ByteCountFormatter.string(fromByteCount: 4000, countStyle: .file).replacingOccurrences(of: "4", with: "0"))
    #expect(!bytes(0).localizedCaseInsensitiveContains("zero"))
    #expect(memBytes(16 * 1_073_741_824) == ByteCountFormatter.string(fromByteCount: 16 * 1_073_741_824, countStyle: .memory))
    #expect(sizeText(10, selected: 4, partial: false) == bytes(10))
    #expect(sizeText(10, selected: 4, partial: true).contains(bytes(4)))
    // a word never splits: the longest one fits the width
    let size = BigButton.fitting("Run Selected", size: 22, width: 80)
    let font = NSFont.systemFont(ofSize: size, weight: .semibold)
    #expect(size < 22 && ("Selected" as NSString).size(withAttributes: [.font: font]).width <= 80)
    #expect(BigButton.fitting("Scan", size: 17, width: 80) == 17)
}
