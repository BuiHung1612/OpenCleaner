import SwiftUI

/// A pick list from the engine (installers, project build folders, an app and its
/// leftovers): find → pick → act. Nothing is touched until `act`, and the engine checks
/// every item again right before.
@MainActor
final class PicksModel: ObservableObject {
    enum Phase { case idle, finding, review, acting, done }

    @Published private(set) var phase = Phase.idle
    @Published private(set) var list = JSON(nil)
    @Published var selected: Set<Int> = []
    @Published var parts = Parts()
    @Published private(set) var outcome = JSON(nil)
    @Published private(set) var progress = JSON(nil)
    @Published private(set) var error: String?
    private var job: Task<Void, Never>?

    var items: [JSON] { list.items.array }
    var selectedBytes: Double {
        selected.filter(items.indices.contains).map { parts.bytes(path($0), whole: items[$0].bytes.double ?? 0) }
            .reduce(0, +)
    }

    func path(_ i: Int) -> String { items.indices.contains(i) ? items[i].path.string ?? "" : "" }

    func toggle(_ i: Int) {
        if selected.contains(i) { selected.remove(i) } else { selected.insert(i) }
        parts.whole(path(i))
    }

    func toggle(child: String, of i: Int) {
        if !selected.contains(i) {
            selected.insert(i)
            parts.only(child, in: path(i))
        } else if !parts.toggle(child, in: path(i)) {
            selected.remove(i)
        }
    }

