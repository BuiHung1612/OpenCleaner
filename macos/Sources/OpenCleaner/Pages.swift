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

struct HistoryPage: View {
    @State private var sessions: [JSON]?
    @State private var log: String?
    @State private var error: String?
    @State private var putBackNote: String?

    /// The engine logs a session per tool run; show the tool's own name.
    static func title(_ command: String) -> String {
        let id = switch command {
        case "projects": "purge"
        case "installers": "installer"
        default: command
        }
        switch id {
        case "uninstall": return String(localized: "Uninstaller")
        case "space_lens": return String(localized: "Space Lens")
        default: return Tools.all.first { $0.id == id }?.title ?? id.capitalized
        }
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
                    ForEach(Array((sessions ?? []).enumerated()), id: \.offset) { _, s in
                        HStack {
                            VStack(alignment: .leading, spacing: 2) {
                                Text(verbatim: Self.title(s.command.string ?? "?")).font(.system(size: 14, weight: .semibold))
                                Text(verbatim: Self.when(s)).font(.system(size: 12)).foregroundStyle(.dim)
                            }
                            Spacer()
                            Text("\(s.items.string ?? "0") items · \(bytes(s.bytes.double))").monospacedDigit()
                            // what this session moved to the Trash and is still there
                            if !s.trashed.array.isEmpty {
                                Button(String(localized: "Put Back")) { putBack(s.trashed.array) }
                                    .buttonStyle(.pill)
                                    .help(String(localized: "Move \(s.trashed.array.count) items from the Trash back where they were"))
                            }
                        }
                        .padding(.horizontal, 14).padding(.vertical, 10)
                    }
                    if sessions == nil {
                        if let error { Text(error) } else { ProgressView().brightScroller().padding(40) }
                    }
                }
                .padding(8)
            }
            .brightScroller()
            .glassPanel()
            if let putBackNote { Text(verbatim: putBackNote).font(.system(size: 12)).foregroundStyle(Color(hex: "#FFD54F")) }
            if let log {
                Text("Every deleted file is listed in \((log as NSString).abbreviatingWithTildeInPath)")
                    .font(.system(size: 12)).foregroundStyle(.dim)
            }
        }
        .padding(.horizontal, 32).padding(.bottom, 28)
        .task { load() }
    }

    private func putBack(_ items: [JSON]) {
        let request = items.map { ["from": $0.from.string ?? "", "to": $0.to.string ?? ""] }
        let reply = Engine.call(["op": "put_back", "items": request])
        let failed = reply.ok.bool ? reply.results.strings.filter { $0 != "ok" } : [reply.error.string ?? "?"]
        putBackNote = failed.isEmpty ? nil : String(localized: "\(failed.count) items were not put back: \(failed[0])")
        load()
    }

    private func load() {
        let history = Engine.call(["op": "history"])
        if history.ok.bool {
            sessions = history.sessions.array
            log = history.logs.deletions.string
        } else {
            error = history.error.string ?? String(localized: "The engine did not answer.")
            sessions = []
        }
    }

    static func when(_ s: JSON) -> String {
        s.started_unix.double.map { Date(timeIntervalSince1970: $0).formatted(date: .abbreviated, time: .shortened) } ?? ""
    }
}

