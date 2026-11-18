import AppKit
import SwiftUI

struct DashboardPage: View {
    /// owned by the window, so coming back to the dashboard shows the last reading at once
    @ObservedObject var stream: StatusStream
    @AppStorage("autoRefresh") private var autoRefresh = false
    @AppStorage("refreshInterval") private var refreshInterval = 5

    var body: some View {
        let s = stream.snapshot
        ScrollView {
            VStack(alignment: .leading, spacing: 20) {
                HStack(spacing: 28) {
                    HealthRing(score: s.health_score.double).id(stream.replay)
                    VStack(alignment: .leading, spacing: 10) {
                        HStack(spacing: 12) {
                            Text(String(localized: "Dashboard")).font(.system(size: 34, weight: .bold)).lineLimit(1).fixedSize()
                            Button { stream.refresh() } label: {
                                Image(systemName: "arrow.clockwise").symbolEffect(.rotate, isActive: stream.loading)
                            }
                            .buttonStyle(.pill)
                            .disabled(stream.loading)
                            .help(String(localized: "Refresh"))
                        }
                        Text(verbatim: s.health_score_msg.string.map(tr) ?? String(localized: "Measuring system status…"))
                            .font(.system(size: 15)).foregroundStyle(.dim)
                        let hw = s.hardware
                        Text(verbatim: [hw.model, hw.cpu_model, hw.total_ram, hw.os_version].compactMap(\.string).filter { !$0.isEmpty }.joined(separator: " · "))
                            .font(.system(size: 13)).foregroundStyle(.dim)
                        if let up = s.uptime.string { Text("Up for \(up)").font(.system(size: 13)).foregroundStyle(.dim) }
                        if let t = stream.updatedAt {
                            Text("Updated \(t.formatted(date: .omitted, time: .standard))").font(.system(size: 12)).foregroundStyle(.dim)
                        }
                    }
                    .layoutPriority(1)
                    Spacer()
                    Art(id: "dashboard", symbol: "square.grid.2x2", size: 170)
                }
                .padding(.top, 40)

                if let error = stream.error { Label(error, systemImage: "exclamationmark.triangle") }

                GlassEffectContainer(spacing: 16) {
                    LazyVGrid(columns: [GridItem(.adaptive(minimum: 140), spacing: 16)], spacing: 16) {
                        Tile(replay: stream.replay, title: String(localized: "CPU"), icon: "cpu", value: percent(s.cpu.usage.double), fraction: (s.cpu.usage.double ?? 0) / 100,
                             detail: s.cpu.load1.double.map { String(localized: "Load \(String(format: "%.1f", $0))") })
                        Tile(replay: stream.replay, title: String(localized: "Memory"), icon: "memorychip", value: percent(s.memory.used_percent.double), fraction: (s.memory.used_percent.double ?? 0) / 100,
                             detail: "\(bytes(s.memory.used.double)) / \(bytes(s.memory.total.double))")
                        let disk = s.disks.array.first { $0.mount.string == "/" }
                        Tile(replay: stream.replay, title: String(localized: "Disk"), icon: "internaldrive", value: percent(disk?.used_percent.double), fraction: (disk?.used_percent.double ?? 0) / 100,
                             detail: String(localized: "\(bytes(disk.flatMap { d in d.total.double.flatMap { t in d.used.double.map { t - $0 } } })) free"))
                        Tile(title: String(localized: "Trash"), icon: "trash", value: bytes(s.trash_size.double), fraction: nil, detail: nil)
                    }
                }

                VStack(alignment: .leading, spacing: 10) {
                    Text("Top processes").font(.system(size: 15, weight: .semibold))
                    ForEach(Array(s.top_processes.array.prefix(6).enumerated()), id: \.offset) { _, p in
                        HStack {
                            Text(verbatim: p.name.string ?? "?").lineLimit(1)
                            Spacer()
                            Text(verbatim: "\(percent(p.cpu.double)) CPU · \(bytes(p.memory_bytes.double))").monospacedDigit().foregroundStyle(.dim)
                        }
                        .font(.system(size: 13))
                    }
                }
                .padding(20)
                .glassPanel()
            }
            .padding(.horizontal, 32).padding(.bottom, 32)
        }
        // glass tiles live inside this scroll view; a dark scheme here would darken them
        .scrollIndicators(.never)
        .onAppear {
            stream.refresh(ifOlderThan: 10)
            if autoRefresh { stream.start(every: refreshInterval) }
        }
        .onDisappear { stream.stop() }
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in
            stream.refresh(ifOlderThan: 10)
        }
        // nobody is looking: stop the collector instead of redrawing glass every few seconds
        .onReceive(NotificationCenter.default.publisher(for: NSWindow.didChangeOcclusionStateNotification)) { note in
            guard let window = note.object as? NSWindow, window.identifier?.rawValue.contains("main") ?? true else { return }
            if window.occlusionState.contains(.visible), autoRefresh { stream.start(every: refreshInterval) } else { stream.stop() }
        }
        .onChange(of: autoRefresh) { restartStream() }
        .onChange(of: refreshInterval) { restartStream() }
    }

    private func restartStream() {
        stream.stop()
        if autoRefresh { stream.start(every: refreshInterval) }
    }
}

