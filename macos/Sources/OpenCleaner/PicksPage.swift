import SwiftUI

/// A pick list from the engine (installers, project build folders, an app and its
/// leftovers): find → pick → act. Nothing is touched until `act`, and the engine checks
/// every item again right before.
@MainActor @Observable
final class PicksModel {
    enum Phase { case idle, finding, review, acting, done }

    private(set) var phase = Phase.idle
    private(set) var list = JSON(nil) {
        // parsed once here, not on every read: rows, filters and sums index into them
        didSet {
            items = list.items.array
            groups = Self.grouped(items)
        }
    }
    private(set) var items: [JSON] = []
    /// Items in list order, grouped by what they belong to (an app, a project…).
    private(set) var groups: [(name: String, indexes: [Int])] = []
    var selected: Set<Int> = []
    var parts = Parts()
    private(set) var outcome = JSON(nil)
    private(set) var progress = JSON(nil)
    private(set) var error: String?
    @ObservationIgnored private var job: Task<Void, Never>?

    var selectedBytes: Double {
        selected.filter(items.indices.contains).map { parts.bytes(path($0), whole: items[$0].bytes.double ?? 0) }
            .reduce(0, +)
    }

    func path(_ i: Int) -> String { items.indices.contains(i) ? items[i].path.string ?? "" : "" }

    func toggle(_ i: Int) {
        if selected.contains(i) { selected.remove(i) } else { selected.insert(i) }
        parts.whole(path(i))
    }

    /// Ticks or unticks something at any depth inside item `i`.
    func toggle(child: String, of i: Int) {
        if parts.toggle(child, root: path(i), rootOn: selected.contains(i)) {
            selected.insert(i)
        } else {
            selected.remove(i)
        }
    }

    func toggleOpen(_ p: String) {
        parts.toggleOpen(p) { [weak self] kids in self?.parts.children[p] = kids }
    }

    static func grouped(_ items: [JSON]) -> [(name: String, indexes: [Int])] {
        var order: [String] = []
        var byGroup: [String: [Int]] = [:]
        for (i, item) in items.enumerated() {
            let g = item.group.string ?? ""
            if byGroup[g] == nil { order.append(g) }
            byGroup[g, default: []].append(i)
        }
        return order.map { ($0, byGroup[$0] ?? []) }
    }

    func find(_ request: [String: Any]) {
        job?.cancel()
        phase = .finding
        list = JSON(nil)
        parts = Parts()
        error = nil
        job = Task {
            var found = JSON(nil)
            for await event in Engine.run(request) {
                switch event.event.string {
                case "picks": found = event
                case "error": error = event.error.string
                default: break
                }
            }
            // stopped, or replaced by a newer search
            guard !Task.isCancelled else { return }
            guard found.raw != nil else {
                if error == nil { error = String(localized: "The search did not finish.") }
                phase = .idle
                return
            }
            list = found
            selected = Set(items.indices.filter { items[$0].preselect.bool })
            phase = .review
        }
    }

    func act() {
        guard let plan = list.plan.double, !selected.isEmpty else { return }
        job?.cancel()
        phase = .acting
        progress = JSON(nil)
        job = Task {
            var final = JSON(nil)
            // folders opened with only some children ticked: those children only
            let byPath = Dictionary(selected.map { (path($0), $0) }, uniquingKeysWith: { a, _ in a })
            let chosen = parts.request(byPath.keys)
            let request: [String: Any] = [
                "op": "remove", "plan": Int(plan), "items": selected.sorted(),
                "parts": Dictionary(uniqueKeysWithValues: chosen.compactMap { key, kids in byPath[key].map { ("\($0)", kids) } }),
            ]
            var shown = Date.distantPast
            for await event in Engine.run(request) {
                switch event.event.string {
                case "progress" where Date().timeIntervalSince(shown) >= JunkModel.progressInterval:
                    progress = event
                    shown = Date()
                case "removed": final = event
                default: break
                }
            }
            if let message = final.error.string { error = message }
            outcome = final
            phase = .done
        }
    }

