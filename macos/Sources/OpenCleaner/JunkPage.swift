import SwiftUI

/// System Junk on the Rust engine: scan (nothing removed) → review by rule → clean.
/// Lives as long as the window, like Space Lens, so a scan survives switching pages.
@MainActor @Observable
final class JunkModel {
    enum Phase { case idle, scanning, review, cleaning, done }

    private(set) var phase = Phase.idle
    /// The latest plan summary from the engine (partial while scanning, at most ~10 a second).
    private(set) var plan = JSON(nil)
    var selected: Set<String> = []
    /// items unticked inside a selected rule (paths)
    var excluded: Set<String> = []
    var expanded: Set<String> = []
    /// items opened in turn, with some of their children ticked
    var parts = Parts()
    private(set) var outcome = JSON(nil)
    private(set) var cleanProgress = JSON(nil)
    private(set) var error: String?
    @ObservationIgnored private var job: Task<Void, Never>?

    /// Progress events can come hundreds of times a second; the page needs ~10.
    static let progressInterval: TimeInterval = 0.1

    var groups: [JSON] { plan.groups.array }
    var rules: [JSON] { groups.flatMap(\.rules.array) }
    var selectedBytes: Double {
        rules.filter { selected.contains($0.id.string ?? "") && $0.status.string == "ready" }
            .map(selectedBytes(of:)).reduce(0, +)
    }

    /// What goes of a ticked rule: all of it, less the items unticked inside it.
    func selectedBytes(of rule: JSON) -> Double {
        let items = Self.items(rule)
        guard !items.isEmpty else { return rule.bytes.double ?? 0 }
        return items.filter { !excluded.contains($0.path) }
            .map { parts.bytes($0.path, whole: $0.bytes) }.reduce(0, +)
    }

    /// The items a rule found, biggest first (as the engine lists them).
    static func items(_ rule: JSON) -> [(path: String, bytes: Double)] {
        rule.paths.array.compactMap { pair in
            let p = pair.array
            guard p.count == 2, let path = p[0].string else { return nil }
            return (path, p[1].double ?? 0)
        }
    }

    func toggleRule(_ rule: JSON) {
        guard let id = rule.id.string else { return }
        let paths = Set(Self.items(rule).map(\.path))
        if selected.contains(id) && paths.isDisjoint(with: excluded) && !paths.contains(where: parts.isPartial) {
            selected.remove(id)
        } else {
            selected.insert(id)
        }
        excluded.subtract(paths)
        paths.forEach { parts.whole($0) }
    }

    func isOn(_ path: String, in rule: JSON) -> Bool {
        selected.contains(rule.id.string ?? "") && !excluded.contains(path)
    }

    func toggleChild(_ child: String, of path: String, in rule: JSON) {
        if !isOn(path, in: rule) {
            toggleItem(path, in: rule)
            _ = parts.toggle(child, root: path, rootOn: false)
        } else if !parts.toggle(child, root: path, rootOn: true) {
            toggleItem(path, in: rule)
        }
    }

    func toggleOpen(_ path: String) {
        parts.toggleOpen(path) { [weak self] kids in self?.parts.children[path] = kids }
    }

    func toggleItem(_ path: String, in rule: JSON) {
        guard let id = rule.id.string else { return }
        defer { parts.whole(path) }
        if !selected.contains(id) {
            // ticking one item of an unticked rule: that item only
            selected.insert(id)
            excluded.formUnion(Self.items(rule).map(\.path).filter { $0 != path })
        } else if excluded.contains(path) {
            excluded.remove(path)
        } else {
            excluded.insert(path)
            if Self.items(rule).allSatisfy({ excluded.contains($0.path) }) {
                selected.remove(id)
                excluded.subtract(Self.items(rule).map(\.path))
            }
        }
    }

    func scan() {
        job?.cancel()
        phase = .scanning
        plan = JSON(nil)
        error = nil
        excluded = []
        expanded = []
        parts = Parts()
        job = Task {
            var final = JSON(nil)
            var shown = Date.distantPast
            for await event in Engine.run(["op": "scan", "list": true]) {
                switch event.event.string {
                case "progress" where Date().timeIntervalSince(shown) >= Self.progressInterval:
                    plan = event
                    shown = Date()
                case "plan": final = event
                case "error": error = event.error.string
                default: break
                }
            }
            // stopped, or replaced by a newer scan: that one owns the page now
            guard !Task.isCancelled else { return }
            guard final.raw != nil else {
                if error == nil { error = String(localized: "The scan did not finish.") }
                phase = .idle
                return
            }
            plan = final
            // what each rule says, minus rows with nothing to do
            selected = Set(rules.filter { $0.default.bool && $0.status.string == "ready" }.compactMap(\.id.string))
            phase = .review
        }
    }

