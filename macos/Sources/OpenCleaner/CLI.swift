//! Everything that knows about the cleaner CLI (currently `mo`). The UI never builds a command line itself: it asks
//! for a command by key from `shared/cli.json`, so a new mo release usually means editing that
//! file, not the Swift code. JSON output is read through the schema-less `JSON` wrapper, so new or
//! missing fields never break decoding.

import AppKit
import CryptoKit
import Foundation

@dynamicMemberLookup
struct JSON {
    let raw: Any?

    init(_ raw: Any?) { self.raw = raw }
    init(data: Data) { raw = try? JSONSerialization.jsonObject(with: data) }

    subscript(key: String) -> JSON { JSON((raw as? [String: Any])?[key]) }
    subscript(dynamicMember key: String) -> JSON { self[key] }

    var array: [JSON] { (raw as? [Any])?.map(JSON.init) ?? [] }
    var string: String? { raw as? String ?? (raw as? NSNumber)?.stringValue }
    var double: Double? { (raw as? NSNumber)?.doubleValue ?? string.flatMap(Double.init) }
    var bool: Bool { (raw as? NSNumber)?.boolValue ?? false }
    var strings: [String] { array.compactMap(\.string) }
}

struct Tool: Identifiable {
    let id: String
    let group: String
    let title: String
    let subtitle: String
    let points: [String]
    let icon: String
    let colors: [String]
    let preview: [String]
    let run: [String]
    let confirm: String?
    let terminal: Bool
}

/// Manifest text is English; translations live in `<lang>.lproj/Localizable.strings`, and an
/// untranslated string from a newer manifest simply shows in English.
func tr(_ key: String) -> String {
    Bundle.main.localizedString(forKey: key, value: nil, table: nil)
}

enum CLI {
    static let manifestOverride = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        .appendingPathComponent("OpenCleaner/cli.json")

    /// First readable manifest wins: user override, app bundle, then the repo copy for `swift run`.
    static let manifestURL: URL? = {
        let repoCopy = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().appendingPathComponent("../../../shared/cli.json").standardized
        return [manifestOverride, Bundle.main.url(forResource: "cli", withExtension: "json"), repoCopy]
            .compactMap { $0 }
            .first { (try? Data(contentsOf: $0)).map { JSON(data: $0).raw != nil } ?? false }
    }()

    static let manifest = manifestURL.flatMap { try? Data(contentsOf: $0) }.map { JSON(data: $0) } ?? JSON(nil)

    static let tools: [Tool] = manifest.tools.array.compactMap { t in
        guard let id = t.id.string, !t.run.strings.isEmpty else { return nil }
        return Tool(
            id: id,
            group: t.group.string ?? "Tools",
            title: tr(t.title.string ?? id),
            subtitle: tr(t.subtitle.string ?? ""),
            points: t.points.strings.map(tr),
            icon: t.icon.string ?? "terminal",
            colors: t.colors.strings,
            preview: t.preview.strings,
            run: t.run.strings,
            confirm: t.confirm.string,
            terminal: t.terminal.bool
        )
    }

    /// Arguments for a manifest command. `{var}` expands to every value in `vars[var]`; a
    /// missing variable expands to nothing (e.g. `analyze --json` without a path).
    static func command(_ key: String, _ vars: [String: [String]] = [:]) -> [String] {
        manifest.commands[key].strings.flatMap { arg in
            guard arg.hasPrefix("{"), arg.hasSuffix("}") else { return [arg] }
            return vars[String(arg.dropFirst().dropLast())] ?? []
        }
    }

    // MARK: binary

    static let pathKey = "moPath"