/// Sweeps up from empty when it appears (the parent re-creates it on each one-shot read);
/// auto-refresh updates move it without animation.
struct HealthRing: View {
    let score: Double?
    @State private var shown = 0.0

    var body: some View {
        ZStack {
            Circle().stroke(.white.opacity(0.12), lineWidth: 14)
            Circle()
                .trim(from: 0, to: shown / 100)
                .stroke(
                    AngularGradient(colors: [Color(hex: "#FF8A65"), Color(hex: "#FFD54F"), Color(hex: "#7CF2C8")], center: .center),
                    style: StrokeStyle(lineWidth: 14, lineCap: .round)
                )
                .rotationEffect(.degrees(-90))
            VStack(spacing: 0) {
                Text(verbatim: score.map { String(Int($0)) } ?? "—").font(.system(size: 48, weight: .bold)).monospacedDigit()
                Text("Health").font(.system(size: 13)).foregroundStyle(.dim)
            }
        }
        .frame(width: 140, height: 140)
        .padding(12)
        .glassEffect(.clear.tint(.white.opacity(0.07)), in: .circle)
        .onAppear { withAnimation(.easeOut(duration: 1.1)) { shown = score ?? 0 } }
        .onChange(of: score) { shown = score ?? 0 }
    }
}

struct Tile: View {
    var replay = 0
    let title: String
    let icon: String
    let value: String
    let fraction: Double?
    let detail: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Label(title, systemImage: icon).font(.system(size: 13, weight: .medium)).foregroundStyle(.dim)
            Text(verbatim: value).font(.system(size: 28, weight: .bold)).monospacedDigit()
            if let fraction { UsageBar(fraction: fraction).id(replay) }
            if let detail { Text(verbatim: detail).font(.system(size: 13)).foregroundStyle(.dim) }
        }
        .frame(maxWidth: .infinity, minHeight: 120, alignment: .topLeading)
        .padding(18)
        .glassPanel()
    }
}

struct UninstallerPage: View {
    @State private var apps: [JSON]?
    /// when `apps` was listed; shown while a newer list is loading
    @State private var listedAt: Date?
    @State private var refreshing = false
    @State private var error: String?
    @State private var query = ""
    @State private var selection: Set<String> = []
    @State private var session: Session?
    @State private var confirming = false

    var body: some View {
        VStack(spacing: 14) {
            if let session {
                SessionView(session: session, module: .uninstaller)
            } else {
                HStack(spacing: 20) {
                    Art(id: "uninstaller", symbol: "xmark.app", size: 84)
                    VStack(alignment: .leading, spacing: 4) {
                        Text(String(localized: "Uninstaller")).font(.system(size: 28, weight: .bold))
                        Text("Remove apps together with their caches, preferences and support files.").foregroundStyle(.dim)
                    }
                    Spacer()
                    HStack {
                        Image(systemName: "magnifyingglass").foregroundStyle(.dim)
                        ZStack(alignment: .leading) {
                            if query.isEmpty { Text(String(localized: "Search apps")).foregroundStyle(.dim).allowsHitTesting(false) }
                            TextField("", text: $query).textFieldStyle(.plain)
                        }
                        .font(.system(size: 14))
                    }
                    .padding(.horizontal, 14).padding(.vertical, 8)
                    .frame(width: 240)
                    .glassEffect(.clear.tint(.white.opacity(0.07)), in: .capsule)
                }
                .padding(.top, 36)

                if apps != nil, let listedAt, refreshing || error != nil {
                    RefreshBanner(date: listedAt, error: refreshing ? nil : error)
                }

                if let apps {
                    let shown = query.isEmpty ? apps : apps.filter { ($0.name.string ?? "").localizedCaseInsensitiveContains(query) }
                    ScrollView {
                        LazyVStack(spacing: 4) {
                            ForEach(Array(shown.enumerated()), id: \.offset) { _, a in
                                appRow(a)
                            }
                        }
                        .padding(8)
                    }
                    .brightScroller()
                    .glassPanel()
                } else {
                    VStack(spacing: 12) {
                        if let error { Text(error) } else { ProgressView().controlSize(.large).brightScroller() }
                    }
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                }
            }
            Group {
                if let session { Observe(session: session) { bar(for: $0) } } else { bar(for: nil) }
            }
        }
        .padding(.horizontal, 32)
        .task { await load() }
        .confirmationDialog(String(localized: "Uninstall \(selection.count) apps? They go to the Trash."), isPresented: $confirming) {
            Button(String(localized: "Uninstall"), role: .destructive) { start(preview: false) }
        }
    }

