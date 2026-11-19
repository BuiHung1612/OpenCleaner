import AppKit
import SwiftUI

/// Runs a shell script the engine built (fixed commands only) with administrator rights,
/// through the system's authorisation dialog (password or Touch ID).
enum Privileged {
    enum Outcome { case done, cancelled, failed(String) }

    /// The AppleScript that runs `script` as root (quotes and backslashes escaped).
    static func source(for script: String) -> String {
        let escaped = script.replacingOccurrences(of: "\\", with: "\\\\").replacingOccurrences(of: "\"", with: "\\\"")
        return "do shell script \"\(escaped)\" with administrator privileges"
    }

    static func run(_ script: String) async -> Outcome {
        let source = source(for: script)
        return await MainActor.run {
            var error: NSDictionary?
            NSAppleScript(source: source)?.executeAndReturnError(&error)
            guard let error else { return .done }
            if (error[NSAppleScript.errorNumber] as? Int) == -128 { return .cancelled }
            return .failed((error[NSAppleScript.errorMessage] as? String) ?? String(localized: "It did not work."))
        }
    }
}

/// Optimization: safe maintenance tasks, picked by the user.
struct OptimizePage: View {
    let tool: Tool
    @State private var tasks: [JSON] = []
    @State private var selected: Set<String> = []
    @State private var running = false
    @State private var results: [String: String] = [:]
    @State private var done = false
    /// "Free up memory": free memory before and after, so the effect is plain to see
    @State private var memoryNote: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            PageHeader(art: done ? "done" : tool.id, symbol: done ? "checkmark.seal" : tool.icon, title: tool.title, status: status)
            ScrollView {
                VStack(alignment: .leading, spacing: 4) {
                    ForEach(tasks, id: \.id.string) { task in row(task) }
                }
                .padding(12)
            }
            .brightScroller()
            .glassPanel()
            ActionBar {
                EmptyView()
            } center: {
                if running {
                    BigButton(title: "", busy: true) {}
                } else {
                    BigButton(title: String(localized: "Run Selected")) { Task { await run() } }
                        .disabled(selected.isEmpty)
                        .opacity(selected.isEmpty ? 0.5 : 1)
                }
            } trailing: {
                EmptyView()
            }
        }
        .padding(.horizontal, Layout.side)
        .task {
            tasks = await Engine.ask(["op": "maintenance"]).tasks.array
            selected = Set(tasks.filter { !$0.admin.bool }.compactMap(\.id.string))
        }
    }

    /// The subtitle until a run, then how it went.
    private var status: String {
        guard !results.isEmpty, !running else { return tool.subtitle }
        let ok = results.values.filter { $0 == Self.okMark }.count
        return ok == results.count
            ? String(localized: "All \(ok) tasks done.")
            : String(localized: "\(ok) of \(results.count) tasks done.")
    }

    private static let okMark = "✓"

    private func row(_ task: JSON) -> some View {
        let id = task.id.string ?? ""
        let on = selected.contains(id)
        return HStack(alignment: .top, spacing: 12) {
            Image(systemName: on ? "checkmark.circle.fill" : "circle").font(.app(17))
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 6) {
                    Text(tr(task.title.string ?? id)).font(.app(14, weight: .medium))
                    if task.admin.bool {
                        Image(systemName: "lock.fill").font(.app(11)).foregroundStyle(.dim)
                            .help(String(localized: "Asks for your password"))
                            .accessibilityLabel(String(localized: "Asks for your password"))
                    }
                }
                Text(tr(task.note.string ?? "")).font(.app(12)).foregroundStyle(.dim)
                if id == "memory", let memoryNote {
                    Text(verbatim: memoryNote).font(.app(12, weight: .medium)).monospacedDigit()
                }
            }
            Spacer()
            if let result = results[id] {
                if result == Self.okMark {
                    Image(systemName: "checkmark.circle.fill").foregroundStyle(Color.ok)
                        .accessibilityLabel(String(localized: "Done"))
                } else {
                    Text(verbatim: result).font(.app(12)).foregroundStyle(Color.warn)
                }
            }
        }
        .padding(.horizontal, 12).padding(.vertical, 8)
        .background(RoundedRectangle(cornerRadius: 12).fill(.white.opacity(on ? 0.12 : 0)))
        .checkRow(on) {
            guard !running else { return }
            if on { selected.remove(id) } else { selected.insert(id) }
        }
    }

    private static func freeMemory() async -> Double? {
        await Engine.ask(["op": "status"]).memory.free.double
    }

    private func run() async {
        running = true
        results = [:]
        memoryNote = nil
        let ids = Array(selected)
        let reply = await Task.detached { Engine.call(["op": "optimize", "tasks": ids]) }.value
        for r in reply.results.array {
            if let id = r.id.string { results[id] = r.ok.bool ? "✓" : (r.message.string ?? String(localized: "Failed")) }
        }
        if let script = reply.admin_script.string {
            let memory = ids.contains("memory")
            let before = memory ? await Self.freeMemory() : nil
            let admin = tasks.filter { $0.admin.bool && selected.contains($0.id.string ?? "") }.compactMap(\.id.string)
            let mark: String
            switch await Privileged.run(script) {
            case .done: mark = "✓"
            case .cancelled: mark = String(localized: "Not run")
            case .failed(let message): mark = message
            }
            for id in admin { results[id] = mark }
            if memory, mark == "✓" {
                let after = await Self.freeMemory()
                memoryNote = String(localized: "Free memory: \(bytes(before)) before, \(bytes(after)) after")
            }
        }
        running = false
        done = results.values.allSatisfy { $0 == "✓" }
    }
}

/// Touch ID for sudo: on or off, with the system's authorisation dialog.
struct TouchIDPage: View {
    let tool: Tool
    @State private var state = JSON(nil)
    @State private var working = false
    @State private var message: String?

    var body: some View {
        VStack(spacing: 0) {
            Intro(module: .tool(tool.id), subtitle: tool.subtitle, points: tool.points)
                .overlay(alignment: .bottom) {
                    if !statusText.isEmpty {
                        Label(statusText, systemImage: state.enabled.bool ? "checkmark.circle.fill" : "circle")
                            .font(.app(14, weight: .medium))
                            .padding(.horizontal, 16).padding(.vertical, 8)
                            .glassPanel(radius: 18)
                    }
                }
            if let message { ErrorBanner(message: message).padding(.top, 8) }
            ActionBar {
                EmptyView()
            } center: {
                if working {
                    BigButton(title: "", busy: true) {}
                } else if state.supported.bool {
                    BigButton(title: state.enabled.bool ? String(localized: "Turn Off") : String(localized: "Turn On")) {
                        Task { await toggle() }
                    }
                }
            } trailing: {
                EmptyView()
            }
        }
        .padding(.horizontal, Layout.side)
        .task { await refresh() }
    }

    private var statusText: String {
        guard state.raw != nil else { return "" }
        if !state.supported.bool { return String(localized: "Needs macOS 14 or later.") }
        return state.enabled.bool ? String(localized: "On: sudo accepts your fingerprint.") : String(localized: "Off")
    }

    private func refresh() async {
        state = await Engine.ask(["op": "maintenance"]).touchid
    }

    private func toggle() async {
        working = true
        message = nil
        let script = await Engine.ask(["op": "touchid_script", "enable": !state.enabled.bool]).script.string ?? ""
        switch await Privileged.run(script) {
        case .done, .cancelled: break
        case .failed(let m): message = m
        }
        await refresh()
        working = false
    }
}