    func stop() {
        job?.cancel()
        job = nil
        if phase == .finding { phase = .idle }
    }

    func reset() {
        stop()
        phase = .idle
        list = JSON(nil)
        outcome = JSON(nil)
        selected = []
        parts = Parts()
        error = nil
    }
}

/// What a pick is, in the user's language. The engine sends the parts (`kind`, `age_days`,
/// `idle_days`, `place`, `flags`); `detail` is its English sentence, used when there are none.
enum PickText {
    static func detail(_ item: JSON) -> String {
        let flags = Set(item.flags.strings)
        if flags.contains("dot") { return hidden(item, flags) }
        if let days = item.idle_days.double {
            return String(localized: "Not opened for \(Int(days)) days")
        }
        if let days = item.age_days.double, let kind = item.kind.string {
            if Int(days) == 0 {
                return kind == "screenshot" ? String(localized: "Screenshot · today") : String(localized: "\(kind) · today")
            }
            return kind == "screenshot"
                ? String(localized: "Screenshot · \(Int(days)) days old")
                : String(localized: "\(kind) · \(Int(days)) days old")
        }
        if let place = item.place.string {
            if flags.contains("kept") { return String(localized: "\(place) · oldest, kept") }
            // an APFS clone: its blocks are shared, so the Trash gets nothing back
            return item.bytes.double == 0
                ? place + " · " + String(localized: "shares its data with the other copy: removing it frees no space")
                : place
        }
        if let kind = item.kind.string {
            var text = tr(kind)
            if flags.contains("git_unknown") { text += " · " + String(localized: "git status unknown") }
            if flags.contains("active") { text += " · " + String(localized: "active this week") }
            return text
        }
        let text = tr(item.detail.string ?? "")
        return flags.contains("shared") ? text + " · " + String(localized: "also used by the other copy of this app") : text
    }

    /// A hidden home item: what it holds, whether its app is still there, when anything in
    /// it last changed.
    private static func hidden(_ item: JSON, _ flags: Set<String>) -> String {
        let owner = item.kind.string ?? ""
        var parts: [String] = []
        if flags.contains("inner") {
            parts.append(owner)
            parts.append(String(localized: "\(item.place.string ?? "") is newer"))
        }
        if flags.contains("backup") { parts.append(String(localized: "Backup of \(owner)")) }
        if flags.contains("rebuilt") { parts.append(String(localized: "zsh rebuilds it when needed")) }
        if flags.contains("cache") { parts.append(String(localized: "\(owner) cache, downloaded again when needed")) }
        if flags.contains("tool") { parts.append(String(localized: "\(owner) installation")) }
        if flags.contains("data") { parts.append(String(localized: "\(owner) settings and data")) }
        if flags.contains("missing") { parts.append(String(localized: "\(owner) is not installed")) }
        let days = Int(item.changed_days.double ?? 0)
        if flags.contains("inner") {
            parts.append(days == 0 ? String(localized: "installed today") : String(localized: "installed \(days) days ago"))
        } else {
        parts.append(days == 0 ? String(localized: "last used today")
                     : days == 1 ? String(localized: "last used yesterday")
                     : String(localized: "last used \(days) days ago"))
        }
        if flags.contains("newest") { parts.append(String(localized: "newest, not ticked")) }
        return parts.joined(separator: " · ")
    }

    /// A group's header: a duplicate set ("a.zip · 2 copies" from the engine) says how many copies it has.
    static func group(_ name: String, first: JSON?) -> String {
        if let first, first.flags.strings.contains("inner") {
            return String(localized: "Old versions: \(first.kind.string ?? "")")
        }
        if first?.flags.strings.contains("dot") == true { return tr(name) }
        guard let copies = first?.copies.double, let cut = name.range(of: " · ", options: .backwards) else { return name }
        return String(localized: "\(String(name[..<cut.lowerBound])) · \(Int(copies)) copies")
    }
}

/// The checkbox list of a pick list, grouped.
struct PicksList: View {
    let model: PicksModel
    @State private var filter = ListFilter()
    /// the one section shown (nil: all)
    @State private var only: String?