    /// The engine bundled in the app; for `swift run`, the one legacy/mo/fetch-engine.sh put in build/.
    static let bundledEngine: String? = {
        let macos = URL(fileURLWithPath: #filePath).deletingLastPathComponent().appendingPathComponent("../../build").standardized
        let dev = (try? FileManager.default.contentsOfDirectory(atPath: macos.path))?
            .filter { $0.hasPrefix("engine-") }.sorted().last.map { macos.appendingPathComponent($0).path }
        return [Bundle.main.url(forResource: "engine", withExtension: nil)?.path, dev]
            .compactMap { $0 }
            .map { $0 + "/mole" }
            .first { FileManager.default.isExecutableFile(atPath: $0) }
    }()

    /// Custom path from Settings, then the bundled engine, then a system install.
    static var binary: String? {
        let fm = FileManager.default
        if let custom = UserDefaults.standard.string(forKey: pathKey), fm.isExecutableFile(atPath: custom) { return custom }
        if let bundledEngine { return bundledEngine }
        for dir in manifest.searchPaths.strings {
            for name in manifest.binary.strings {
                let path = (dir as NSString).expandingTildeInPath + "/" + name
                if fm.isExecutableFile(atPath: path) { return path }
            }
        }
        return nil
    }

    /// GUI apps start with a bare PATH; mo shells out to fd, brew, go helpers…
    static var environment: [String: String] {
        var env = ProcessInfo.processInfo.environment
        let dirs = manifest.searchPaths.strings.map { ($0 as NSString).expandingTildeInPath }
            + [binary.map { ($0 as NSString).deletingLastPathComponent }].compactMap { $0 }
            + ["/usr/bin", "/bin", "/usr/sbin", "/sbin"]
        env["PATH"] = (dirs + [env["PATH"] ?? ""]).joined(separator: ":")
        env["TERM"] = "xterm-256color"
        env["COLUMNS"] = "100"
        return env
    }

    struct Missing: LocalizedError {
        var errorDescription: String? { String(localized: "The cleaning engine is missing. Reinstall OpenCleaner.") }
    }

    static func process(_ args: [String]) throws -> Process {
        guard let bin = binary else { throw Missing() }
        let p = Process()
        p.executableURL = URL(fileURLWithPath: bin)
        p.arguments = args
        p.environment = environment
        p.standardInput = FileHandle.nullDevice
        return p
    }

    /// Runs a command to completion and returns stdout. `background` runs it at utility QoS so
    /// speculative work (Space Lens prefetch) never competes with what the user is waiting for.
    static func output(_ args: [String], background: Bool = false) async throws -> Data {
        let p = try process(args)
        p.qualityOfService = background ? .utility : .userInitiated
        let out = Pipe()
        p.standardOutput = out
        p.standardError = FileHandle.nullDevice
        try Task.checkCancellation()
        try p.run()
        // cancelling the Swift task kills the process (mo execs straight into its Go helper)
        return try await withTaskCancellationHandler {
            let data = await Task.detached {
                let data = out.fileHandleForReading.readDataToEndOfFile()
                p.waitUntilExit()
                return data
            }.value
            try Task.checkCancellation()
            return data
        } onCancel: {
            p.terminate()
        }
    }

    /// `persist` keeps the output on disk so the next launch can show it at once (`cached`).
    static func json(_ key: String, _ vars: [String: [String]] = [:], background: Bool = false, persist: Bool = false) async throws -> JSON {
        let args = command(key, vars)
        let data = try await output(args, background: background)
        let json = JSON(data: data)
        if persist, json.raw != nil { try? ResultCache.save(data, for: args) }
        return json
    }

    /// The last persisted output of a command and when it was taken, if any.
    static func cached(_ key: String, _ vars: [String: [String]] = [:]) -> (json: JSON, date: Date)? {
        ResultCache.load(command(key, vars))
    }

    static func version() async -> String? {
        guard let data = try? await output(command("version")) else { return nil }
        let first = String(decoding: data, as: UTF8.self).split(separator: "\n").first ?? ""
        return first.split(separator: " ").last.map(String.init)
    }

    /// For interactive TUIs (selectors, menus) that only a real terminal can drive.
    static func openInTerminal(_ args: [String]) {
        guard let bin = binary else { return }
        let quote = { (s: String) in "'" + s.replacingOccurrences(of: "'", with: "'\\''") + "'" }
        let script = """
        #!/bin/zsh
        export PATH=\(quote(environment["PATH"] ?? ""))
        clear
        \(([bin] + args).map(quote).joined(separator: " "))
        echo
        read -k1 '?\(String(localized: "Press any key to close…"))'
        """
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("opencleaner-\(UUID().uuidString.prefix(8)).command")
        do {
            try script.write(to: url, atomically: true, encoding: .utf8)
            try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: url.path)
            NSWorkspace.shared.open(url)
        } catch {
            NSSound.beep()
        }
    }
}

