//! Crash reports without a service: on launch, if macOS wrote a new OpenCleaner crash report
//! since the last one we saw, offer to open a prefilled GitHub issue (the user reviews and posts it).

import AppKit
import Foundation

enum CrashReports {
    static let seenKey = "lastCrashReportSeen"
    static let dir = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Logs/DiagnosticReports")

    @MainActor static func offer() {
        guard Updater.installed else { return }
        let key = URLResourceKey.contentModificationDateKey
        let newest = ((try? FileManager.default.contentsOfDirectory(at: dir, includingPropertiesForKeys: [key])) ?? [])
            .filter { $0.lastPathComponent.hasPrefix("OpenCleaner") && $0.pathExtension == "ips" }
            .compactMap { url in (try? url.resourceValues(forKeys: [key]))?.contentModificationDate.map { (url, $0) } }
            .max { $0.1 < $1.1 }
        let seen = UserDefaults.standard.object(forKey: seenKey) as? Date ?? .distantPast
        guard let (report, date) = newest, date > seen else { return }
        UserDefaults.standard.set(date, forKey: seenKey)

        let ask = NSAlert()
        ask.messageText = String(localized: "OpenCleaner quit unexpectedly last time")
        ask.informativeText = String(localized: "Report it on GitHub? You can review the crash details before posting.")
        ask.addButton(withTitle: String(localized: "Report on GitHub"))
        ask.addButton(withTitle: String(localized: "Later"))
        guard ask.runModal() == .alertFirstButtonReturn else { return }
        let text = (try? String(contentsOf: report, encoding: .utf8)) ?? ""
        NSWorkspace.shared.open(issueURL(version: Updater.current, os: ProcessInfo.processInfo.operatingSystemVersionString, report: text))
    }

    /// A new-issue link with title and body filled in: versions plus the report's first 40 lines,
    /// fewer if the link would pass `limit` characters (GitHub and browsers cut long URLs).
    static func issueURL(version: String, os: String, report: String, limit: Int = 7000) -> URL {
        let allowed = CharacterSet.alphanumerics.union(CharacterSet(charactersIn: "-._~"))
        func enc(_ s: String) -> String { s.addingPercentEncoding(withAllowedCharacters: allowed) ?? "" }
        let base = "https://github.com/\(Updater.repo)/issues/new?title=" + enc("Crash: OpenCleaner \(version)") + "&body="
        var lines = Array(report.split(separator: "\n", omittingEmptySubsequences: false).prefix(40))
        while true {
            let body = "OpenCleaner \(version), macOS \(os)\n\nCrash report (first \(lines.count) lines):\n```\n"
                + lines.joined(separator: "\n") + "\n```\n"
            let url = base + enc(body)
            if url.count <= limit || lines.isEmpty { return URL(string: url)! }
            lines.removeLast()
        }
    }
}