    var body: some View {
        VStack(spacing: 10) {
            if model.phase == .review {
                if let error = model.error { ErrorBanner(message: error) }
                if !model.items.isEmpty {
                    FilterBar(filter: $filter)
                    // not for duplicates, whose sections are sets of copies (hundreds of them)
                    HStack(alignment: .center, spacing: 10) {
                        if model.groups.count <= 20 { GroupChips(chips: chips, only: $only) }
                        Spacer(minLength: 0)
                        let rows = shown.flatMap(\.indexes)
                        SelectAllButton(
                            allOn: !rows.isEmpty && rows.allSatisfy(model.selected.contains),
                            risky: rows.filter { !model.selected.contains($0) && model.items[$0].preselect.bool != true }.count,
                            select: { model.selected.formUnion(rows) },
                            deselect: { model.selected.subtract(rows) })
                    }
                }
            }
            list
        }
        // a new list has other sections
        .onChange(of: model.groups.map(\.name)) { if let o = only, !model.groups.contains(where: { $0.name == o }) { only = nil } }
    }

    private var chips: [GroupChips.Chip] {
        model.groups.filter { !$0.name.isEmpty }.map { g in
            GroupChips.Chip(id: g.name, title: PickText.group(g.name, first: g.indexes.first.map { model.items[$0] }),
                            count: g.indexes.count, bytes: g.indexes.reduce(0) { $0 + (model.items[$1].bytes.double ?? 0) })
        }
    }

    /// Groups keep the engine's order; the filter picks and orders the rows inside each.
    private var shown: [(name: String, indexes: [Int])] {
        let items = model.items
        return model.groups.filter { only == nil || $0.name == only }.compactMap { group in
            let rows = filter.sorted(
                group.indexes.filter { filter.matches(items[$0].name.string ?? "", items[$0].path.string ?? "") },
                name: { items[$0].name.string ?? "" }, size: { items[$0].bytes.double ?? 0 })
            return rows.isEmpty ? nil : (group.name, rows)
        }
    }

    private var list: some View {
        let shown = shown
        return ScrollView {
            LazyVStack(alignment: .leading, spacing: 14) {
                ForEach(shown, id: \.name) { group in
                    // rows are keyed by the item's path, and indexes are checked against the
                    // current list: a new list (rescan, Start Over, after removing) can arrive
                    // while SwiftUI still holds rows built from the old one
                    let rows = group.indexes.filter(model.items.indices.contains)
                        .map { (index: $0, key: model.items[$0].path.string ?? "\($0)") }
                    VStack(alignment: .leading, spacing: 2) {
                        if !group.name.isEmpty {
                            Text(verbatim: PickText.group(group.name, first: rows.first.map { model.items[$0.index] }))
                                .font(.app(13, weight: .semibold)).foregroundStyle(.dim)
                                .lineLimit(1).truncationMode(.head).padding(.leading, 12)
                                .accessibilityAddTraits(.isHeader)
                        }
                        // the key is computed here, with the list it came from: the row body may
                        // run again after the list changed, and only reads through the guarded row()
                        ForEach(rows, id: \.key) { r in row(r.index) }
                    }
                }
                if shown.isEmpty {
                    if filter.query.isEmpty {
                        EmptyState(symbol: "checkmark.circle", title: String(localized: "Nothing found."),
                                   message: String(localized: "There is nothing here to remove."))
                    } else {
                        EmptyState(symbol: "magnifyingglass", title: String(localized: "No matches"),
                                   message: String(localized: "Nothing found matches “\(filter.query)”."))
                    }
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(12)
        }
        .brightScroller()
        .glassPanel()
        .quickLookList()
    }

    /// A row re-renders on its own when the list changes, possibly before its parent drops
    /// it: an index past the new list draws nothing instead of crashing.
    @ViewBuilder private func row(_ i: Int) -> some View {
        if model.items.indices.contains(i) { rowBody(i) }
    }

    private func rowBody(_ i: Int) -> some View {
        let item = model.items[i]
        let path = item.path.string ?? ""
        let on = model.selected.contains(i)
        let partial = on && model.parts.isPartial(path)
        let open = model.parts.open.contains(path)
        return VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 12) {
                Image(systemName: partial ? "minus.circle.fill" : on ? "checkmark.circle.fill" : "circle").font(.app(17))
                Image(nsImage: fileIcon(path)).resizable().frame(width: 28, height: 28)
                VStack(alignment: .leading, spacing: 2) {
                    Text(verbatim: item.name.string ?? "?").font(.app(14, weight: .medium)).lineLimit(1)
                    Text(verbatim: PickText.detail(item)).font(.app(12)).foregroundStyle(.dim).lineLimit(1)
                }
                Spacer()
                Text(verbatim: sizeText(item.bytes.double ?? 0, selected: model.parts.bytes(path, whole: item.bytes.double ?? 0), partial: partial))
                    .font(.app(14)).monospacedDigit()
                if model.phase == .review && Parts.canOpen(path) {
                    OpenButton(open: open) { model.toggleOpen(path) }
                }
            }
            .padding(.horizontal, 12).padding(.vertical, 6)
            .checkRow(on, partial: partial) { if model.phase == .review { model.toggle(i) } }
            .peekable(path)
            .contextMenu { FileMenu(path: path) }
            if open && model.phase == .review {
                PartRows(parent: path, parts: model.parts, parentOn: on, toggle: { model.toggle(child: $0, of: i) }, open: model.toggleOpen)
                    .padding(.leading, 40).padding(.bottom, 6)
            }
        }
        .background(RoundedRectangle(cornerRadius: 12).fill(.white.opacity(on ? 0.12 : 0)))
    }
}

