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

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack(spacing: 16) {
                Art(id: done ? "done" : tool.id, symbol: done ? "checkmark.seal" : tool.icon, size: 72)
                VStack(alignment: .leading, spacing: 4) {
                    Text(tool.title).font(.system(size: 24, weight: .bold))
                    Text(tr(tool.subtitle)).foregroundStyle(.dim)
                }
                Spacer()
            }
            .padding(.top, 36)
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
                    BigButton(title: String(localized: "Run")) { Task { await run() } }
                        .disabled(selected.isEmpty)
                        .opacity(selected.isEmpty ? 0.5 : 1)
                }
            } trailing: {
                EmptyView()
            }
            .padding(.bottom, 28)
        }
        .padding(.horizontal, 32)
        .task {
            tasks = Engine.call(["op": "maintenance"]).tasks.array
            selected = Set(tasks.filter { !$0.admin.bool }.compactMap(\.id.string))
        }
    }

    private func row(_ task: JSON) -> some View {
        let id = task.id.string ?? ""
        let on = selected.contains(id)
        return HStack(alignment: .top, spacing: 12) {
            Image(systemName: on ? "checkmark.circle.fill" : "circle").font(.system(size: 17))
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 6) {
                    Text(tr(task.title.string ?? id)).font(.system(size: 14, weight: .medium))
                    if task.admin.bool {
                        Image(systemName: "lock.fill").font(.system(size: 11)).foregroundStyle(.dim)
                            .help(String(localized: "Asks for your password"))
                    }
                }
                Text(tr(task.note.string ?? "")).font(.system(size: 12)).foregroundStyle(.dim)
            }
            Spacer()
            if let result = results[id] {
                Text(verbatim: result).font(.system(size: 12)).foregroundStyle(result == "✓" ? Color(hex: "#7CF2C8") : Color(hex: "#FFD54F"))
            }
        }
        .padding(.horizontal, 12).padding(.vertical, 8)
        .background(RoundedRectangle(cornerRadius: 12).fill(.white.opacity(on ? 0.12 : 0)))
        .contentShape(Rectangle())
        .onTapGesture {
            guard !running else { return }
            if on { selected.remove(id) } else { selected.insert(id) }
        }
    }

    private func run() async {
        running = true
        results = [:]
        let ids = Array(selected)
        let reply = await Task.detached { Engine.call(["op": "optimize", "tasks": ids]) }.value
        for r in reply.results.array {
            if let id = r.id.string { results[id] = r.ok.bool ? "✓" : (r.message.string ?? String(localized: "Failed")) }
        }
        if let script = reply.admin_script.string {
            let admin = tasks.filter { $0.admin.bool && selected.contains($0.id.string ?? "") }.compactMap(\.id.string)
            let mark: String
            switch await Privileged.run(script) {
            case .done: mark = "✓"
            case .cancelled: mark = String(localized: "Not run")
            case .failed(let message): mark = message
            }
            for id in admin { results[id] = mark }
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
            if let message { Text(verbatim: message).foregroundStyle(Color(hex: "#FFD54F")).padding(.bottom, 8) }
            ActionBar {
                Text(statusText).foregroundStyle(.dim)
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
            .padding(.bottom, 28)
        }
        .padding(.horizontal, 32)
        .task { refresh() }
    }

    private var statusText: String {
        guard state.raw != nil else { return "" }
        if !state.supported.bool { return String(localized: "Needs macOS 14 or later.") }
        return state.enabled.bool ? String(localized: "On: sudo accepts your fingerprint.") : String(localized: "Off")
    }

    private func refresh() {
        state = Engine.call(["op": "maintenance"]).touchid
    }

    private func toggle() async {
        working = true
        message = nil
        let script = Engine.call(["op": "touchid_script", "enable": !state.enabled.bool]).script.string ?? ""
        switch await Privileged.run(script) {
        case .done, .cancelled: break
        case .failed(let m): message = m
        }
        refresh()
        working = false
    }
}