    private func appRow(_ a: JSON) -> some View {
        let name = a.uninstall_name.string ?? ""
        let selected = selection.contains(name)
        return HStack(spacing: 12) {
            Image(systemName: selected ? "checkmark.circle.fill" : "circle").font(.system(size: 17))
            Image(nsImage: fileIcon(a.path.string ?? "")).resizable().frame(width: 34, height: 34)
            VStack(alignment: .leading, spacing: 2) {
                Text(verbatim: a.name.string ?? "?").font(.system(size: 14, weight: .medium))
                Text(verbatim: a.bundle_id.string ?? "").font(.system(size: 12)).foregroundStyle(.dim)
            }
            Spacer()
            Text(verbatim: a.size.string ?? "").monospacedDigit()
        }
        .padding(.horizontal, 12).padding(.vertical, 8)
        .background(RoundedRectangle(cornerRadius: 12).fill(.white.opacity(selected ? 0.12 : 0)))
        .contentShape(Rectangle())
        .onTapGesture { if selected { selection.remove(name) } else { selection.insert(name) } }
        .contextMenu { Button(String(localized: "Show in Finder")) { reveal(a.path.string) } }
    }

    private func bar(for current: Session?) -> some View {
        let picking = current == nil && !selection.isEmpty
        return ActionBar {
            if let current, current.exitCode != nil {
                Button(String(localized: "Start Over")) {
                    self.session = nil
                    selection = []
                    Task { await load() }
                }
                .buttonStyle(.pill)
            } else if picking {
                let total = (apps ?? []).filter { selection.contains($0.uninstall_name.string ?? "") }.map { parseSize($0.size.string) }.reduce(0, +)
                Text(verbatim: bytes(total)).font(.system(size: 17, weight: .semibold)).monospacedDigit()
            }
        } center: {
            if picking { BigButton(title: String(localized: "Uninstall")) { confirming = true } }
        } trailing: {
            if picking { Button(String(localized: "Preview")) { start(preview: true) }.buttonStyle(.pill) }
        }
        .padding(.bottom, 12)
    }

    private func start(preview: Bool) {
        let names = selection.sorted()
        session = Session(
            title: preview ? String(localized: "Uninstall preview") : String(localized: "Uninstalling…"),
            args: CLI.command(preview ? "uninstallPreview" : "uninstall", ["names": names])
        )
    }

    /// Lists apps (~2.5 s). The previous list, from this visit or an earlier launch, stays on
    /// screen meanwhile with a banner saying how old it is.
    private func load() async {
        if apps == nil, let old = CLI.cached("apps") {
            apps = Self.sorted(old.json)
            listedAt = old.date
        }
        refreshing = true
        defer { refreshing = false }
        do {
            apps = Self.sorted(try await CLI.json("apps", persist: true))
            listedAt = Date()
            error = nil
        } catch {
            self.error = error.localizedDescription
        }
    }

    private static func sorted(_ list: JSON) -> [JSON] {
        list.array.sorted { parseSize($0.size.string) > parseSize($1.size.string) }
    }
}

struct HistoryPage: View {
    @State private var data = JSON(nil)
    @State private var error: String?

    /// mo logs commands like "optimize-dry-run"; show the app's own name for that tool.
    static func title(_ command: String) -> String {
        let preview = command.hasSuffix("-dry-run")
        let id = preview ? String(command.dropLast("-dry-run".count)) : command
        let name: String = switch id {
        case "uninstall": String(localized: "Uninstaller")
        case "analyze": String(localized: "Space Lens")
        default: CLI.tools.first { $0.id == id }?.title ?? id.capitalized
        }
        return preview ? String(localized: "\(name) · preview") : name
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack(spacing: 20) {
                Art(id: "history", symbol: "clock.arrow.circlepath", size: 84)
                Text(String(localized: "History")).font(.system(size: 28, weight: .bold))
            }
            .padding(.top, 36)
            ScrollView {
                VStack(spacing: 4) {
                    ForEach(Array(data.sessions.array.enumerated()), id: \.offset) { _, s in
                        HStack {
                            VStack(alignment: .leading, spacing: 2) {
                                Text(verbatim: Self.title(s.command.string ?? "?")).font(.system(size: 14, weight: .semibold))
                                Text(verbatim: s.started_at.string ?? "").font(.system(size: 12)).foregroundStyle(.dim)
                            }
                            Spacer()
                            Text("\(s.items.string ?? "0") items · \(s.size.string ?? "—")").monospacedDigit()
                        }
                        .padding(.horizontal, 14).padding(.vertical, 10)
                    }
                    if data.raw == nil {
                        if let error { Text(error) } else { ProgressView().brightScroller().padding(40) }
                    }
                }
                .padding(8)
            }
            .brightScroller()
            .glassPanel()
            if let log = data.logs.deletions.string {
                Text("Every deleted file is listed in \((log as NSString).abbreviatingWithTildeInPath)")
                    .font(.system(size: 12)).foregroundStyle(.dim)
            }
        }
        .padding(.horizontal, 32).padding(.bottom, 28)
        .task {
            do { data = try await CLI.json("history") } catch { self.error = error.localizedDescription }
        }
    }
}