/// What happened after `act`.
struct PicksOutcome: View {
    let outcome: JSON
    let error: String?
    /// what was removed went to the Trash (and can be put back)
    var trashed = false

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            if let error { Text(verbatim: error).foregroundStyle(Color.warn) }
            let removed = Int(outcome.removed.double ?? 0)
            Text(trashed ? String(localized: "\(removed) items moved to the Trash, \(bytes(outcome.bytes.double ?? 0)) freed")
                         : String(localized: "\(removed) items removed, \(bytes(outcome.bytes.double ?? 0)) freed"))
            if (outcome.skipped.double ?? 0) > 0 {
                Text("\(Int(outcome.skipped.double ?? 0)) items left alone: in use or changed since the scan").foregroundStyle(.dim)
            }
            if (outcome.failed.double ?? 0) > 0 {
                Text("\(Int(outcome.failed.double ?? 0)) items could not be removed").foregroundStyle(Color.warn)
                if let first = outcome.first_failure.string {
                    Text(verbatim: first).font(.app(12)).foregroundStyle(.dim).lineLimit(2)
                }
            }
            if trashed && removed > 0 { TrashNote().padding(.top, 6) }
        }
        .font(.app(15, weight: .medium))
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(18)
        .glassPanel()
    }
}

/// What a pick list does to the picked items.
enum PicksKind {
    /// to the Trash (installers)
    case trash
    /// deleted (rebuildable build folders)
    case delete
    /// Intel code removed (the original app goes to the Trash)
    case slim
}

/// Installers, Project Junk and Slim Apps: one engine request finds the items.
struct PicksPage: View {
    let tool: Tool
    let request: [String: Any]
    let kind: PicksKind
    let model: PicksModel
    @State private var confirming = false

