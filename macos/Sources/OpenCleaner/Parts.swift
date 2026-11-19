import SwiftUI

/// Listed folders the user opened, their children (from the engine), and the children kept
/// ticked. A folder missing from `ticked` goes whole; present, only those children go.
struct Parts {
    var open: Set<String> = []
    var children: [String: [JSON]] = [:]
    var ticked: [String: Set<String>] = [:]

    /// A plain folder (not an app or another package) can be opened.
    static func canOpen(_ path: String) -> Bool {
        var dir: ObjCBool = false
        return FileManager.default.fileExists(atPath: path, isDirectory: &dir) && dir.boolValue
            && !NSWorkspace.shared.isFilePackage(atPath: path)
    }

    /// Bytes going from `parent`: its own size when whole, else its ticked children.
    func bytes(_ parent: String, whole: Double) -> Double {
        guard let t = ticked[parent] else { return whole }
        return (children[parent] ?? []).filter { t.contains($0.path.string ?? "") }
            .compactMap(\.size.double).reduce(0, +)
    }

    func isPartial(_ parent: String) -> Bool { ticked[parent] != nil }
    func isTicked(_ child: String, in parent: String) -> Bool { ticked[parent]?.contains(child) ?? true }

    /// Ticks or unticks one child of a ticked folder. False when none is left: the caller
    /// unticks the folder itself.
    mutating func toggle(_ child: String, in parent: String) -> Bool {
        let all = Set((children[parent] ?? []).compactMap(\.path.string))
        var t = ticked[parent] ?? all
        if t.contains(child) { t.remove(child) } else { t.insert(child) }
        ticked[parent] = t == all || t.isEmpty ? nil : t
        return !t.isEmpty
    }

    /// Ticking a child of an unticked folder: that child only.
    mutating func only(_ child: String, in parent: String) {
        ticked[parent] = (children[parent]?.count ?? 0) > 1 ? [child] : nil
    }

    mutating func whole(_ parent: String) { ticked[parent] = nil }

    /// Opens or closes `parent`; the first time, `loaded` receives its children.
    mutating func toggleOpen(_ parent: String, loaded: @escaping @MainActor ([JSON]) -> Void) {
        if open.contains(parent) {
            open.remove(parent)
            return
        }
        open.insert(parent)
        guard children[parent] == nil else { return }
        Task { @MainActor in
            var entries: [JSON] = []
            for await event in Engine.run(["op": "space", "path": parent]) where event.event.string == "space" {
                entries = event.entries.array
            }
            loaded(entries)
        }
    }

    /// The `parts` argument of a remove/clean request, for the folders in `chosen`.
    func request(_ chosen: (String) -> Bool) -> [String: [String]] {
        ticked.filter { chosen($0.key) }.mapValues { Array($0) }
    }
}

/// The chevron that opens a folder row.
struct OpenButton: View {
    let open: Bool
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Image(systemName: "chevron.right")
                .rotationEffect(.degrees(open ? 90 : 0))
                .frame(width: 22, height: 22)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .help(String(localized: "Show items"))
    }
}

/// The children of an opened folder, each with its checkbox.
struct PartRows: View {
    let parent: String
    let parts: Parts
    /// the folder itself is ticked (whole or in part)
    let parentOn: Bool
    let toggle: (String) -> Void

    var body: some View {
        if let kids = parts.children[parent] {
            LazyVStack(alignment: .leading, spacing: 0) {
                ForEach(kids, id: \.path.string) { child in row(child) }
                if kids.isEmpty { Text("Empty").font(.system(size: 12)).foregroundStyle(.dim).padding(8) }
            }
        } else {
            HStack(spacing: 8) {
                ProgressView().controlSize(.small)
                Text("Loading…").font(.system(size: 12)).foregroundStyle(.dim)
            }
            .padding(8)
        }
    }

    private func row(_ child: JSON) -> some View {
        let path = child.path.string ?? ""
        let on = parentOn && parts.isTicked(path, in: parent)
        return HStack(spacing: 10) {
            Image(systemName: on ? "checkmark.circle.fill" : "circle").font(.system(size: 14))
            Image(nsImage: fileIcon(path)).resizable().frame(width: 18, height: 18)
            Text(verbatim: child.name.string ?? URL(fileURLWithPath: path).lastPathComponent)
                .font(.system(size: 12)).lineLimit(1).truncationMode(.middle).help(path)
            Spacer()
            Text(verbatim: bytes(child.size.double ?? 0)).font(.system(size: 12)).monospacedDigit().foregroundStyle(.dim)
        }
        .padding(.horizontal, 12).padding(.vertical, 4)
        .contentShape(Rectangle())
        .onTapGesture { toggle(path) }
        .contextMenu { Button(String(localized: "Show in Finder")) { reveal(path) } }
    }
}
