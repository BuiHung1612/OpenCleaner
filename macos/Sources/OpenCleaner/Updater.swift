//! Self-update from GitHub Releases, same flow as VKeyEngine: check 10 s after launch and then
//! daily (can be turned off), ask, download the release zip, unpack it, and let a small script
//! swap the bundles once this process has quit, then reopen the app.

import AppKit
import Foundation

enum Updater {
    static let repo = "BuiHung1612/OpenCleaner"
    static let asset = "OpenCleaner-macos.zip"
    static let releasesPage = URL(string: "https://github.com/\(repo)/releases/latest")!
    static let autoKey = "checkUpdates"

    static var current: String {
        Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "0"
    }

    /// Only a real .app can replace itself; `swift run` builds skip all of this.
    static var installed: Bool { Bundle.main.bundleURL.pathExtension == "app" }

    @MainActor static func startAutomaticChecks() {
        guard installed else { return }
        Task {
            try? await Task.sleep(for: .seconds(10))
            while true {
                if UserDefaults.standard.object(forKey: autoKey) as? Bool ?? true { await check(manual: false) }
                try? await Task.sleep(for: .seconds(24 * 3600))
            }
        }
    }

    struct Release {
        let version: String
        let download: URL?
    }

    static func latest() async throws -> Release? {
        var request = URLRequest(url: URL(string: "https://api.github.com/repos/\(repo)/releases/latest")!)
        request.setValue("application/vnd.github+json", forHTTPHeaderField: "Accept")
        request.setValue("OpenCleaner", forHTTPHeaderField: "User-Agent")
        let (data, response) = try await URLSession.shared.data(for: request)
        // 404: nothing published yet
        if (response as? HTTPURLResponse)?.statusCode == 404 { return nil }
        let json = JSON(data: data)
        guard let tag = json.tag_name.string else { throw Failure(String(localized: "Unexpected reply from GitHub.")) }
        let url = json.assets.array.first { $0.name.string == asset }?.browser_download_url.string.flatMap(URL.init(string:))
        return Release(version: String(tag.drop { $0 == "v" }), download: url)
    }

    /// "1.0.10" is newer than "1.0.9".
    static func isNewer(_ remote: String, than local: String) -> Bool {
        let r = remote.split(separator: ".").map { Int($0) ?? 0 }, l = local.split(separator: ".").map { Int($0) ?? 0 }
        for i in 0..<max(r.count, l.count) {
            let a = i < r.count ? r[i] : 0, b = i < l.count ? l[i] : 0
            if a != b { return a > b }
        }
        return false
    }

    @MainActor static func check(manual: Bool) async {
        do {
            guard let release = try await latest(), isNewer(release.version, than: current) else {
                if manual { alert(String(localized: "You're up to date"), String(localized: "OpenCleaner \(current) is the newest version.")) }
                return
            }
            let ask = NSAlert()
            ask.messageText = String(localized: "OpenCleaner \(release.version) is available")
            ask.informativeText = String(localized: "You have \(current). Download and install it now? OpenCleaner will restart.")
            ask.addButton(withTitle: String(localized: "Install"))
            ask.addButton(withTitle: String(localized: "Later"))
            guard ask.runModal() == .alertFirstButtonReturn else { return }
            guard let url = release.download else {
                NSWorkspace.shared.open(releasesPage)
                return
            }
            try await install(from: url)
            NSApp.terminate(nil)
        } catch {
            guard manual || error is InstallFailure else { return } // automatic check: stay quiet when offline
            alert(String(localized: "Couldn't update"), error.localizedDescription)
            if error is InstallFailure { NSWorkspace.shared.open(releasesPage) }
        }
    }

    struct Failure: LocalizedError {
        let errorDescription: String?
        init(_ message: String) { errorDescription = message }
    }

    /// Wraps install errors so they are reported even for automatic checks (the user said yes).
    struct InstallFailure: LocalizedError {
        let errorDescription: String?
    }

    /// Downloads and unpacks the new app, checks it, then starts the swap script that waits for us to quit.
    @MainActor static func install(from url: URL) async throws {
        do {
            let app = Bundle.main.bundleURL
            let parent = app.deletingLastPathComponent()
            guard FileManager.default.isWritableFile(atPath: parent.path) else {
                throw Failure(String(localized: "Can't write to \(parent.path). Move OpenCleaner to Applications and try again."))
            }
            let dir = FileManager.default.temporaryDirectory.appendingPathComponent("opencleaner-update")
            try? FileManager.default.removeItem(at: dir)
            try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
            let (downloaded, _) = try await URLSession.shared.download(from: url)
            let zip = dir.appendingPathComponent(asset)
            try FileManager.default.moveItem(at: downloaded, to: zip)
            let newApp = dir.appendingPathComponent("OpenCleaner.app")
            guard run("/usr/bin/ditto", ["-x", "-k", zip.path, dir.path]) != nil,
                  FileManager.default.fileExists(atPath: newApp.appendingPathComponent("Contents/MacOS/OpenCleaner").path)
            else { throw Failure(String(localized: "The download is damaged.")) }
            try verifySameSigner(current: app, new: newApp)

            let script = """
            while kill -0 "$1" 2>/dev/null; do sleep 0.2; done
            rm -rf "$2.old" && mv "$2" "$2.old" && mv "$3" "$2" && rm -rf "$2.old"
            open "$2"
            """
            let p = Process()
            p.executableURL = URL(fileURLWithPath: "/bin/sh")
            p.arguments = ["-c", script, "sh", String(ProcessInfo.processInfo.processIdentifier), app.path, newApp.path]
            try p.run() // a child of launchd once we quit, so it outlives us
        } catch let e as InstallFailure {
            throw e
        } catch {
            throw InstallFailure(errorDescription: error.localizedDescription)
        }
    }

    /// When this app carries a real signing identity, the update must satisfy the same designated
    /// requirement, so a tampered zip can't replace it. Ad-hoc builds have nothing stable to compare.
    static func verifySameSigner(current: URL, new: URL) throws {
        guard let out = run("/usr/bin/codesign", ["-d", "-r-", current.path]),
              let line = out.split(separator: "\n").first(where: { $0.hasPrefix("designated => ") })
        else { return }
        let requirement = String(line.dropFirst("designated => ".count))
        if requirement.hasPrefix("cdhash") { return }
        guard run("/usr/bin/codesign", ["--verify", "--deep", "--strict", "-R", "=" + requirement, new.path]) != nil else {
            throw Failure(String(localized: "The download is not signed by OpenCleaner. It was not installed."))
        }
    }

    /// stdout + stderr of a tool, or nil when it fails.
    @discardableResult
    static func run(_ tool: String, _ args: [String]) -> String? {
        let p = Process()
        p.executableURL = URL(fileURLWithPath: tool)
        p.arguments = args
        let out = Pipe()
        p.standardOutput = out
        p.standardError = out
        guard (try? p.run()) != nil else { return nil }
        let data = out.fileHandleForReading.readDataToEndOfFile()
        p.waitUntilExit()
        return p.terminationStatus == 0 ? String(decoding: data, as: UTF8.self) : nil
    }

    @MainActor static func alert(_ title: String, _ text: String) {
        let a = NSAlert()
        a.messageText = title
        a.informativeText = text
        a.runModal()
    }
}