    var body: some View {
        VStack(spacing: 0) {
            if model.phase == .idle {
                Intro(module: .tool(tool.id), subtitle: tool.subtitle, points: tool.points)
                if let error = model.error { ErrorBanner(message: error) { model.find(request) } }
            } else {
                VStack(alignment: .leading, spacing: 14) {
                    PageHeader(art: model.phase == .done ? "done" : tool.id, symbol: model.phase == .done ? "checkmark.seal" : tool.icon,
                               title: tool.title, status: status)
                    switch model.phase {
                    case .finding: LoadingState(text: String(localized: "Looking…"))
                    case .done: PicksOutcome(outcome: model.outcome, error: model.error, trashed: kind != .delete); Spacer()
                    case .acting: RemovalProgress(progress: model.progress); PicksList(model: model)
                    default: PicksList(model: model)
                    }
                }
            }
            bar
        }
        .padding(.horizontal, Layout.side)
        .confirmationDialog(confirmText, isPresented: $confirming) {
            Button(actionTitle, role: .destructive) { model.act() }
        } message: {
            if allCopies > 0 {
                Text("Every copy of \(allCopies) files is ticked: no copy would be left outside the Trash.")
            } else if kind == .delete {
                Text("Deleted folders skip the Trash.")
            }
        }
    }

    /// Duplicates: sets (groups) whose every copy is ticked.
    private var allCopies: Int {
        guard tool.id == "duplicates" else { return 0 }
        return model.groups.filter { $0.indexes.allSatisfy(model.selected.contains) }.count
    }

    private var actionTitle: String {
        switch kind {
        case .trash: String(localized: "Move to Trash")
        case .delete: String(localized: "Delete")
        case .slim: String(localized: "Slim")
        }
    }

    private var confirmText: String {
        switch kind {
        case .trash: String(localized: "Move \(model.selected.count) items (\(bytes(model.selectedBytes))) to the Trash?")
        case .delete: String(localized: "Delete \(model.selected.count) build folders (\(bytes(model.selectedBytes)))? They are rebuilt the next time you build.")
        case .slim: String(localized: "Remove the Intel code from \(model.selected.count) apps (\(bytes(model.selectedBytes)))? Each app is checked first; the originals go to the Trash.")
        }
    }

    private var status: String {
        switch model.phase {
        case .idle: ""
        case .finding: String(localized: "Looking…")
        case .review: model.items.isEmpty ? String(localized: "Nothing found.")
            : String(localized: "\(model.items.count) found, \(bytes(model.list.bytes.double ?? 0)). Nothing has been removed yet.")
        case .acting: String(localized: "Working…")
        case .done: String(localized: "Done: \(bytes(model.outcome.bytes.double ?? 0)) freed")
        }
    }

    private var bar: some View {
        ActionBar {
            if model.phase == .review || model.phase == .done {
                Button(String(localized: "Start Over")) { model.reset() }.buttonStyle(.pill)
            }
        } center: {
            switch model.phase {
            case .idle, .done: BigButton(title: String(localized: "Scan"), returnKey: true) { model.find(request) }
            case .finding, .acting: BigButton(title: "", busy: true) {}
            case .review:
                BigButton(title: actionTitle) { confirming = true }
                    .disabled(model.selected.isEmpty)
                    .opacity(model.selected.isEmpty ? 0.5 : 1)
            }
        } trailing: {
            if model.phase == .finding {
                Button(String(localized: "Stop")) { model.stop() }.buttonStyle(.pill).keyboardShortcut(.cancelAction)
            } else if model.phase == .review {
                Text(verbatim: bytes(model.selectedBytes)).font(.app(17, weight: .semibold)).monospacedDigit()
            }
        }
    }
}

/// Uninstaller on the engine: the app list at once (sizes fill in), pick apps, review
/// each app's leftovers, move them to the Trash.
@MainActor @Observable
final class AppsModel {
    private(set) var apps: [JSON]?
    private(set) var sizes: [Double] = []
    /// `sizes` by app path, so sorting and lookups stay O(1) per app
    private(set) var sizeByPath: [String: Double] = [:]
    var chosen: Set<String> = []
    var query = ""
    let picks = PicksModel()
    @ObservationIgnored private var job: Task<Void, Never>?

    /// Asks the app to quit (it may still ask to save), then lists the apps again once it
    /// has gone, or after 10 seconds.
    func quit(_ bundleID: String) {
        let running = NSRunningApplication.runningApplications(withBundleIdentifier: bundleID)
        running.forEach { $0.terminate() }
        Task {
            for _ in 0..<40 where running.contains(where: { !$0.isTerminated }) {
                try? await Task.sleep(for: .milliseconds(250))
            }
            load()
        }
    }

