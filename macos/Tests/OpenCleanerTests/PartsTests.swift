import Foundation
import Testing
@testable import OpenCleaner

/// /r holds folder a (x, y) and file b.
private func tree() -> Parts {
    let entry = { (path: String, size: Double) in JSON(["path": path, "size": size]) }
    var p = Parts()
    p.children["/r"] = [entry("/r/a", 10), entry("/r/b", 5)]
    p.children["/r/a"] = [entry("/r/a/x", 3), entry("/r/a/y", 7)]
    return p
}

@Test func untouchedFolderGoesWhole() {
    let p = tree()
    #expect(!p.isPartial("/r"))
    #expect(p.request(["/r"]).isEmpty)
    #expect(p.bytes("/r", whole: 15) == 15)
}

@Test func unticksAtAnyDepthSendTheDeepestFullyTickedItems() {
    var p = tree()
    var ok = false
    ok = p.toggle("/r/a/x", root: "/r", rootOn: true); #expect(ok)
    #expect(p.isPartial("/r") && p.isPartial("/r/a"))
    #expect(p.request(["/r"]) == ["/r": ["/r/a/y", "/r/b"]])
    #expect(p.bytes("/r", whole: 15) == 12)

    ok = p.toggle("/r/b", root: "/r", rootOn: true); #expect(ok)
    #expect(p.request(["/r"]) == ["/r": ["/r/a/y"]])
    // the last thing ticked under /r goes: /r itself is unticked
    ok = p.toggle("/r/a/y", root: "/r", rootOn: true); #expect(!ok)
    #expect(p.ticked.isEmpty)
}

@Test func tickingDeepInsideAnUntickedFolderTicksThatOnly() {
    var p = tree()
    var ok = false
    ok = p.toggle("/r/a/x", root: "/r", rootOn: false); #expect(ok)
    #expect(p.request(["/r"]) == ["/r": ["/r/a/x"]])
    #expect(p.bytes("/r", whole: 15) == 3)
    // a folder ticked in full is sent as itself, not as its children
    ok = p.toggle("/r/a/y", root: "/r", rootOn: true); #expect(ok)
    #expect(p.request(["/r"]) == ["/r": ["/r/a"]])
    #expect(!p.isPartial("/r/a"))
    // everything ticked again: /r goes whole
    ok = p.toggle("/r/b", root: "/r", rootOn: true); #expect(ok)
    #expect(!p.isPartial("/r") && p.request(["/r"]).isEmpty)
}

@Test func tickingAFolderWholeForgetsTicksInsideIt() {
    var p = tree()
    var ok = false
    _ = p.toggle("/r/a/x", root: "/r", rootOn: true)
    // a off, then on again: whole, with x back
    ok = p.toggle("/r/a", root: "/r", rootOn: true); #expect(ok)
    #expect(p.isTicked("/r/b", in: "/r") && !p.isTicked("/r/a", in: "/r"))
    ok = p.toggle("/r/a", root: "/r", rootOn: true); #expect(ok)
    #expect(p.ticked.isEmpty)
    // a folder not in the request's chosen items is not sent
    _ = p.toggle("/r/b", root: "/r", rootOn: true)
    #expect(p.request(["/other"]).isEmpty)
}

@MainActor
@Test func twoAppsOfOneNameSayWhereEachIs() {
    let app = { (s: String) in JSON(data: Data(s.utf8)) }
    #expect(UninstallerPage.subtitle(app(#"{"bundle_id":"a.b","path":"/Applications/X.app"}"#)) == "a.b")
    #expect(UninstallerPage.subtitle(app(#"{"bundle_id":"a.b","path":"/Applications/Codex.app","version":"1","twin":true}"#))
        == "/Applications/Codex.app · 1")
    #expect(UninstallerPage.subtitle(app(#"{"path":"/Applications/C.app","twin":true,"running":true}"#))
        == "Open now: quit it before uninstalling · /Applications/C.app")
}
