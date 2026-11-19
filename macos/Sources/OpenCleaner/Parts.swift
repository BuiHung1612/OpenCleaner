import QuickLook
import SwiftUI

/// Listed folders the user opened (at any depth), their children (from the engine), and
/// the children kept ticked. A ticked folder missing from `ticked` goes whole; present,
/// only those children go. Only ticked folders that are partly ticked have an entry.
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

    private func kids(_ folder: String) -> [String] { (children[folder] ?? []).compactMap(\.path.string) }

    /// Bytes going from `folder`: its own size when whole, else what goes of its ticked children.
    func bytes(_ folder: String, whole: Double) -> Double {
        guard isPartial(folder) else { return whole }
        return (children[folder] ?? []).filter { isTicked($0.path.string ?? "", in: folder) }
            .map { bytes($0.path.string ?? "", whole: $0.size.double ?? 0) }.reduce(0, +)
    }

    /// Some of what is inside `folder`, at any depth, is unticked.
    func isPartial(_ folder: String) -> Bool {
        ticked[folder] != nil || ticked.keys.contains { $0.hasPrefix(folder + "/") }
    }

    func isTicked(_ child: String, in folder: String) -> Bool { ticked[folder]?.contains(child) ?? true }

    /// Ticks or unticks `item` (with all it holds), somewhere inside the listed folder
    /// `root`, which is ticked or not (`rootOn`). False when nothing of `root` is left
    /// ticked: the caller unticks `root` itself.
    mutating func toggle(_ item: String, root: String, rootOn: Bool) -> Bool {
        guard item.hasPrefix(root + "/") else { return rootOn }
        // the folders from root down to item's parent, and the next step from each
        var chain = [root]
        for name in item.dropFirst(root.count + 1).split(separator: "/").dropLast() {
            chain.append(chain[chain.count - 1] + "/" + name)
        }
        let steps = Array(chain.dropFirst()) + [item]
        var on = rootOn
        for (folder, next) in zip(chain, steps) { on = on && isTicked(next, in: folder) }
        whole(item)
        if on {
            // untick it; a folder left with nothing ticked is unticked in turn
            for (folder, next) in zip(chain, steps).reversed() {
                var t = ticked[folder] ?? Set(kids(folder))
                t.remove(next)
                if !t.isEmpty {
                    ticked[folder] = t
                    return true
                }
                whole(folder)
            }
            return false
        }
        // tick it, and the way down to it: a folder that was off gets this one only
        on = rootOn
        for (folder, next) in zip(chain, steps) {
            let wasOn = on
            on = on && isTicked(next, in: folder)
            var t = wasOn ? ticked[folder] ?? Set(kids(folder)) : []
            t.insert(next)
            ticked[folder] = t == Set(kids(folder)) ? nil : t
        }
        return true
    }

    /// `folder` goes whole (or not at all): forget the ticks inside it.
    mutating func whole(_ folder: String) {
        ticked = ticked.filter { $0.key != folder && !$0.key.hasPrefix(folder + "/") }
    }

    /// What goes of a partly ticked `folder`: the deepest fully ticked items inside it.
    func leaves(_ folder: String) -> [String] {
        kids(folder).filter { isTicked($0, in: folder) }.flatMap { isPartial($0) ? leaves($0) : [$0] }
    }

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

    /// The `parts` argument of a remove/clean request, for the listed folders in `chosen`
    /// that go in part.
    func request(_ chosen: some Sequence<String>) -> [String: [String]] {
        Dictionary(chosen.filter(isPartial).map { ($0, leaves($0)) }, uniquingKeysWith: { a, _ in a })
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

/// The children of an opened folder, with a checkbox at every depth.
struct PartRows: View {
    let parent: String
    let parts: Parts
    /// the folder itself is ticked (whole or in part)
    let parentOn: Bool
    let toggle: (String) -> Void
    let open: (String) -> Void

    var body: some View {
        if let kids = parts.children[parent] {
            LazyVStack(alignment: .leading, spacing: 0) {
                ForEach(kids, id: \.path.string) { child in row(child) }
                if kids.isEmpty { Text("Empty").font(.app(12)).foregroundStyle(.dim).padding(8) }
            }
        } else {
            HStack(spacing: 8) {
                ProgressView().controlSize(.small)
                Text("Loading…").font(.app(12)).foregroundStyle(.dim)
            }
            .padding(8)
        }
    }

    private func row(_ child: JSON) -> some View {
        let path = child.path.string ?? ""
        let on = parentOn && parts.isTicked(path, in: parent)
        let partial = on && parts.isPartial(path)
        let opened = parts.open.contains(path)
        return VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 10) {
                Image(systemName: partial ? "minus.circle.fill" : on ? "checkmark.circle.fill" : "circle").font(.app(14))
                Image(nsImage: fileIcon(path)).resizable().frame(width: 18, height: 18)
                Text(verbatim: child.name.string ?? URL(fileURLWithPath: path).lastPathComponent)
                    .font(.app(12)).lineLimit(1).truncationMode(.middle).help(path)
                Spacer()
                Text(verbatim: bytes(child.size.double ?? 0)).font(.app(12)).monospacedDigit().foregroundStyle(.dim)
                if child.is_dir.bool && Parts.canOpen(path) {
                    OpenButton(open: opened) { open(path) }
                } else {
                    Color.clear.frame(width: 22, height: 1)
                }
            }
            .padding(.horizontal, 12).padding(.vertical, 4)
            .checkRow(on) { toggle(path) }
            .peekable(path)
            .contextMenu { FileMenu(path: path) }
            if opened {
                PartRows(parent: path, parts: parts, parentOn: on, toggle: toggle, open: open)
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
            .font(.app(14))
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
