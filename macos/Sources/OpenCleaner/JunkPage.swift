import SwiftUI

/// System Junk on the Rust engine: scan (nothing removed) → review by rule → clean.
/// Lives as long as the window, like Space Lens, so a scan survives switching pages.
@MainActor
final class JunkModel: ObservableObject {
    enum Phase { case idle, scanning, review, cleaning, done }

    @Published private(set) var phase = Phase.idle
    /// The latest plan summary from the engine (partial while scanning).
    @Published private(set) var plan = JSON(nil)
    @Published var selected: Set<String> = []
    @Published private(set) var outcome = JSON(nil)
    @Published private(set) var cleanProgress = JSON(nil)
    @Published private(set) var error: String?
    private var job: Task<Void, Never>?

    var groups: [JSON] { plan.groups.array }
    var rules: [JSON] { groups.flatMap(\.rules.array) }
    var selectedBytes: Double {
        rules.filter { selected.contains($0.id.string ?? "") && $0.status.string == "ready" }
            .compactMap(\.bytes.double).reduce(0, +)
    }

    func scan() {
        job?.cancel()
        phase = .scanning
        plan = JSON(nil)
        error = nil
        job = Task {
            var final = JSON(nil)
            for await event in Engine.run(["op": "scan"]) {
                switch event.event.string {
                case "progress": plan = event
                case "plan": final = event
                default: break
                }
            }
            guard !Task.isCancelled, final.raw != nil else {
                if phase == .scanning { phase = .idle }
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
            for await event in Engine.run(["op": "clean", "plan": Int(planID), "rules": Array(selected)]) {
                switch event.event.string {
                case "progress": cleanProgress = event
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
        error = nil
    }
}

struct JunkPage: View {
    let tool: Tool
    @ObservedObject var model: JunkModel
    @State private var confirming = false

    var body: some View {
        VStack(spacing: 0) {
            switch model.phase {
            case .idle:
                Intro(module: .tool(tool.id), subtitle: tool.subtitle, points: tool.points)
            case .scanning, .review:
                review
            case .cleaning, .done:
                result
            }
            bar.padding(.bottom, 28)
        }
        .padding(.horizontal, 32)
        .confirmationDialog(
            String(localized: "Remove \(bytes(model.selectedBytes)) of junk? This cannot be undone."),
            isPresented: $confirming
        ) {
            Button(String(localized: "Clean"), role: .destructive) { model.clean() }
        }
    }

    private var header: some View {
        HStack(spacing: 16) {
            Art(id: model.phase == .done ? "done" : tool.id, symbol: model.phase == .done ? "checkmark.seal" : tool.icon, size: 72)
            VStack(alignment: .leading, spacing: 4) {
                Text(tool.title).font(.system(size: 24, weight: .bold))
                Text(status).foregroundStyle(.dim)
            }
            Spacer()
        }
        .padding(.top, 36)
    }

    private var status: String {
        switch model.phase {
        case .idle: return ""
        case .scanning: return String(localized: "Scanning… \(bytes(model.plan.bytes.double ?? 0)) found so far")
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
            ScrollView {
                VStack(alignment: .leading, spacing: 18) {
                    ForEach(model.groups, id: \.id.string) { group in
                        VStack(alignment: .leading, spacing: 4) {
                            Text(tr(group.title.string ?? "")).font(.system(size: 13, weight: .semibold)).foregroundStyle(.dim)
                                .padding(.leading, 12)
                            ForEach(group.rules.array, id: \.id.string) { rule in ruleRow(rule) }
                        }
                    }
                }
                .padding(12)
            }
            .brightScroller()
            .glassPanel()
        }
    }

    private func ruleRow(_ rule: JSON) -> some View {
        let id = rule.id.string ?? ""
        let ready = rule.status.string == "ready" && model.phase == .review
        let on = model.selected.contains(id)
        return HStack(alignment: .top, spacing: 12) {
            Image(systemName: on && ready ? "checkmark.circle.fill" : "circle")
                .font(.system(size: 17))
                .opacity(ready ? 1 : 0.35)
            VStack(alignment: .leading, spacing: 2) {
                Text(tr(rule.title.string ?? id)).font(.system(size: 14, weight: .medium))
                if let app = rule.in_use_by.string, rule.status.string == "in_use" {
                    Text("Skipped while \(app) is open").font(.system(size: 12)).foregroundStyle(Color(hex: "#FFD54F"))
                } else if let note = rule.note.string, !note.isEmpty {
                    Text(tr(note)).font(.system(size: 12)).foregroundStyle(.dim).fixedSize(horizontal: false, vertical: true)
                }
            }
            Spacer()
            Text(verbatim: bytes(rule.bytes.double ?? 0)).font(.system(size: 14)).monospacedDigit()
        }
        .padding(.horizontal, 12).padding(.vertical, 8)
        .background(RoundedRectangle(cornerRadius: 12).fill(.white.opacity(on && ready ? 0.12 : 0)))
        .contentShape(Rectangle())
        .onTapGesture {
            guard ready else { return }
            if on { model.selected.remove(id) } else { model.selected.insert(id) }
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
                        Text("\(Int(o.failed.double ?? 0)) items could not be removed").foregroundStyle(Color(hex: "#FFD54F"))
                    }
                }
                .font(.system(size: 15, weight: .medium))
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(18)
                .glassPanel()
            } else {
                ProgressView(value: model.cleanProgress.removed.double ?? 0, total: max(model.cleanProgress.total.double ?? 1, 1))
                    .brightScroller()
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
            case .idle: BigButton(title: String(localized: "Scan")) { model.scan() }
            case .scanning, .cleaning: BigButton(title: "", busy: true) {}
            case .review:
                BigButton(title: String(localized: "Clean")) { confirming = true }
                    .disabled(model.selectedBytes == 0)
                    .opacity(model.selectedBytes == 0 ? 0.5 : 1)
            case .done: BigButton(title: String(localized: "Scan")) { model.scan() }
            }
        } trailing: {
            if model.phase == .scanning {
                Button(String(localized: "Stop")) { model.stop() }.buttonStyle(.pill)
            } else if model.phase == .review {
                Text(verbatim: bytes(model.selectedBytes)).font(.system(size: 17, weight: .semibold)).monospacedDigit()
            }
        }
    }
}