/// Last good JSON of slow commands (a full-disk map takes ~30 s), one file per command line in
/// ~/Library/Caches. Pages show it immediately, marked as old, while a fresh run replaces it.
enum ResultCache {
    static let dir = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0]
        .appendingPathComponent("OpenCleaner/results")

    static func url(_ args: [String]) -> URL {
        let name = SHA256.hash(data: Data(args.joined(separator: "\u{0}").utf8)).map { String(format: "%02x", $0) }.joined()
        return dir.appendingPathComponent(name + ".json")
    }

    static func save(_ data: Data, for args: [String]) throws {
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        try data.write(to: url(args), options: .atomic)
    }

    static func load(_ args: [String]) -> (json: JSON, date: Date)? {
        let url = url(args)
        guard let data = try? Data(contentsOf: url),
              let date = (try? url.resourceValues(forKeys: [.contentModificationDateKey]))?.contentModificationDate
        else { return nil }
        let json = JSON(data: data)
        return json.raw == nil ? nil : (json, date)
    }
}

/// System status. One-shot reads (launch, focus, reload button) bump `replay` so the dashboard
/// plays its intro animation; the optional `--watch` stream updates values quietly.
@MainActor
final class StatusStream: ObservableObject {
    @Published var snapshot = JSON(nil)
    @Published var error: String?
    @Published private(set) var replay = 0
    @Published private(set) var loading = false
    @Published private(set) var updatedAt: Date?
    private var process: Process?
    private var lastRead = Date.distantPast

    /// A single read costs about a second of collector CPU, so focus-driven reads pass `ifOlderThan`.
    func refresh(ifOlderThan age: TimeInterval = 0) {
        guard !loading, Date().timeIntervalSince(lastRead) >= age else { return }
        loading = true
        Task {
            do {
                let json = try await CLI.json("statusOnce")
                snapshot = json
                replay += 1
                updatedAt = Date()
                error = nil
            } catch {
                self.error = error.localizedDescription
            }
            loading = false
            lastRead = Date()
        }
    }

    func start(every seconds: Int) {
        guard process == nil else { return }
        do {
            let p = try CLI.process(CLI.command("status", ["interval": ["\(seconds)s"]]))
            let out = Pipe()
            p.standardOutput = out
            p.standardError = FileHandle.nullDevice
            nonisolated(unsafe) var buffer = Data() // readabilityHandler calls are serial per handle
            out.fileHandleForReading.readabilityHandler = { [weak self] h in
                let data = h.availableData
                if data.isEmpty { h.readabilityHandler = nil; return }
                buffer.append(data)
                guard let nl = buffer.lastIndex(of: 0x0A) else { return }
                let line = buffer[..<nl].split(separator: 0x0A).last.map { Data($0) }
                buffer.removeSubrange(...nl)
                guard let line else { return }
                let json = JSON(data: line)
                Task { @MainActor in
                    self?.snapshot = json
                    self?.updatedAt = Date()
                }
            }
            try p.run()
            process = p
            error = nil
        } catch {
            self.error = error.localizedDescription
        }
    }

    func stop() {
        process?.terminate()
        process = nil
    }
}

/// A command run inside a pseudo-terminal (`script`), so mo's sudo / y-n prompts still work and
/// the user can answer them from the console sheet.
@MainActor
final class Session: ObservableObject, Identifiable {
    let id = UUID()
    let title: String
    let args: [String]
    @Published private(set) var text = ""
    @Published private(set) var running = false
    @Published private(set) var exitCode: Int32?
    /// The closing report mo prints between two `=====` rules, if any.
    @Published private(set) var summary: [String] = []
    var isPreview = false
    /// bytes after the last newline: the line still being drawn (spinner, prompt)
    private var pending = Data()
    /// finished lines, already cleaned; never re-processed
    private var committed = ""
    private var flushScheduled = false
    private var process: Process?
    private var input: Pipe?

    init(title: String, args: [String]) {
        self.title = title
        self.args = args
    }