    func load() {
        job?.cancel()
        job = Task {
            for await event in Engine.run(["op": "apps"]) {
                switch event.event.string {
                case "apps":
                    apps = event.apps.array
                    sizes = Array(repeating: 0, count: apps?.count ?? 0)
                    sizeByPath = [:]
                case "sizes":
                    sizes = event.sizes.array.compactMap(\.double)
                    sizeByPath = Dictionary(zip((apps ?? []).map { $0.path.string ?? "" }, sizes), uniquingKeysWith: { a, _ in a })
                default: break
                }
            }
        }
    }

    func size(of app: JSON) -> Double? {
        sizeByPath[app.path.string ?? ""].flatMap { $0 > 0 ? $0 : nil }
    }

    var shown: [JSON] {
        let all = apps ?? []
        let q = query.trimmingCharacters(in: .whitespaces)
        let list = q.isEmpty ? all : all.filter { ($0.name.string ?? "").localizedCaseInsensitiveContains(q) }
        return list.sorted { (size(of: $0) ?? 0) > (size(of: $1) ?? 0) }
    }

    func review() {
        picks.find(["op": "uninstall_plan", "paths": Array(chosen)])
    }

    /// Apps dropped on the page: the engine refuses any it does not list as installed.
    func uninstall(_ paths: [String]) {
        picks.reset()
        chosen = Set(paths)
        review()
    }

    func startOver() {
        picks.reset()
        chosen = []
        load()
    }
}

struct UninstallerPage: View {
    @Bindable var model: AppsModel
    @State private var confirming = false

    private var picks: PicksModel { model.picks }

    var body: some View {
        VStack(spacing: 14) {
            PageHeader(art: picks.phase == .done ? "done" : "uninstaller", symbol: picks.phase == .done ? "checkmark.seal" : Module.uninstaller.icon,
                       title: String(localized: "Uninstaller"), status: subtitle) {
                if picks.phase == .idle { search }
            }

            switch picks.phase {
            case .idle: appList
            case .finding: LoadingState(text: String(localized: "Looking for what these apps left behind…"))
            case .acting: RemovalProgress(progress: picks.progress); Spacer()
            case .review: PicksList(model: picks)
            case .done: PicksOutcome(outcome: picks.outcome, error: picks.error, trashed: true); Spacer()
            }
            bar
        }
        .padding(.horizontal, Layout.side)
        .task { if model.apps == nil { model.load() } }
        // an app dragged in from Finder goes straight to the review of what it leaves behind
        .contentShape(Rectangle())
        .dropDestination(for: URL.self) { urls, _ in
            let apps = urls.filter { $0.pathExtension == "app" }.map(\.path)
            guard !apps.isEmpty, picks.phase != .acting else { return false }
            model.uninstall(apps)
            return true
        }
        .confirmationDialog(
            String(localized: "Move \(picks.selected.count) items (\(bytes(picks.selectedBytes))) to the Trash?"),
            isPresented: $confirming
        ) {
            Button(String(localized: "Uninstall"), role: .destructive) { picks.act() }
        }
    }

    private var subtitle: String {
        switch picks.phase {
        case .idle: String(localized: "Remove apps together with their caches, preferences and support files.")
        case .finding: String(localized: "Looking for what these apps left behind…")
        case .review: String(localized: "Review what goes to the Trash. Nothing has been removed yet.")
        case .acting: String(localized: "Working…")
        case .done: String(localized: "Done: \(bytes(picks.outcome.bytes.double ?? 0)) freed")
        }
    }

    private var search: some View {
        // grows with the text size, so the prompt is not cut at Extra Large
        SearchField(prompt: String(localized: "Search apps"), text: $model.query).frame(width: 240 * max(1, TextScale.shared.value))
    }

