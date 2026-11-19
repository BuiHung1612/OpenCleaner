import QuickLook
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
        .accessibilityLabel(String(localized: "Show items"))
        .accessibilityValue(open ? String(localized: "Open") : "")
    }
}

/// The children of an opened folder. Only the first level has checkboxes: the engine takes
/// direct children of a listed folder as parts, so deeper levels are for looking only.
struct PartRows: View {
    let parent: String
    let parts: Parts
    /// the folder itself is ticked (whole or in part)
    let parentOn: Bool
    var depth = 0
    let toggle: (String) -> Void
    let open: (String) -> Void

    var body: some View {
        if let kids = parts.children[parent] {
            LazyVStack(alignment: .leading, spacing: 0) {
                if depth == 1 && !kids.isEmpty {
                    Text("Only the first level can be ticked; deeper folders are for looking.")
                        .font(.system(size: 11)).foregroundStyle(.dim).padding(.horizontal, 12).padding(.vertical, 2)
                }
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
        let opened = parts.open.contains(path)
        return VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 10) {
                Image(systemName: on ? "checkmark.circle.fill" : "circle").font(.system(size: 14)).opacity(depth == 0 ? 1 : 0)
                Image(nsImage: fileIcon(path)).resizable().frame(width: 18, height: 18)
                Text(verbatim: child.name.string ?? URL(fileURLWithPath: path).lastPathComponent)
                    .font(.system(size: 12)).lineLimit(1).truncationMode(.middle).help(path)
                Spacer()
                Text(verbatim: bytes(child.size.double ?? 0)).font(.system(size: 12)).monospacedDigit().foregroundStyle(.dim)
                if child.is_dir.bool && Parts.canOpen(path) {
                    OpenButton(open: opened) { open(path) }
                } else {
                    Color.clear.frame(width: 22, height: 1)
                }
            }
            .padding(.horizontal, 12).padding(.vertical, 4)
            .checkRow(on) { if depth == 0 { toggle(path) } }
            .peekable(path)
            .contextMenu { FileMenu(path: path) }
            if opened {
                PartRows(parent: path, parts: parts, parentOn: false, depth: depth + 1, toggle: toggle, open: open)
                    .padding(.leading, 24)
            }
        }
    }
}

/// Context menu of a file row.
struct FileMenu: View {
    let path: String

    var body: some View {
        Button(String(localized: "Show in Finder")) { reveal(path) }
        Button(String(localized: "Quick Look")) { Peek.shared.show(path) }
    }
}

/// Quick Look for list rows: Space previews the row last pointed at or clicked, like Finder,
/// and Space again closes it.
@MainActor
final class Peek: ObservableObject {
    static let shared = Peek()
    @Published var url: URL?
    /// not published: hover would redraw every list
    var current: String?
    private var monitor: Any?

    func show(_ path: String) { url = URL(fileURLWithPath: path) }

    /// A key monitor, not SwiftUI focus: the rows are plain views that never take focus.
    func listen() {
        guard monitor == nil else { return }
        monitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { event in
            let space = event.keyCode == 49 && event.modifierFlags.intersection(.deviceIndependentFlagsMask).isEmpty
            return space && MainActor.assumeIsolated({ Peek.shared.space() }) ? nil : event
        }
    }

    /// True when Space was used here.
    private func space() -> Bool {
        // a space typed into a search field stays a space
        guard !(NSApp.keyWindow?.firstResponder is NSText) else { return false }
        if url != nil {
            url = nil
        } else if let current {
            show(current)
        } else {
            return false
        }
        return true
    }
}

private struct QuickLookList: ViewModifier {
    @ObservedObject private var peek = Peek.shared

    func body(content: Content) -> some View {
        content
            .quickLookPreview($peek.url)
            .onAppear { peek.listen() }
            // another page's rows must not answer Space here
            .onDisappear { peek.current = nil }
    }
}

extension View {
    /// The list whose rows can be previewed (see `peekable`).
    func quickLookList() -> some View { modifier(QuickLookList()) }

    /// A row Space previews once the pointer is on it or it was clicked.
    func peekable(_ path: String) -> some View {
        onHover { if $0 { Peek.shared.current = path } }
            .simultaneousGesture(TapGesture().onEnded { Peek.shared.current = path })
    }
}

/// Search and sort of a review list. Rows carry no dates, so size and name only.
struct ListFilter {
    enum Sort { case size, name }
    var query = ""
    var sort = Sort.size

    func matches(_ texts: String...) -> Bool {
        let q = query.trimmingCharacters(in: .whitespaces)
        return q.isEmpty || texts.contains { $0.localizedCaseInsensitiveContains(q) }
    }

    /// Biggest first, or by name the way Finder orders them.
    func sorted<T>(_ rows: [T], name: (T) -> String, size: (T) -> Double) -> [T] {
        switch sort {
        case .size: rows.sorted { size($0) > size($1) }
        case .name: rows.sorted { name($0).localizedStandardCompare(name($1)) == .orderedAscending }
        }
    }
}

/// The glass search capsule. ⌘F puts the cursor in it (one is on screen at a time).
struct SearchField: View {
    let prompt: String
    @Binding var text: String
    @FocusState private var focused: Bool

    var body: some View {
        HStack {
            Image(systemName: "magnifyingglass").foregroundStyle(.dim)
            ZStack(alignment: .leading) {
                if text.isEmpty { Text(verbatim: prompt).foregroundStyle(.dim).allowsHitTesting(false).accessibilityHidden(true) }
                TextField("", text: $text).textFieldStyle(.plain)
                    .focused($focused)
                    .accessibilityLabel(prompt)
                    // Esc in the field clears the search first
                    .onExitCommand { if text.isEmpty { focused = false } else { text = "" } }
            }
            .font(.system(size: 14))
        }
        .padding(.horizontal, 14).padding(.vertical, 8)
        .glassEffect(.clear.tint(.white.opacity(0.07)), in: .capsule)
        .background {
            Button { focused = true } label: { EmptyView() }
                .keyboardShortcut("f", modifiers: .command)
                .opacity(0)
                .accessibilityHidden(true)
        }
    }
}

/// Search field and sort menu above a review list.
struct FilterBar: View {
    @Binding var filter: ListFilter

    var body: some View {
        HStack(spacing: 10) {
            SearchField(prompt: String(localized: "Search by name or path"), text: $filter.query)
            Menu {
                Picker(String(localized: "Sort by"), selection: $filter.sort) {
                    Text("Size").tag(ListFilter.Sort.size)
                    Text("Name").tag(ListFilter.Sort.name)
                }
                .pickerStyle(.inline)
            } label: {
                Label(filter.sort == .size ? String(localized: "Size") : String(localized: "Name"), systemImage: "arrow.up.arrow.down")
            }
            .menuStyle(.button)
            .menuIndicator(.hidden)
            .buttonStyle(.pill)
            .fixedSize()
        }
    }
}
