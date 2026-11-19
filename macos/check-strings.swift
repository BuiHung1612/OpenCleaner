// Lists String(localized: "…") and Text("…") keys in macos/Sources that a language's
// Localizable.strings lacks. Interpolations count as one placeholder, whatever its format
// (%@, %lld, …). Reports only; always exits 0.   Usage: swift macos/check-strings.swift [lang]
// (build.sh runs the compiler-exact check on a full build; this one needs no build.)

import Foundation

let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
let lang = CommandLine.arguments.dropFirst().first ?? "vi"

/// The key of the string literal starting after `i` (just past its opening quote), with each
/// interpolation as "%@"; nil for multi-line or raw literals.
func literal(_ s: [Character], from start: Int) -> String? {
    var key = "", i = start
    if i + 1 < s.count, s[i] == "\"", s[i + 1] == "\"" { return nil } // """ or empty
    while i < s.count {
        let c = s[i]
        if c == "\"" { return key }
        if c == "\n" { return nil }
        if c == "\\", i + 1 < s.count {
            let n = s[i + 1]
            if n == "(" { // skip the interpolation, including strings and parens inside it
                var depth = 1, inString = false
                i += 2
                while i < s.count, depth > 0 {
                    if s[i] == "\\" { i += 1 } else if s[i] == "\"" { inString.toggle() } else if !inString {
                        if s[i] == "(" { depth += 1 } else if s[i] == ")" { depth -= 1 }
                    }
                    i += 1
                }
                key += "%@"
                continue
            }
            key.append(["n": "\n", "t": "\t"][n] ?? n)
            i += 2
            continue
        }
        key.append(c)
        i += 1
    }
    return nil
}

var used = Set<String>()
let files = FileManager.default.enumerator(at: root.appendingPathComponent("Sources"), includingPropertiesForKeys: nil)!
for case let file as URL in files where file.pathExtension == "swift" {
    let text = Array(try! String(contentsOf: file, encoding: .utf8))
    for opener in [Array("String(localized: \""), Array("Text(\"")] {
        var i = 0
        while i + opener.count <= text.count {
            if Array(text[i..<i + opener.count]) == opener, i == 0 || !(text[i - 1].isLetter || text[i - 1].isNumber) {
                if let key = literal(text, from: i + opener.count), !key.isEmpty { used.insert(key) }
            }
            i += 1
        }
    }
}

// Text the app shows through `tr()`, so the scan above never sees it: the engine's rule,
// group and maintenance titles and notes, the kinds of pick it reports, and the sidebar
// manifest (shared/tools.json).
func captures(_ pattern: String, in file: String) -> [String] {
    guard let text = try? String(contentsOf: root.appendingPathComponent(file), encoding: .utf8) else {
        print("warning: cannot read \(file)")
        return []
    }
    let re = try! NSRegularExpression(pattern: pattern, options: .anchorsMatchLines)
    return re.matches(in: text, range: NSRange(text.startIndex..., in: text)).compactMap {
        Range($0.range(at: 1), in: text).map { unescape(String(text[$0])) }
    }
}

/// `\"`, `\\` and Rust's `\u{201c}` as the characters they stand for.
func unescape(_ s: String) -> String {
    var out = "", i = s.startIndex
    while i < s.endIndex {
        let c = s[i]
        i = s.index(after: i)
        guard c == "\\", i < s.endIndex else { out.append(c); continue }
        if s[i] == "u", let open = s[i...].firstIndex(of: "{"), let close = s[open...].firstIndex(of: "}"),
           let code = UInt32(s[s.index(after: open)..<close], radix: 16), let scalar = Unicode.Scalar(code) {
            out.unicodeScalars.append(scalar)
            i = s.index(after: close)
        } else {
            out.append(s[i])
            i = s.index(after: i)
        }
    }
    return out
}

let engineText = captures(#"^(?:title|note) = "(.*)"$"#, in: "../engine/rules/macos.toml")
    + captures(#"^\s*(?:title|note): "(.*)",$"#, in: "../engine/src/maintenance.rs")
    + captures(#"^\s*\("~/[^"]*", "([^"]+)"\),$"#, in: "../engine/src/apps.rs")
    + captures(#"=>\s*(?:\{\s*)?Some\("([^"]+)"\)"#, in: "../engine/src/finders.rs")
used.formUnion(engineText)
let manifest = (try? JSONSerialization.jsonObject(with: Data(contentsOf: root.appendingPathComponent("../shared/tools.json")))) as? [String: Any]
for tool in manifest?["tools"] as? [[String: Any]] ?? [] {
    for key in ["title", "subtitle", "group"] { if let s = tool[key] as? String { used.insert(s) } }
    used.formUnion(tool["points"] as? [String] ?? [])
}

let placeholder = try! NSRegularExpression(pattern: #"%(\d+\$)?[-+ 0#]*\d*(\.\d+)?(ll|l|h)?[@dDuUxXfeEgGcsS]"#)
func normalized(_ key: String) -> String {
    placeholder.stringByReplacingMatches(in: key, range: NSRange(key.startIndex..., in: key), withTemplate: "%@")
}
let stringsFile = root.appendingPathComponent("Resources/\(lang).lproj/Localizable.strings")
let table = (try? PropertyListSerialization.propertyList(from: Data(contentsOf: stringsFile), format: nil)) as? [String: String] ?? [:]
let have = Set(table.keys.map(normalized))
let missing = used.filter { !have.contains(normalized($0)) }.sorted()

let ci = ProcessInfo.processInfo.environment["GITHUB_ACTIONS"] == "true"
for key in missing {
    let shown = key.replacingOccurrences(of: "\n", with: "\\n")
    print(ci ? "::warning title=Missing \(lang) string::\(shown)" : "missing: \(shown)")
}
print("\(missing.count) of \(used.count) keys missing from \(lang).lproj/Localizable.strings")