struct SettingsView: View {
    @AppStorage("autoRefresh") private var autoRefresh = false
    @AppStorage("refreshInterval") private var refreshInterval = 5
    @AppStorage("language") private var language = ""
    @AppStorage(Updater.autoKey) private var checkUpdates = true
    @AppStorage(Engine.fastKey) private var fastScans = false
    @State private var languageChanged = false
    @State private var exclusions: [String] = []
    @AppStorage("largeMiB") private var largeMiB = 500
    @AppStorage("idleDays") private var idleDays = 180

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
            Section(String(localized: "Excluded folders")) {
                ForEach(exclusions, id: \.self) { path in
                    HStack {
                        Text(verbatim: (path as NSString).abbreviatingWithTildeInPath).lineLimit(1).truncationMode(.middle)
                        Spacer()
                        Button(String(localized: "Remove")) { saveExclusions(exclusions.filter { $0 != path }) }
                    }
                }
                Button(String(localized: "Add Folder…"), action: addExclusion)
                Text("OpenCleaner never scans or removes anything inside these folders.")
                    .font(.system(size: 12)).foregroundStyle(.dim)
            }
            Section(String(localized: "Large & Old Files")) {
                Picker(String(localized: "At least"), selection: $largeMiB) {
                    ForEach([100, 250, 500, 1000, 5000], id: \.self) { Text(verbatim: bytes(Double($0) * 1_048_576)).tag($0) }
                }
                Picker(String(localized: "Not opened for"), selection: $idleDays) {
                    ForEach([30, 90, 180, 365], id: \.self) { Text("\($0) days").tag($0) }
                }
            }
            Section(String(localized: "Engine")) {
                Toggle(String(localized: "Faster scans"), isOn: $fastScans)
                    .onChange(of: fastScans) { Engine.setFast(fastScans) }
                Text("Off (recommended): scans and removals give way to your other apps, so the Mac stays smooth. On: they take every CPU core to finish sooner, and other apps may lag or freeze until they are done.")
                    .font(.system(size: 12)).foregroundStyle(.dim)
                LabeledContent(String(localized: "OpenCleaner engine")) {
                    Text(verbatim: Engine.version ?? "—").font(.system(size: 11, design: .monospaced))
                }
            }
        }
        .formStyle(.grouped)
        .frame(width: 520)
        .task { exclusions = Engine.call(["op": "exclusions"]).paths.strings }
    }

    private func addExclusion() {
        let panel = NSOpenPanel()
        panel.canChooseFiles = false
        panel.canChooseDirectories = true
        panel.allowsMultipleSelection = true
        panel.prompt = String(localized: "Exclude")
        guard panel.runModal() == .OK else { return }
        saveExclusions(exclusions + panel.urls.map(\.path))
    }

    private func saveExclusions(_ paths: [String]) {
        let reply = Engine.call(["op": "set_exclusions", "paths": paths])
        if reply.ok.bool { exclusions = reply.paths.strings }
    }

    private func relaunch() {
        let config = NSWorkspace.OpenConfiguration()
        config.createsNewApplicationInstance = true
        NSWorkspace.shared.openApplication(at: Bundle.main.bundleURL, configuration: config) { _, _ in
            DispatchQueue.main.async { NSApp.terminate(nil) }
        }
    }
}

/// System status. One-shot reads (launch, focus, reload button) bump `replay` so the dashboard
/// plays its intro animation; the optional `--watch` stream updates values quietly.
@MainActor
final class StatusStream: ObservableObject {
    @Published var snapshot = JSON(nil)
    @Published var error: String?
    @Published private(set) var replay = 0
    @Published private(set) var loading = false
    @Published private(set) var updatedAt: Date?
    private var ticker: Task<Void, Never>?
    private var lastRead = Date.distantPast

    /// One engine reading, off the main thread (the first one samples CPU for 250 ms).
    private static func read() async -> JSON {
        await Task.detached(priority: .userInitiated) { Engine.call(["op": "status"]) }.value
    }

    /// Focus-driven reads pass `ifOlderThan` so switching windows does not re-read every time.
    func refresh(ifOlderThan age: TimeInterval = 0) {
        guard !loading, Date().timeIntervalSince(lastRead) >= age else { return }
        loading = true
        Task {
            let json = await Self.read()
            if json.ok.bool {
                snapshot = json
                replay += 1
                updatedAt = Date()
                error = nil
            } else {
                error = json.error.string ?? String(localized: "The engine did not answer.")
            }
            loading = false
            lastRead = Date()
        }
    }

    /// Quiet updates every `seconds` (no intro animation) until `stop`.
    func start(every seconds: Int) {
        guard ticker == nil else { return }
        ticker = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(seconds))
                guard !Task.isCancelled else { return }
                let json = await Self.read()
                guard let self, json.ok.bool else { continue }
                self.snapshot = json
                self.updatedAt = Date()
                self.lastRead = Date()
            }
        }
    }

    func stop() {
        ticker?.cancel()
        ticker = nil
    }
}
