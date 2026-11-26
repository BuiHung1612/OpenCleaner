//! Small shared pieces: the schema-less JSON reader for engine replies, the sidebar tools
//! (shared/tools.json) and the on-disk cache of earlier results.

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

/// A sidebar tool from `shared/tools.json`. What it does lives in the engine; this is only
/// how it looks.
struct Tool: Identifiable {
    let id: String
    let group: String
    let title: String
    let subtitle: String
    let points: [String]
    let icon: String
    let colors: [String]
}

/// Tool text is English; translations live in `<lang>.lproj/Localizable.strings`, and an
/// untranslated string simply shows in English.
func tr(_ key: String) -> String {
    Bundle.main.localizedString(forKey: key, value: nil, table: nil)
}

enum Tools {
    /// The app bundle's copy, or the repository's for `swift run`.
    static let all: [Tool] = {
        let repoCopy = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().appendingPathComponent("../../../shared/tools.json").standardized
        let url = Bundle.main.url(forResource: "tools", withExtension: "json") ?? repoCopy
        let manifest = (try? Data(contentsOf: url)).map { JSON(data: $0) } ?? JSON(nil)
        return manifest.tools.array.compactMap { t in
            guard let id = t.id.string else { return nil }
            return Tool(
                id: id,
                group: t.group.string ?? "Tools",
                title: tr(t.title.string ?? id),
                subtitle: tr(t.subtitle.string ?? ""),
                points: t.points.strings.map(tr),
                icon: t.icon.string ?? "wrench.and.screwdriver",
                colors: t.colors.strings
            )
        }
    }()
}

/// Last good JSON of slow commands (a full-disk map takes ~30 s), one file per command line in
/// ~/Library/Caches. Pages show it immediately, marked as old, while a fresh run replaces it.
enum ResultCache {
    private static let io = DispatchQueue(label: "OpenCleaner.result-cache", qos: .utility)

    static func read(_ args: [String]) async -> (json: JSON, date: Date)? {
        await withCheckedContinuation { reply in
            io.async { reply.resume(returning: load(args)) }
        }
    }

    static func write(_ json: JSON, for args: [String]) async {
        await withCheckedContinuation { reply in
            io.async {
                if let raw = json.raw, let data = try? JSONSerialization.data(withJSONObject: raw) {
                    try? save(data, for: args)
                }
                reply.resume()
            }
        }
    }

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
