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