    @ViewBuilder private var appList: some View {
        if model.apps == nil {
            LoadingState(text: String(localized: "Listing your apps…"))
        } else {
            let shown = model.shown
            ScrollView {
                LazyVStack(spacing: 4) {
                    ForEach(shown, id: \.path.string) { app in appRow(app) }
                    if shown.isEmpty {
                        EmptyState(symbol: "magnifyingglass", title: String(localized: "No matches"),
                                   message: String(localized: "No app matches “\(model.query)”."))
                    }
                }
                .padding(8)
            }
            .brightScroller()
            .glassPanel()
            .quickLookList()
        }
    }

    /// The bundle id (or that the app is open); for two apps of one name, also where each
    /// is and its version.
    static func subtitle(_ app: JSON) -> String {
        let open = app.running.bool ? String(localized: "Open now: quit it before uninstalling") : nil
        guard app.twin.bool else { return open ?? app.bundle_id.string ?? "" }
        let place = ((app.path.string ?? "") as NSString).abbreviatingWithTildeInPath
        return [open, place, app.version.string].compactMap { $0 }.filter { !$0.isEmpty }.joined(separator: " · ")
    }

    /// Two apps of one name are told apart by their bundles' file names ("Codex" and
    /// "Codex Beta" both showing as "Codex").
    static func title(_ app: JSON) -> String {
        guard app.twin.bool, let path = app.path.string else { return app.name.string ?? "?" }
        return ((path as NSString).lastPathComponent as NSString).deletingPathExtension
    }

    private func appRow(_ app: JSON) -> some View {
        let path = app.path.string ?? ""
        let on = model.chosen.contains(path)
        return HStack(spacing: 12) {
            Image(systemName: on ? "checkmark.circle.fill" : "circle").font(.app(17))
            Image(nsImage: fileIcon(path)).resizable().frame(width: 34, height: 34)
            VStack(alignment: .leading, spacing: 2) {
                Text(verbatim: Self.title(app)).font(.app(14, weight: .medium))
                Text(verbatim: Self.subtitle(app))
                    .font(.app(12)).foregroundStyle(app.running.bool ? Color.warn : .dim)
            }
            Spacer()
            if app.running.bool, let id = app.bundle_id.string {
                Button(String(localized: "Quit")) { model.quit(id) }
                    .buttonStyle(.pill)
                    .help(String(localized: "Quit \(Self.title(app)) so it can be uninstalled"))
            }
            Text(verbatim: model.size(of: app).map { bytes($0) } ?? "…").monospacedDigit()
        }
        .padding(.horizontal, 12).padding(.vertical, 8)
        .background(RoundedRectangle(cornerRadius: 12).fill(.white.opacity(on ? 0.12 : 0)))
        .checkRow(on) { if on { model.chosen.remove(path) } else { model.chosen.insert(path) } }
        .peekable(path)
        .contextMenu { FileMenu(path: path) }
    }

    private var bar: some View {
        ActionBar(shown: !(picks.phase == .idle && model.chosen.isEmpty)) {
            if picks.phase == .review || picks.phase == .done {
                Button(String(localized: "Start Over")) { model.startOver() }.buttonStyle(.pill)
            } else if picks.phase == .idle, !model.chosen.isEmpty {
                let total = model.chosen.compactMap { model.sizeByPath[$0] }.reduce(0, +)
                Text(verbatim: bytes(total)).font(.app(17, weight: .semibold)).monospacedDigit()
            }
        } center: {
            switch picks.phase {
            case .idle:
                if !model.chosen.isEmpty { BigButton(title: String(localized: "Review"), returnKey: true) { model.review() } }
            case .finding, .acting: BigButton(title: "", busy: true) {}
            case .review:
                BigButton(title: String(localized: "Uninstall")) { confirming = true }
                    .disabled(picks.selected.isEmpty)
                    .opacity(picks.selected.isEmpty ? 0.5 : 1)
            case .done: EmptyView()
            }
        } trailing: {
            if picks.phase == .finding {
                Button(String(localized: "Stop")) { picks.stop() }.buttonStyle(.pill).keyboardShortcut(.cancelAction)
            } else if picks.phase == .review {
                Text(verbatim: bytes(picks.selectedBytes)).font(.app(17, weight: .semibold)).monospacedDigit()
            }
        }
    }
}
