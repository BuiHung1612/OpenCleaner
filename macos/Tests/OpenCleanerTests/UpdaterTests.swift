import Foundation
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

@Test func releaseAssetSelection() {
    let reply = #"{"tag_name":"v1.2.0","assets":[{"name":"OpenCleaner-windows.zip","browser_download_url":"https://x/win.zip"},{"name":"OpenCleaner-macos.zip","browser_download_url":"https://x/mac.zip"}]}"#
    let release = Updater.release(from: JSON(data: Data(reply.utf8)))
    #expect(release?.version == "1.2.0")
    #expect(release?.download?.absoluteString == "https://x/mac.zip")
    // a release without the macOS zip still reports its version, with nothing to install
    let bare = Updater.release(from: JSON(data: Data(#"{"tag_name":"v1.3.0","assets":[]}"#.utf8)))
    #expect(bare?.version == "1.3.0" && bare?.download == nil)
    #expect(Updater.release(from: JSON(data: Data(#"{"message":"rate limited"}"#.utf8))) == nil)
}

@Test func signerParsing() {
    let signed = Updater.signer(fromCodesign: "Identifier=io.github.opencleaner\nAuthority=OpenCleaner Code Signing\nSigned Time=x\nTeamIdentifier=not set\n")
    #expect(!signed.adHoc && signed.authorities == ["OpenCleaner Code Signing"] && signed.team == "not set")
    let adHoc = Updater.signer(fromCodesign: "Identifier=OpenCleaner\nSignature=adhoc\nTeamIdentifier=not set\n")
    #expect(adHoc.adHoc && adHoc != signed)
}

/// Real signatures: an app accepts itself, rejects another app, and nothing can update an ad-hoc build.
@Test func sameSignerCheck() throws {
    let calc = URL(fileURLWithPath: "/System/Applications/Calculator.app")
    let textEdit = URL(fileURLWithPath: "/System/Applications/TextEdit.app")
    #expect(Updater.signer(of: calc)?.authorities.first == "macOS Software Signing")
    try Updater.verifySameSigner(current: calc, new: calc)
    #expect(throws: Updater.Failure.self) { try Updater.verifySameSigner(current: calc, new: textEdit) }
    let adHoc = URL(fileURLWithPath: CommandLine.arguments[0]) // the test runner, signed ad-hoc by the linker
    if Updater.signer(of: adHoc)?.adHoc == true {
        #expect(throws: Updater.Failure.self) { try Updater.verifySameSigner(current: adHoc, new: adHoc) }
    }
}

@Test func crashIssueURLStaysShort() {
    let report = (1...100).map { "line \($0) " + String(repeating: "x", count: 200) }.joined(separator: "\n")
    let url = CrashReports.issueURL(version: "1.0", os: "26.1", report: report).absoluteString
    #expect(url.hasPrefix("https://github.com/BuiHung1612/OpenCleaner/issues/new?title=Crash%3A%20OpenCleaner%201.0&body="))
    #expect(url.count <= 7000 && url.contains("line%2010%20") && !url.contains("line%2041%20"))
    let short = CrashReports.issueURL(version: "1.0", os: "26.1", report: report.split(separator: "\n").prefix(3).joined(separator: "\n"))
    #expect(short.absoluteString.contains("first%203%20lines"))
}