struct SettingsView: View {
    @AppStorage(CLI.pathKey) private var customPath = ""
    @AppStorage("autoRefresh") private var autoRefresh = false
    @AppStorage("refreshInterval") private var refreshInterval = 5
    @AppStorage("language") private var language = ""
    @AppStorage(Updater.autoKey) private var checkUpdates = true
    @State private var languageChanged = false

    var body: some View {
        Form {
            Section(String(localized: "General")) {
                Picker(String(localized: "Language"), selection: $language) {
                    Text("System").tag("")
                    Text(verbatim: "English").tag("en")
                    Text(verbatim: "Tiếng Việt").tag("vi")
                }
                .onChange(of: language) {
                    UserDefaults.standard.set(language.isEmpty ? nil : [language], forKey: "AppleLanguages")
                    languageChanged = true
                }
                if languageChanged { Button(String(localized: "Relaunch"), action: relaunch) }
            }
            Section(String(localized: "Updates")) {
                LabeledContent(String(localized: "Version")) { Text(verbatim: Updater.current) }
                Toggle(String(localized: "Check for updates automatically"), isOn: $checkUpdates)
                Button(String(localized: "Check for Updates…")) { Task { await Updater.check(manual: true) } }
            }
            Section(String(localized: "Dashboard")) {
                Toggle(String(localized: "Refresh automatically"), isOn: $autoRefresh)
                Picker(String(localized: "Every"), selection: $refreshInterval) {
                    ForEach([3, 5, 10, 30], id: \.self) { Text("\($0) seconds").tag($0) }
                }
                .disabled(!autoRefresh)
                Text("Off: the dashboard reads once when the app opens or comes to the front. Use the refresh button for a new reading.")
                    .font(.system(size: 12)).foregroundStyle(.dim)
            }
            Section(String(localized: "Engine")) {
                LabeledContent(String(localized: "OpenCleaner engine")) {
                    Text(verbatim: Engine.version ?? "—").font(.system(size: 11, design: .monospaced))
                }
                LabeledContent(String(localized: "Engine path")) {
                    HStack {
                        Text(verbatim: CLI.binary ?? String(localized: "Not found")).font(.system(size: 11, design: .monospaced))
                            .lineLimit(1).truncationMode(.middle)
                        Button(String(localized: "Choose…")) {
                            let panel = NSOpenPanel()
                            panel.directoryURL = URL(fileURLWithPath: "/opt/homebrew/bin")
                            if panel.runModal() == .OK, let path = panel.url?.path { customPath = path }
                        }
                    }
                }
                if !customPath.isEmpty {
                    Button(String(localized: "Use the built-in engine")) { customPath = "" }
                }
                LabeledContent(String(localized: "Command list (cli.json)")) {
                    Button(String(localized: "Customize…"), action: customizeManifest)
                }
                Text("OpenCleaner runs the open-source mo engine (GPL-3.0), bundled with the app. If its commands change, editing cli.json is enough; reopen the app to apply.")
                    .font(.system(size: 12)).foregroundStyle(.dim)
            }
        }
        .formStyle(.grouped)
        .frame(width: 520)
    }

    private func customizeManifest() {
        let dest = CLI.manifestOverride
        let fm = FileManager.default
        if !fm.fileExists(atPath: dest.path), let src = CLI.manifestURL {
            try? fm.createDirectory(at: dest.deletingLastPathComponent(), withIntermediateDirectories: true)
            try? fm.copyItem(at: src, to: dest)
        }
        reveal(dest.path)
    }

    private func relaunch() {
        let config = NSWorkspace.OpenConfiguration()
        config.createsNewApplicationInstance = true
        NSWorkspace.shared.openApplication(at: Bundle.main.bundleURL, configuration: config) { _, _ in
            DispatchQueue.main.async { NSApp.terminate(nil) }
        }
    }
}
