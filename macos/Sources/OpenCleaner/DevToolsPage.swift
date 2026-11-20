import SwiftUI

/// Developer Tools: space held by Docker and the iOS Simulator, given back with the tools'
/// own commands. Lives as long as the window, so the Dashboard can suggest it from the
/// last listing without running anything.
@MainActor @Observable
final class DevToolsModel {
    private(set) var listing = JSON(nil)
    private(set) var loading = false
    private(set) var working = false
    private(set) var outcome = JSON(nil)
    var selected: Set<String> = []

    var items: [JSON] { listing.items.array }
    /// What the items ticked by default would give back.
    var reclaimable: Double { items.filter(\.default.bool).compactMap(\.bytes.double).reduce(0, +) }

    /// Read-only: `docker system df` and `xcrun simctl list`.
    func load() async {
        loading = true
        listing = await Engine.ask(["op": "devtools"])
        selected = Set(items.filter { $0.default.bool && ($0.bytes.double ?? 0) > 0 }.compactMap(\.id.string))
        loading = false
    }

    func clean() async {
        working = true
        outcome = await Engine.ask(["op": "devtools_clean", "items": Array(selected)])
        working = false
        await load()
    }
}

struct DevToolsPage: View {
    let tool: Tool
    let model: DevToolsModel
    @State private var confirming = false

    var body: some View {
        let l = model.listing
        VStack(alignment: .leading, spacing: 14) {
            PageHeader(art: tool.id, symbol: tool.icon, title: tool.title, status: status) {
                Button { Task { await model.load() } } label: {
                    Label(String(localized: "Refresh"), systemImage: "arrow.clockwise").labelStyle(.iconOnly)
                }
                .buttonStyle(.pill)
                .disabled(model.loading || model.working)
                .help(String(localized: "Refresh"))
            }
            if let failure = model.outcome.failures.strings.first { ErrorBanner(message: failure) }
            ScrollView {
                VStack(alignment: .leading, spacing: 4) {
                    if l.raw == nil {
                        ProgressView().frame(maxWidth: .infinity).padding(40)
                    } else if l.docker.string == "missing" && !l.simulators.bool && !l.android.bool {
                        EmptyState(symbol: "hammer", title: String(localized: "No developer tools found"),
                                   message: String(localized: "Docker, Xcode's simulators and the Android SDK are not set up on this Mac."))
                    } else {
                        if l.docker.string == "missing" {
                            section(String(localized: "Docker"))
                            Text("Docker is not installed").font(.app(13)).foregroundStyle(.dim).padding(.horizontal, 12)
                        } else {
                            let context = l.docker_context.string ?? ""
                            section(context.isEmpty ? String(localized: "Docker") : "Docker · \(context)")
                            if l.docker.string == "stopped" {
                                Text("Docker is not running").font(.app(13)).foregroundStyle(.dim).padding(.horizontal, 12)
                            }
                            if let note = l.docker_note.string, !note.isEmpty {
                                Text(tr(note)).font(.app(13)).foregroundStyle(.dim).padding(.horizontal, 12)
                            }
                            ForEach(group("docker"), id: \.id.string) { row($0) }
                        }
                        if l.simulators.bool {
                            section(String(localized: "iOS Simulators"))
                            // the devices only; runtimes are listed below, each with its own size
                            Text("Simulator devices use \(bytes(l.devices_bytes.double)) in ~/Library/Developer/CoreSimulator/Devices")
                                .font(.app(13)).foregroundStyle(.dim).padding(.horizontal, 12)
                            ForEach(group("simulators"), id: \.id.string) { row($0) }
                        }
                        if l.android.bool {
                            section(String(localized: "Android"))
                            if let note = l.android_note.string, !note.isEmpty {
                                Text(tr(note)).font(.app(13)).foregroundStyle(.dim).padding(.horizontal, 12)
                            }
                            ForEach(group("android"), id: \.id.string) { row($0) }
                        }
                    }
                    // file caches (editors, AI coding apps, package managers) are rules of
                    // System Junk, with its in-use checks and review: point there
                    if l.raw != nil {
                        section(String(localized: "Editors, AI apps and package caches"))
                        Text("Xcode, Android Studio and JetBrains, VS Code, Cursor, Antigravity, ChatGPT and Codex, Claude, Orca, npm, Gradle, CocoaPods, Cargo and more are cleaned in System Junk, under Developer tools.")
                            .font(.app(13)).foregroundStyle(.dim).fixedSize(horizontal: false, vertical: true)
                            .padding(.horizontal, 12)
                        Button(String(localized: "Open System Junk")) { Automation.shared.openModule = .tool("clean") }
                            .buttonStyle(.pill).padding(.horizontal, 12).padding(.top, 4)
                    }
                }
                .padding(12)
            }
            .brightScroller()
            .glassPanel()
            ActionBar {
                EmptyView()
            } center: {
                if model.working {
                    BigButton(title: "", busy: true) {}
                } else {
                    BigButton(title: String(localized: "Clean")) { confirming = true }
                        .disabled(model.selected.isEmpty || model.loading)
                        .opacity(model.selected.isEmpty ? 0.5 : 1)
                }
            } trailing: {
                EmptyView()
            }
        }
        .padding(.horizontal, Layout.side)
        .task { if model.listing.raw == nil { await model.load() } }
        .confirmationDialog(String(localized: "Run the cleanup of \(model.selected.count) items? What they remove cannot be put back."),
                            isPresented: $confirming) {
            Button(String(localized: "Clean"), role: .destructive) { Task { await model.clean() } }
        }
    }

    private var status: String {
        guard let freed = model.outcome.freed.double, !model.working else { return tool.subtitle }
        return String(localized: "Freed \(bytes(freed)).")
    }

    private func group(_ name: String) -> [JSON] { model.items.filter { $0.group.string == name } }

    private func section(_ title: String) -> some View {
        Text(verbatim: title).font(.app(15, weight: .semibold)).padding(.horizontal, 12).padding(.top, 8)
    }

    private func row(_ item: JSON) -> some View {
        let id = item.id.string ?? ""
        let on = model.selected.contains(id)
        let detail = item.detail.string ?? ""
        return HStack(alignment: .top, spacing: 12) {
            Image(systemName: on ? "checkmark.circle.fill" : "circle").font(.app(17))
            VStack(alignment: .leading, spacing: 2) {
                Text(verbatim: tr(item.title.string ?? id) + (detail.isEmpty ? "" : " · \(detail)"))
                    .font(.app(14, weight: .medium))
                Text(tr(item.note.string ?? "")).font(.app(12)).foregroundStyle(.dim)
            }
            Spacer()
            Text(verbatim: bytes(item.bytes.double)).monospacedDigit()
        }
        .padding(.horizontal, 12).padding(.vertical, 8)
        .background(RoundedRectangle(cornerRadius: 12).fill(.white.opacity(on ? 0.12 : 0)))
        .checkRow(on) {
            guard !model.working else { return }
            if on { model.selected.remove(id) } else { model.selected.insert(id) }
        }
    }
}