    func toggleOpen(_ i: Int) {
        let p = path(i)
        parts.toggleOpen(p) { [weak self] kids in self?.parts.children[p] = kids }
    }
    /// Items in list order, grouped by what they belong to (an app, a project…).
    var groups: [(name: String, indexes: [Int])] {
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
            guard !Task.isCancelled, found.raw != nil else {
                if phase == .finding { phase = .idle }
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
            let chosen = parts.request { byPath[$0] != nil }
            let request: [String: Any] = [
                "op": "remove", "plan": Int(plan), "items": selected.sorted(),
                "parts": Dictionary(uniqueKeysWithValues: chosen.compactMap { key, kids in byPath[key].map { ("\($0)", kids) } }),
            ]
            for await event in Engine.run(request) {
                switch event.event.string {
                case "progress": progress = event
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

/// The checkbox list of a pick list, grouped.
struct PicksList: View {
    @ObservedObject var model: PicksModel

    var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 14) {
                ForEach(model.groups, id: \.name) { group in
                    VStack(alignment: .leading, spacing: 2) {
                        if !group.name.isEmpty {
                            Text(verbatim: group.name).font(.system(size: 13, weight: .semibold)).foregroundStyle(.dim)
                                .lineLimit(1).truncationMode(.head).padding(.leading, 12)
                        }
                        ForEach(group.indexes, id: \.self) { i in row(i) }
                    }
                }
                if model.items.isEmpty {
                    Text("Nothing found.").foregroundStyle(.dim).padding(20)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(12)
        }
        .brightScroller()
        .glassPanel()
    }

    private func row(_ i: Int) -> some View {
        let item = model.items[i]
        let path = item.path.string ?? ""
        let on = model.selected.contains(i)
        let partial = on && model.parts.isPartial(path)
        let open = model.parts.open.contains(path)
        return VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 12) {
                Image(systemName: partial ? "minus.circle.fill" : on ? "checkmark.circle.fill" : "circle").font(.system(size: 17))
                Image(nsImage: fileIcon(path)).resizable().frame(width: 28, height: 28)
                VStack(alignment: .leading, spacing: 2) {
                    Text(verbatim: item.name.string ?? "?").font(.system(size: 14, weight: .medium)).lineLimit(1)
                    Text(verbatim: tr(item.detail.string ?? "")).font(.system(size: 12)).foregroundStyle(.dim).lineLimit(1)
                }
                Spacer()
                Text(verbatim: bytes(model.parts.bytes(path, whole: item.bytes.double ?? 0))).font(.system(size: 14)).monospacedDigit()
                if model.phase == .review && Parts.canOpen(path) {
                    OpenButton(open: open) { model.toggleOpen(i) }
                }
            }
            .padding(.horizontal, 12).padding(.vertical, 6)
            .contentShape(Rectangle())
            .onTapGesture { if model.phase == .review { model.toggle(i) } }
            .contextMenu { Button(String(localized: "Show in Finder")) { reveal(path) } }
            if open && model.phase == .review {
                PartRows(parent: path, parts: model.parts, parentOn: on) { model.toggle(child: $0, of: i) }
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

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            if let error { Text(verbatim: error).foregroundStyle(Color(hex: "#FFD54F")) }
            Text("\(Int(outcome.removed.double ?? 0)) items removed, \(bytes(outcome.bytes.double ?? 0)) freed")
            if (outcome.skipped.double ?? 0) > 0 {
                Text("\(Int(outcome.skipped.double ?? 0)) items left alone: in use or changed since the scan").foregroundStyle(.dim)
            }
            if (outcome.failed.double ?? 0) > 0 {
                Text("\(Int(outcome.failed.double ?? 0)) items could not be removed").foregroundStyle(Color(hex: "#FFD54F"))
                if let first = outcome.first_failure.string {
                    Text(verbatim: first).font(.system(size: 12)).foregroundStyle(.dim).lineLimit(2)
                }
            }
        }
        .font(.system(size: 15, weight: .medium))
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
    @ObservedObject var model: PicksModel
    @State private var confirming = false

    var body: some View {
        VStack(spacing: 0) {
            if model.phase == .idle {
                Intro(module: .tool(tool.id), subtitle: tool.subtitle, points: tool.points)
            } else {
                VStack(alignment: .leading, spacing: 14) {
                    HStack(spacing: 16) {
                        Art(id: model.phase == .done ? "done" : tool.id, symbol: model.phase == .done ? "checkmark.seal" : tool.icon, size: 72)
                        VStack(alignment: .leading, spacing: 4) {
                            Text(tool.title).font(.system(size: 24, weight: .bold))
                            Text(status).foregroundStyle(.dim)
                        }
                        Spacer()
                    }
                    .padding(.top, 36)
                    switch model.phase {
                    case .finding: Spacer()
                    case .done: PicksOutcome(outcome: model.outcome, error: model.error); Spacer()
                    case .acting: RemovalProgress(progress: model.progress); PicksList(model: model)
                    default: PicksList(model: model)
                    }
                }
            }
            bar.padding(.bottom, 28)
        }
        .padding(.horizontal, 32)
        .confirmationDialog(confirmText, isPresented: $confirming) {
            Button(actionTitle, role: .destructive) { model.act() }
        }
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
        case .review: String(localized: "\(model.items.count) found, \(bytes(model.list.bytes.double ?? 0)). Nothing has been removed yet.")
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
            case .idle, .done: BigButton(title: String(localized: "Scan")) { model.find(request) }
            case .finding, .acting: BigButton(title: "", busy: true) {}
            case .review:
                BigButton(title: kind == .trash ? String(localized: "Remove") : actionTitle) { confirming = true }
                    .disabled(model.selected.isEmpty)
                    .opacity(model.selected.isEmpty ? 0.5 : 1)
            }
        } trailing: {
            if model.phase == .finding {
                Button(String(localized: "Stop")) { model.stop() }.buttonStyle(.pill)
            } else if model.phase == .review {
                Text(verbatim: bytes(model.selectedBytes)).font(.system(size: 17, weight: .semibold)).monospacedDigit()
            }
        }
    }
}

/// Uninstaller on the engine: the app list at once (sizes fill in), pick apps, review
/// each app's leftovers, move them to the Trash.
@MainActor
final class AppsModel: ObservableObject {
    @Published private(set) var apps: [JSON]?
    @Published private(set) var sizes: [Double] = []
    @Published var chosen: Set<String> = []
    @Published var query = ""
    let picks = PicksModel()
    private var job: Task<Void, Never>?

    func load() {
        job?.cancel()
        job = Task {
            for await event in Engine.run(["op": "apps"]) {
                switch event.event.string {
                case "apps":
                    apps = event.apps.array
                    sizes = Array(repeating: 0, count: apps?.count ?? 0)
                case "sizes":
                    sizes = event.sizes.array.compactMap(\.double)
                default: break
                }
            }
        }
    }

    func size(of app: JSON) -> Double? {
        guard let apps, let i = apps.firstIndex(where: { $0.path.string == app.path.string }), sizes.indices.contains(i) else { return nil }
        return sizes[i] > 0 ? sizes[i] : nil
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

    func startOver() {
        picks.reset()
        chosen = []
        load()
    }
}

struct UninstallerPage: View {
    @ObservedObject var model: AppsModel
    @ObservedObject var picks: PicksModel
    @State private var confirming = false

    init(model: AppsModel) {
        self.model = model
        self.picks = model.picks
    }

    var body: some View {
        VStack(spacing: 14) {
            HStack(spacing: 20) {
                Art(id: picks.phase == .done ? "done" : "uninstaller", symbol: picks.phase == .done ? "checkmark.seal" : "xmark.app", size: 84)
                VStack(alignment: .leading, spacing: 4) {
                    Text(String(localized: "Uninstaller")).font(.system(size: 28, weight: .bold))
                    Text(subtitle).foregroundStyle(.dim)
                }
                Spacer()
                if picks.phase == .idle { search }
            }
            .padding(.top, 36)

            switch picks.phase {
            case .idle: appList
            case .finding: Spacer()
            case .acting: RemovalProgress(progress: picks.progress); Spacer()
            case .review: PicksList(model: picks)
            case .done: PicksOutcome(outcome: picks.outcome, error: picks.error); Spacer()
            }
            bar.padding(.bottom, 12)
        }
        .padding(.horizontal, 32)
        .task { if model.apps == nil { model.load() } }
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
        HStack {
            Image(systemName: "magnifyingglass").foregroundStyle(.dim)
            ZStack(alignment: .leading) {
                if model.query.isEmpty { Text(String(localized: "Search apps")).foregroundStyle(.dim).allowsHitTesting(false) }
                TextField("", text: $model.query).textFieldStyle(.plain)
            }
            .font(.system(size: 14))
        }
        .padding(.horizontal, 14).padding(.vertical, 8)
        .frame(width: 240)
        .glassEffect(.clear.tint(.white.opacity(0.07)), in: .capsule)
    }

    @ViewBuilder private var appList: some View {
        if model.apps == nil {
            ProgressView().controlSize(.large).brightScroller().frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
            ScrollView {
                LazyVStack(spacing: 4) {
                    ForEach(model.shown, id: \.path.string) { app in appRow(app) }
                }
                .padding(8)
            }
            .brightScroller()
            .glassPanel()
        }
    }

    private func appRow(_ app: JSON) -> some View {
        let path = app.path.string ?? ""
        let on = model.chosen.contains(path)
        return HStack(spacing: 12) {
            Image(systemName: on ? "checkmark.circle.fill" : "circle").font(.system(size: 17))
            Image(nsImage: fileIcon(path)).resizable().frame(width: 34, height: 34)
            VStack(alignment: .leading, spacing: 2) {
                Text(verbatim: app.name.string ?? "?").font(.system(size: 14, weight: .medium))
                Text(verbatim: app.running.bool ? String(localized: "Open now: quit it before uninstalling") : (app.bundle_id.string ?? ""))
                    .font(.system(size: 12)).foregroundStyle(app.running.bool ? Color(hex: "#FFD54F") : .dim)
            }
            Spacer()
            Text(verbatim: model.size(of: app).map { bytes($0) } ?? "…").monospacedDigit()
        }
        .padding(.horizontal, 12).padding(.vertical, 8)
        .background(RoundedRectangle(cornerRadius: 12).fill(.white.opacity(on ? 0.12 : 0)))
        .contentShape(Rectangle())
        .onTapGesture { if on { model.chosen.remove(path) } else { model.chosen.insert(path) } }
        .contextMenu { Button(String(localized: "Show in Finder")) { reveal(path) } }
    }

    private var bar: some View {
        ActionBar {
            if picks.phase == .review || picks.phase == .done {
                Button(String(localized: "Start Over")) { model.startOver() }.buttonStyle(.pill)
            } else if picks.phase == .idle, !model.chosen.isEmpty {
                let total = model.apps?.filter { model.chosen.contains($0.path.string ?? "") }.compactMap { model.size(of: $0) }.reduce(0, +) ?? 0
                Text(verbatim: bytes(total)).font(.system(size: 17, weight: .semibold)).monospacedDigit()
            }
        } center: {
            switch picks.phase {
            case .idle:
                if !model.chosen.isEmpty { BigButton(title: String(localized: "Review")) { model.review() } }
            case .finding, .acting: BigButton(title: "", busy: true) {}
            case .review:
                BigButton(title: String(localized: "Uninstall")) { confirming = true }
                    .disabled(picks.selected.isEmpty)
                    .opacity(picks.selected.isEmpty ? 0.5 : 1)
            case .done: EmptyView()
            }
        } trailing: {
            if picks.phase == .review {
                Text(verbatim: bytes(picks.selectedBytes)).font(.system(size: 17, weight: .semibold)).monospacedDigit()
            }
        }
    }
}