    func start() {
        guard process == nil else { return }
        do {
            let inner = try CLI.process(args)
            let p = Process()
            p.executableURL = URL(fileURLWithPath: "/usr/bin/script")
            p.arguments = ["-q", "/dev/null", inner.executableURL?.path ?? ""] + args
            p.environment = inner.environment
            let out = Pipe(), inp = Pipe()
            p.standardOutput = out
            p.standardError = out
            p.standardInput = inp
            out.fileHandleForReading.readabilityHandler = { [weak self] h in
                let data = h.availableData
                if data.isEmpty {
                    h.readabilityHandler = nil
                    Task { @MainActor in self?.flush() }
                    return
                }
                Task { @MainActor in self?.append(data) }
            }
            p.terminationHandler = { [weak self] p in
                Task { @MainActor in
                    self?.flush()
                    self?.running = false
                    self?.exitCode = p.terminationStatus
                }
            }
            try p.run()
            process = p
            input = inp
            running = true
        } catch {
            text = error.localizedDescription
            exitCode = -1
        }
    }

    static func summary(of text: String) -> [String] {
        guard text.contains("=====") else { return [] }
        let lines = text.split(separator: "\n").map { $0.trimmingCharacters(in: .whitespaces) }
        let rules = lines.indices.filter { lines[$0].hasPrefix("=====") }
        guard rules.count >= 2, let end = rules.last else { return [] }
        let start = rules[rules.count - 2]
        // drop CLI hints such as "Run without --dry-run", the app has its own button for that
        return lines[(start + 1)..<end].filter { !$0.isEmpty && !$0.contains("--") }
    }

    func send(_ line: String) {
        input?.fileHandleForWriting.write(Data((line + "\n").utf8))
    }

    func interrupt() {
        input?.fileHandleForWriting.write(Data([0x03]))
    }

    /// Only the unfinished tail is cleaned per chunk (a `\n` byte never sits inside a UTF-8 char,
    /// so cutting there is safe), and the UI gets at most ~12 updates a second however fast mo spins.
    private func append(_ chunk: Data) {
        pending.append(chunk)
        if let nl = pending.lastIndex(of: 0x0A) {
            committed += Self.clean(String(decoding: pending[...nl], as: UTF8.self))
            pending.removeSubrange(...nl)
        }
        guard !flushScheduled else { return }
        flushScheduled = true
        Task {
            try? await Task.sleep(for: .milliseconds(80))
            flush()
        }
    }

    private func flush() {
        flushScheduled = false
        let new = committed + Self.clean(String(decoding: pending, as: UTF8.self))
        let shown = Self.tail(new, lines: Self.maxLines)
        guard shown != text else { return }
        text = shown
        summary = Self.summary(of: new)
    }

    /// The log view re-lays out all of its text on every update, so it only keeps the end.
    static let maxLines = 2000

    /// Last `n` lines of `s`, with a "…" line in front when earlier ones were dropped.
    nonisolated static func tail(_ s: String, lines n: Int) -> String {
        var count = 0
        var i = s.utf8.endIndex
        while i > s.utf8.startIndex {
            let j = s.utf8.index(before: i)
            if s.utf8[j] == 0x0A {
                count += 1
                if count > n { return "…\n" + s[i...] }
            }
            i = j
        }
        return s
    }

    /// Drops ANSI escapes and keeps only what is left after the last `\r` of a line (spinners).
    static func clean(_ s: String) -> String {
        let noAnsi = s
            .replacingOccurrences(of: "\u{1B}\\][^\u{07}\u{1B}]*(\u{07}|\u{1B}\\\\)", with: "", options: .regularExpression)
            .replacingOccurrences(of: "\u{1B}\\[[0-9;?]*[ -/]*[@-~]", with: "", options: .regularExpression)
            .replacingOccurrences(of: "\u{1B}[()][0-9A-Za-z]|\u{1B}[=>78]", with: "", options: .regularExpression)
            .replacingOccurrences(of: "\r\n", with: "\n")
        return noAnsi.split(separator: "\n", omittingEmptySubsequences: false)
            .map { $0.split(separator: "\r").last.map(String.init) ?? "" }
            .joined(separator: "\n")
    }
}