    func clean() {
        guard let planID = plan.plan.double, !selected.isEmpty else { return }
        job?.cancel()
        phase = .cleaning
        cleanProgress = JSON(nil)
        job = Task {
            var final = JSON(nil)
            let chosen = Set(rules.filter { selected.contains($0.id.string ?? "") }.flatMap { Self.items($0).map(\.path) })
                .subtracting(excluded)
            let request: [String: Any] = [
                "op": "clean", "plan": Int(planID), "rules": Array(selected), "skip": Array(excluded),
                "parts": parts.request(chosen),
            ]
            var shown = Date.distantPast
            for await event in Engine.run(request) {
                switch event.event.string {
                case "progress" where Date().timeIntervalSince(shown) >= Self.progressInterval:
                    cleanProgress = event
                    shown = Date()
                case "cleaned": final = event
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
        if phase == .scanning { phase = .idle }
    }

    func reset() {
        stop()
        phase = .idle
        plan = JSON(nil)
        outcome = JSON(nil)
        selected = []
        excluded = []
        expanded = []
        parts = Parts()
        error = nil
    }
}

struct JunkPage: View {
    let tool: Tool
    let model: JunkModel
    @State private var confirming = false
    @State private var filter = ListFilter()

    var body: some View {
        VStack(spacing: 0) {
            switch model.phase {
            case .idle:
                Intro(module: .tool(tool.id), subtitle: tool.subtitle, points: tool.points)
                if let error = model.error { ErrorBanner(message: error) { model.scan() } }
            case .scanning, .review:
                review
            case .cleaning, .done:
                result
            }
            bar
        }
        .padding(.horizontal, Layout.side)
        .confirmationDialog(
            String(localized: "Remove \(bytes(model.selectedBytes)) of junk? This cannot be undone."),
            isPresented: $confirming
        ) {
            Button(String(localized: "Clean"), role: .destructive) { model.clean() }
        } message: {
            Text("Cleaned items skip the Trash.")
        }
    }

    private var header: some View {
        PageHeader(art: model.phase == .done ? "done" : tool.id, symbol: model.phase == .done ? "checkmark.seal" : tool.icon,
                   title: tool.title, status: status)
    }

    private var status: String {
        switch model.phase {
        case .idle: return ""
        case .scanning:
            let found = model.plan.bytes.double ?? 0
            return found > 0 ? String(localized: "Scanning… \(bytes(found)) found so far") : String(localized: "Scanning…")
        case .review: return String(localized: "\(bytes(model.plan.bytes.double ?? 0)) found. Nothing has been removed yet.")
        case .cleaning:
            let p = model.cleanProgress
            return String(localized: "Cleaning… \(bytes(p.bytes.double ?? 0)) freed")
        case .done:
            if let error = model.error { return error }
            return String(localized: "Done: \(bytes(model.outcome.bytes.double ?? 0)) freed")
        }
    }

    private var review: some View {
        VStack(alignment: .leading, spacing: 14) {
            header
            if model.phase == .review { FilterBar(filter: $filter) }
            ScrollView {
                VStack(alignment: .leading, spacing: 18) {
                    ForEach(model.groups, id: \.id.string) { group in
                        let rules = shownRules(group)
                        if !rules.isEmpty {
                            VStack(alignment: .leading, spacing: 4) {
                                HStack(spacing: 6) {
                                    if group.id.string == "privacy" {
                                        Art(id: "privacy", symbol: "hand.raised", size: 26).accessibilityHidden(true)
                                    }
                                    Text(tr(group.title.string ?? "")).font(.app(13, weight: .semibold)).foregroundStyle(.dim)
                                }
                                .padding(.leading, 12)
                                .accessibilityAddTraits(.isHeader)
                                ForEach(rules, id: \.id.string) { rule in ruleRow(rule) }
                                if group.id.string == "privacy" {
                                    Text("Safari is not listed: macOS protects its history and cookies.")
                                        .font(.app(12)).foregroundStyle(.dim).padding(.leading, 12)
                                }
                            }
                        }
                    }
                    if model.groups.isEmpty {
                        HStack(spacing: 10) {
                            ProgressView().controlSize(.small)
                            Text("Looking through caches, logs and app data…").foregroundStyle(.dim)
                        }
                        .padding(8)
                    } else if model.phase == .review, !filter.query.isEmpty, model.groups.allSatisfy({ shownRules($0).isEmpty }) {
                        EmptyState(symbol: "magnifyingglass", title: String(localized: "No matches"),
                                   message: String(localized: "Nothing found matches “\(filter.query)”."))
                    }
                }
                // fill the panel even before the first rows arrive
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(12)
            }
            .brightScroller()
            .glassPanel()
            .quickLookList()
        }
    }

    /// A rule stays when its title or one of its items matches the search.
    private func shownRules(_ group: JSON) -> [JSON] {
        filter.sorted(group.rules.array.filter { !shownItems($0).isEmpty || filter.matches(tr($0.title.string ?? "")) },
                      name: { tr($0.title.string ?? "") }, size: { $0.bytes.double ?? 0 })
    }

    /// All of a rule's items when its title matches, else the items whose path does.
    private func shownItems(_ rule: JSON) -> [(path: String, bytes: Double)] {
        let all = JunkModel.items(rule)
        let items = filter.matches(tr(rule.title.string ?? "")) ? all : all.filter { filter.matches($0.path) }
        return filter.sorted(items, name: { URL(fileURLWithPath: $0.path).lastPathComponent }, size: \.bytes)
    }

    private func ruleRow(_ rule: JSON) -> some View {
        let id = rule.id.string ?? ""
        let ready = rule.status.string == "ready" && model.phase == .review
        let on = model.selected.contains(id)
        let items = JunkModel.items(rule)
        let partial = on && items.contains { model.excluded.contains($0.path) || model.parts.isPartial($0.path) }
        let open = model.expanded.contains(id)
        return VStack(alignment: .leading, spacing: 2) {
            HStack(alignment: .top, spacing: 12) {
                Image(systemName: partial ? "minus.circle.fill" : on && ready ? "checkmark.circle.fill" : "circle")
                    .font(.app(17))
                    .opacity(ready ? 1 : 0.35)
                VStack(alignment: .leading, spacing: 2) {
                    Text(tr(rule.title.string ?? id)).font(.app(14, weight: .medium))
                    if let app = rule.in_use_by.string, rule.status.string == "in_use" {
                        Text("Skipped while \(app) is open").font(.app(12)).foregroundStyle(Color.warn)
                    } else if let note = rule.note.string, !note.isEmpty {
                        Text(tr(note)).font(.app(12)).foregroundStyle(.dim).fixedSize(horizontal: false, vertical: true)
                    }
                }
                Spacer()
                if rule.status.string != "in_use" {
                    Text(verbatim: sizeText(rule.bytes.double ?? 0, selected: model.selectedBytes(of: rule), partial: partial))
                        .font(.app(14)).monospacedDigit()
                }
                if ready && !items.isEmpty {
                    Button {
                        if open { model.expanded.remove(id) } else { model.expanded.insert(id) }
                    } label: {
                        Image(systemName: "chevron.right")
                            .rotationEffect(.degrees(open ? 90 : 0))
                            .frame(width: 22, height: 22)
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .help(String(localized: "Show items"))
                    .accessibilityLabel(String(localized: "Show items"))
                }
            }
            .padding(.horizontal, 12).padding(.vertical, 8)
            .checkRow(on && ready, partial: partial) {
                guard ready else { return }
                model.toggleRule(rule)
            }
            if open && ready {
                LazyVStack(alignment: .leading, spacing: 0) {
                    ForEach(shownItems(rule), id: \.path) { item in itemRow(item, in: rule) }
                }
                .padding(.leading, 29).padding(.bottom, 6)
            }
        }
        .background(RoundedRectangle(cornerRadius: 12).fill(.white.opacity(on && ready ? 0.12 : 0)))
    }

    private func itemRow(_ item: (path: String, bytes: Double), in rule: JSON) -> some View {
        let on = model.isOn(item.path, in: rule)
        let partial = on && model.parts.isPartial(item.path)
        let open = model.parts.open.contains(item.path)
        let url = URL(fileURLWithPath: item.path)
        return VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 10) {
                Image(systemName: partial ? "minus.circle.fill" : on ? "checkmark.circle.fill" : "circle").font(.app(14))
                Text(verbatim: url.deletingLastPathComponent().lastPathComponent + "/" + url.lastPathComponent)
                    .font(.app(12)).lineLimit(1).truncationMode(.middle)
                    .help(item.path)
                Spacer()
                Text(verbatim: sizeText(item.bytes, selected: model.parts.bytes(item.path, whole: item.bytes), partial: partial))
                    .font(.app(12)).monospacedDigit().foregroundStyle(.dim)
                if Parts.canOpen(item.path) {
                    OpenButton(open: open) { model.toggleOpen(item.path) }
                } else {
                    Color.clear.frame(width: 22, height: 1)
                }
            }
            .padding(.horizontal, 12).padding(.vertical, 4)
            .checkRow(on, partial: partial) { model.toggleItem(item.path, in: rule) }
            .peekable(item.path)
            .contextMenu { FileMenu(path: item.path) }
            if open {
                PartRows(parent: item.path, parts: model.parts, parentOn: on,
                         toggle: { model.toggleChild($0, of: item.path, in: rule) }, open: model.toggleOpen)
                    .padding(.leading, 24)
            }
        }
    }

    private var result: some View {
        VStack(alignment: .leading, spacing: 16) {
            header
            if model.phase == .done {
                let o = model.outcome
                VStack(alignment: .leading, spacing: 6) {
                    Text("\(Int(o.removed.double ?? 0)) items removed, \(bytes(o.bytes.double ?? 0)) freed")
                    if (o.skipped.double ?? 0) > 0 {
                        Text("\(Int(o.skipped.double ?? 0)) items left alone: in use or changed since the scan").foregroundStyle(.dim)
                    }
                    if (o.failed.double ?? 0) > 0 {
                        Text("\(Int(o.failed.double ?? 0)) items could not be removed").foregroundStyle(Color.warn)
                    }
                }
                .font(.app(15, weight: .medium))
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(18)
                .glassPanel()
            } else {
                RemovalProgress(progress: model.cleanProgress)
            }
            Spacer()
        }
    }

    private var bar: some View {
        ActionBar {
            if model.phase == .review || model.phase == .done {
                Button(String(localized: "Start Over")) { model.reset() }.buttonStyle(.pill)
            }
        } center: {
            switch model.phase {
            case .idle: BigButton(title: String(localized: "Scan"), returnKey: true) { model.scan() }
            case .scanning, .cleaning: BigButton(title: "", busy: true) {}
            case .review:
                BigButton(title: String(localized: "Clean")) { confirming = true }
                    .disabled(model.selectedBytes == 0)
                    .opacity(model.selectedBytes == 0 ? 0.5 : 1)
            case .done: BigButton(title: String(localized: "Scan"), returnKey: true) { model.scan() }
            }
        } trailing: {
            if model.phase == .scanning {
                Button(String(localized: "Stop")) { model.stop() }.buttonStyle(.pill).keyboardShortcut(.cancelAction)
            } else if model.phase == .review {
                Text(verbatim: bytes(model.selectedBytes)).font(.app(17, weight: .semibold)).monospacedDigit()
            }
        }
    }
}

/// How far a removal is: a bar over the items, the one being removed now, bytes freed.
struct RemovalProgress: View {
    let progress: JSON

    var body: some View {
        let p = progress
        let done = ["removed", "skipped", "failed"].compactMap { p[$0].double }.reduce(0, +)
        let total = max(p.total.double ?? 1, 1)
        VStack(alignment: .leading, spacing: 8) {
            ProgressView(value: min(done, total), total: total).brightScroller()
            HStack {
                if let current = p.current.string {
                    Text("Removing \(URL(fileURLWithPath: current).lastPathComponent)")
                        .lineLimit(1).truncationMode(.middle).help(current)
                }
                Spacer()
                Text("\(Int(done)) of \(Int(total)) · \(bytes(p.bytes.double ?? 0)) freed").monospacedDigit()
            }
            .font(.app(12)).foregroundStyle(.dim)
        }
    }
}
