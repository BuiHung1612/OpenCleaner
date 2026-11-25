import AppKit
import SwiftUI

/// Process Doctor: which of your processes are stuck in a loop, heavy or left behind, and
/// which apps hold the most memory. Quitting asks the process to quit first, as closing it
/// would; "Force Quit" is offered only when it does not.
struct HogsPage: View {
    let tool: Tool
    @State private var report = JSON(nil)
    @State private var loaded = false
    @State private var message: String?
    @State private var forcing: JSON?
    @State private var working: Set<String> = []

    private var findings: [JSON] { report.findings.array }
    private var quitFirst: [JSON] { findings.filter { $0.verdict.string == "quit" } }
    private var review: [JSON] { findings.filter { $0.verdict.string != "quit" } }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            PageHeader(art: tool.id, symbol: tool.icon, title: tool.title, status: status) {
                Button("Refresh") { Task { await refresh() } }.buttonStyle(.pill)
            }
            if let message { ErrorBanner(message: message) }
            ScrollView {
                VStack(alignment: .leading, spacing: 18) {
                    if loaded && findings.isEmpty && report.groups.array.isEmpty {
                        Text("Nothing is stuck or heavy right now.")
                            .font(.app(14)).foregroundStyle(.dim).padding(12)
                    }
                    section("Worth quitting", quitFirst)
                    section("Worth a look", review)
                    groupsSection
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(12)
            }
            .brightScroller()
            .glassPanel()
            .padding(.bottom, Layout.bottom)
        }
        .padding(.horizontal, Layout.side)
        .task { await refresh() }
        .confirmationDialog(
            String(localized: "Force quit \(forcing?.app.string ?? "")?"),
            isPresented: Binding(get: { forcing != nil }, set: { if !$0 { forcing = nil } })
        ) {
            Button("Force Quit", role: .destructive) {
                if let f = forcing { Task { await quit(f, force: true) } }
            }
        } message: {
            Text("It stops at once. Anything it has not saved is lost.")
        }
    }

    private var status: String {
        let machine = report.machine
        guard loaded, machine.under_pressure.bool else { return tool.subtitle }
        return String(localized: "Your Mac is short of memory: \(bytes(machine.swap_used.double ?? 0)) of \(bytes(machine.swap_total.double ?? 0)) swap in use.")
    }

    @ViewBuilder private func section(_ title: LocalizedStringKey, _ items: [JSON]) -> some View {
        if !items.isEmpty {
            VStack(alignment: .leading, spacing: 4) {
                Text(title).font(.app(13, weight: .semibold)).foregroundStyle(.dim).padding(.leading, 12)
                    .accessibilityAddTraits(.isHeader)
                ForEach(Array(items.enumerated()), id: \.offset) { _, f in row(f) }
            }
        }
    }

    private func row(_ f: JSON) -> some View {
        HStack(alignment: .top, spacing: 12) {
            VStack(alignment: .leading, spacing: 2) {
                Text(verbatim: f.app.string ?? f.name.string ?? "").font(.app(14, weight: .medium))
                Text(verbatim: f.reasons.strings.map(Self.reason).joined(separator: " · "))
                    .font(.app(12)).foregroundStyle(.dim).fixedSize(horizontal: false, vertical: true)
                Text(verbatim: f.command.string ?? "")
                    .font(.app(11)).foregroundStyle(.dim).lineLimit(1).truncationMode(.middle)
            }
            Spacer()
            VStack(alignment: .trailing, spacing: 2) {
                Text(verbatim: bytes(f.memory_bytes.double ?? 0)).font(.app(13, weight: .medium))
                Text(verbatim: "\(Int(f.cpu.double ?? 0))% · \(duration(f.age_secs.double ?? 0))")
                    .font(.app(11)).foregroundStyle(.dim)
            }
            if f.can_quit.bool {
                Button("Quit") { Task { await quit(f, force: false) } }
                    .buttonStyle(.pill).disabled(working.contains(f.key))
                    .accessibilityLabel(String(localized: "Quit \(f.app.string ?? "")"))
            }
        }
        .padding(.horizontal, 12).padding(.vertical, 8)
    }

    @ViewBuilder private var groupsSection: some View {
        if !report.groups.array.isEmpty {
            VStack(alignment: .leading, spacing: 4) {
                Text("Apps using the most memory").font(.app(13, weight: .semibold)).foregroundStyle(.dim)
                    .padding(.leading, 12).accessibilityAddTraits(.isHeader)
                ForEach(Array(report.groups.array.enumerated()), id: \.offset) { _, g in
                    let app = Self.runningApp(g.app.string ?? "")
                    HStack(spacing: 12) {
                        VStack(alignment: .leading, spacing: 2) {
                            Text(verbatim: g.app.string ?? "").font(.app(14, weight: .medium))
                            Text("\(Int(g.count.double ?? 0)) processes").font(.app(12)).foregroundStyle(.dim)
                        }
                        Spacer()
                        Text(verbatim: bytes(g.memory_bytes.double ?? 0)).font(.app(13, weight: .medium))
                        if let app {
                            Button("Quit App") { quitApp(app) }.buttonStyle(.pill)
                                .accessibilityLabel(String(localized: "Quit \(g.app.string ?? "")"))
                        }
                    }
                    .padding(.horizontal, 12).padding(.vertical, 8)
                }
            }
        }
    }

    private static func reason(_ code: String) -> String {
        switch code {
        case "spinning": String(localized: "Stuck in a loop: busy for a long time")
        case "heavy_memory": String(localized: "Uses a lot of memory")
        case "left_behind": String(localized: "Left behind after the app that started it closed")
        case "idle_dev_tool": String(localized: "A developer tool running for over a day, doing nothing")
        case "zombie": String(localized: "Has ended but is not cleaned up; quitting the app that started it clears it")
        case "stopped": String(localized: "Suspended")
        default: code
        }
    }

    /// The running GUI app of that name, for "Quit App" (nil for command-line tools).
    private static func runningApp(_ name: String) -> NSRunningApplication? {
        NSWorkspace.shared.runningApplications.first {
            $0.activationPolicy == .regular && $0.localizedName == name
        }
    }

    private func quitApp(_ app: NSRunningApplication) {
        app.terminate()
        Task {
            try? await Task.sleep(for: .seconds(2))
            await refresh()
        }
    }

    private func refresh() async {
        report = await Engine.ask(["op": "hogs"])
        loaded = true
        message = report.ok.bool ? nil : report.error.string
    }

    private func quit(_ f: JSON, force: Bool) async {
        working.insert(f.key)
        defer { working.remove(f.key) }
        let request: [String: Any] = [
            "op": "quit_process", "pid": Int(f.pid.double ?? 0),
            "started": Int64(f.started.double ?? 0), "force": force,
        ]
        let reply = await Engine.ask(request)
        if !reply.ok.bool {
            message = reply.error.string
        } else if reply.outcome.string == "still_running" {
            forcing = f
            return
        } else {
            message = nil
        }
        await refresh()
    }
}

private extension JSON {
    /// A listed process: its pid with its start time, as the engine checks them.
    var key: String { "\(self["pid"].string ?? "")-\(self["started"].string ?? "")" }
}
