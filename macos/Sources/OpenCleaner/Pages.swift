import AppKit
import Charts
import SwiftUI

struct DashboardPage: View {
    /// owned by the window, so coming back to the dashboard shows the last reading at once
    let stream: StatusStream
    /// last results of other pages, for suggestions (nothing is scanned for them)
    let junk: JunkModel
    let devtools: DevToolsModel
    @AppStorage("autoRefresh") private var autoRefresh = false
    @AppStorage("refreshInterval") private var refreshInterval = 5
    @AppStorage("refreshOnFocus") private var refreshOnFocus = false
    /// what History counts as freed this month (UTC month, as History does)
    @State private var freedThisMonth: Double?

    var body: some View {
        let s = stream.snapshot
        ScrollView {
            VStack(alignment: .leading, spacing: 20) {
                let hw = s.hardware
                PageHeader(art: "dashboard", symbol: Module.dashboard.icon, title: String(localized: "Dashboard"),
                           status: [hw.model, hw.cpu_model, hw.total_ram, hw.os_version].compactMap(\.string).filter { !$0.isEmpty }.joined(separator: " · ")) {
                    Button { stream.refresh() } label: {
                        Label(String(localized: "Refresh"), systemImage: "arrow.clockwise").labelStyle(.iconOnly)
                            .symbolEffect(.rotate, isActive: stream.loading)
                    }
                    .buttonStyle(.pill)
                    .disabled(stream.loading)
                    .help(String(localized: "Refresh"))
                }
                HStack(spacing: 16) {
                    if let up = s.uptime_secs.double.map(duration) ?? s.uptime.string { Text("Up for \(up)") }
                    if let t = stream.updatedAt { Text("Updated \(t.formatted(date: .omitted, time: .standard))") }
                    if let freed = freedThisMonth, freed > 0 {
                        Button { Automation.shared.openModule = .history } label: {
                            Text("Freed \(bytes(freed)) this month")
                        }
                        .buttonStyle(.plain)
                        .help(String(localized: "Open History"))
                    }
                }
                .task { freedThisMonth = Self.freedThisMonth(await Engine.ask(["op": "history"])) }
                .font(.app(13)).foregroundStyle(.dim)
                .padding(.top, -10)

                if let error = stream.error { ErrorBanner(message: error) { stream.refresh() } }

                GlassEffectContainer(spacing: 16) {
                    // exactly the four tiles, sharing the width (adaptive columns left an empty fifth slot)
                    LazyVGrid(columns: Array(repeating: GridItem(.flexible(), spacing: 16), count: 4), spacing: 16) {
                        Tile(replay: stream.replay, title: String(localized: "CPU"), icon: "cpu", value: percent(s.cpu.usage.double), fraction: (s.cpu.usage.double ?? 0) / 100,
                             detail: Self.load(s.cpu))
                            .help(s.cpu.load1.double.map { String(localized: "Load average \($0.formatted(.number.precision(.fractionLength(1)))) on \(Int(s.cpu.cores.double ?? 0)) cores") } ?? "")
                        Tile(replay: stream.replay, title: String(localized: "Memory"), icon: "memorychip", value: percent(s.memory.used_percent.double), fraction: (s.memory.used_percent.double ?? 0) / 100,
                             detail: "\(memBytes(s.memory.used.double)) / \(memBytes(s.memory.total.double))")
                        let disk = s.disks.array.first { $0.mount.string == "/" }
                        Tile(replay: stream.replay, title: String(localized: "Disk"), icon: "internaldrive", value: percent(disk?.used_percent.double), fraction: (disk?.used_percent.double ?? 0) / 100,
                             detail: String(localized: "\(bytes(disk.flatMap { d in d.total.double.flatMap { t in d.used.double.map { t - $0 } } })) free"))
                        trashTile(s.trash_size.double)
                    }
                }

                let tips = Self.suggestions(status: s, junkBytes: junk.phase == .review ? junk.selectedBytes : nil,
                                            devtoolsBytes: devtools.reclaimable)
                if !tips.isEmpty { suggestions(tips) }

                VStack(alignment: .leading, spacing: 10) {
                    Text("Top processes").font(.app(15, weight: .semibold))
                    ForEach(Array(s.top_processes.array.prefix(6).enumerated()), id: \.offset) { _, p in
                        HStack {
                            Text(verbatim: p.name.string ?? "?").lineLimit(1)
                            Spacer()
                            Text(verbatim: "\(percent(p.cpu.double)) CPU · \(memBytes(p.memory_bytes.double))").monospacedDigit().foregroundStyle(.dim)
                        }
                        .font(.app(13))
                    }
                }
                .padding(20)
                .glassPanel()
            }
            .padding(.horizontal, Layout.side).padding(.bottom, 32)
        }
        // glass tiles live inside this scroll view; a dark scheme here would darken them
        .scrollIndicators(.never)
        .onAppear {
            stream.refresh()
            if autoRefresh { stream.start(every: refreshInterval) }
        }
        .onDisappear { stream.stop() }
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in
            if refreshOnFocus { stream.refresh(ifOlderThan: 10) }
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

    struct Suggestion: Equatable {
        let symbol: String
        let text: String
        let module: Module
        /// System Junk opens on this group only ("trash"); nil: everything
        var group: String?
    }

    /// This month's bytes from History's months ("2026-11"), in UTC like the ledger.
    static func freedThisMonth(_ history: JSON, now: Date = Date()) -> Double? {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = TimeZone(identifier: "UTC")!
        let c = calendar.dateComponents([.year, .month], from: now)
        let month = String(format: "%04d-%02d", c.year ?? 0, c.month ?? 0)
        return history.months.array.first { $0.month.string == month }?.bytes.double
    }

    /// Up to three things worth doing, from the status reading and results already at hand.
    static func suggestions(status s: JSON, junkBytes: Double?, devtoolsBytes: Double) -> [Suggestion] {
        let gb = 1e9
        var out: [Suggestion] = []
        if let disk = s.disks.array.first(where: { $0.mount.string == "/" }),
           let total = disk.total.double, let used = disk.used.double, total > 0, (total - used) / total < 0.1 {
            out.append(.init(symbol: "internaldrive", text: String(localized: "Your disk is almost full: \(bytes(total - used)) free"), module: .spaceLens))
        }
        if let trash = s.trash_size.double, trash > gb {
            out.append(.init(symbol: "trash", text: String(localized: "Empty the Trash (\(bytes(trash)))"), module: .tool("clean"), group: "trash"))
        }
        if let junk = junkBytes, junk > 2 * gb {
            out.append(.init(symbol: "sparkles", text: String(localized: "Clean \(bytes(junk)) of junk"), module: .tool("clean")))
        }
        if devtoolsBytes > 2 * gb {
            out.append(.init(symbol: "hammer", text: String(localized: "Free \(bytes(devtoolsBytes)) from Docker and simulators"), module: .tool("devtools")))
        }
        return Array(out.prefix(3))
    }

    /// The tip's page; for System Junk, limited to its group and scanned if it has not been.
    private func open(_ tip: Suggestion) {
        if tip.module == .tool("clean") {
            junk.onlyGroup = tip.group
            junk.tickGroup = tip.group
            if tip.group != nil, junk.phase == .idle || junk.phase == .done { junk.scan() }
            if junk.phase == .review { junk.applyTickGroup() }
        }
        Automation.shared.openModule = tip.module
    }

    private func suggestions(_ tips: [Suggestion]) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text("Suggestions").font(.app(15, weight: .semibold)).padding(.bottom, 6)
            ForEach(tips, id: \.text) { tip in
                Button { open(tip) } label: {
                    HStack(spacing: 10) {
                        Image(systemName: tip.symbol).frame(width: 22).accessibilityHidden(true)
                        Text(verbatim: tip.text)
                        Spacer()
                        Image(systemName: "chevron.right").foregroundStyle(.dim)
                    }
                    .font(.app(13))
                    .padding(.vertical, 6)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }
        }
        .padding(20)
        .glassPanel()
    }

    /// The load average in words (not the CPU percentage above it, which can be high while
    /// the load is normal): more runnable work than cores means the Mac is busy.
    static func load(_ cpu: JSON) -> String? {
        guard let load = cpu.load1.double else { return nil }
        let cores = max(cpu.cores.double ?? 1, 1)
        return load > cores ? String(localized: "Load average high: more work than cores") : String(localized: "Load average is normal")
    }

    /// Unknown size: macOS hides the Trash from apps without Full Disk Access.
    private func trashTile(_ size: Double?) -> some View {
        let value = switch size {
        case nil: "—"
        case 0: String(localized: "Empty")
        case let n: bytes(n)
        }
        return Tile(title: String(localized: "Trash"), icon: "trash", value: value, fraction: nil,
                    detail: stream.trashLoading ? String(localized: "Measuring…") : size == nil ? String(localized: "Needs Full Disk Access") : nil) {
            if size == nil && !stream.trashLoading {
                // where the user grants it; the reading works after the app is reopened
                Button(String(localized: "Grant Access…"), action: openFullDiskAccess).buttonStyle(.link).font(.app(13))
            } else {
                Button(String(localized: "Open Trash"), action: openTrash).buttonStyle(.link).font(.app(13))
            }
        }
    }
}

/// System Settings › Privacy & Security › Full Disk Access.
func openFullDiskAccess() {
    if let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles") {
        NSWorkspace.shared.open(url)
    }
}

struct Tile<Accessory: View>: View {
    var replay = 0
    let title: String
    let icon: String
    let value: String
    let fraction: Double?
    let detail: String?
    @ViewBuilder var accessory: Accessory

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Label(title, systemImage: icon).font(.app(13, weight: .medium)).foregroundStyle(.dim)
            Text(verbatim: value).font(.app(28, weight: .bold)).monospacedDigit().lineLimit(1).minimumScaleFactor(0.6)
            if let fraction { UsageBar(fraction: fraction).id(replay) }
            if let detail { Text(verbatim: detail).font(.app(13)).foregroundStyle(.dim) }
            accessory
        }
        .frame(maxWidth: .infinity, minHeight: 120, alignment: .topLeading)
        .padding(18)
        .glassPanel()
        .accessibilityElement(children: .combine)
    }
}

extension Tile where Accessory == EmptyView {
    init(replay: Int = 0, title: String, icon: String, value: String, fraction: Double?, detail: String?) {
        self.init(replay: replay, title: title, icon: icon, value: value, fraction: fraction, detail: detail) { EmptyView() }
    }
}

struct HistoryPage: View {
    @State private var sessions: [JSON]?
    /// freed per month, oldest first (`{month: "2026-11", bytes, sessions}`)
    @State private var months: [JSON] = []
    @State private var log: String?
    @State private var error: String?
    @State private var putBackNote: String?
    /// sessions (by start time) whose skipped and failed items are shown
    @State private var opened: Set<String> = []

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
            PageHeader(art: "history", symbol: Module.history.icon, title: String(localized: "History"))
            if !months.isEmpty { monthly }
            ScrollView {
                VStack(spacing: 4) {
                    ForEach(Array((sessions ?? []).enumerated()), id: \.offset) { _, s in
                        let key = "\(s.started_unix.double ?? 0)-\(s.command.string ?? "")-\(s.items.double ?? 0)-\(s.bytes.double ?? 0)"
                        let problems = s.problems.array
                        VStack(alignment: .leading, spacing: 6) {
                        HStack {
                            Image(systemName: Self.icon(s.command.string ?? "")).frame(width: 22).foregroundStyle(.dim)
                                .accessibilityHidden(true)
                            VStack(alignment: .leading, spacing: 2) {
                                Text(verbatim: Self.title(s.command.string ?? "?")).font(.app(14, weight: .semibold))
                                Text(verbatim: Self.when(s)).font(.app(12)).foregroundStyle(.dim)
                            }
                            Spacer()
                            Text("\(Int(s.items.double ?? 0)) items · \(bytes(s.bytes.double))").monospacedDigit()
                            // a run where everything failed (an app that needs admin rights) says so
                            if let failed = s.failed.double, failed > 0 {
                                Text("\(Int(failed)) failed").monospacedDigit().foregroundStyle(Color.warn)
                            }
                            // why items were skipped or failed, one click away
                            if !problems.isEmpty {
                                Button {
                                    if opened.contains(key) { opened.remove(key) } else { opened.insert(key) }
                                } label: {
                                    Image(systemName: opened.contains(key) ? "chevron.up" : "chevron.down")
                                }
                                .buttonStyle(.plain)
                                .help(String(localized: "Show what was skipped or not removed, and why"))
                                .accessibilityLabel(String(localized: "Show what was skipped or not removed, and why"))
                            }
                            // what this session moved to the Trash and is still there
                            if !s.trashed.array.isEmpty {
                                Button(String(localized: "Put Back")) { Task { await putBack(s.trashed.array) } }
                                    .buttonStyle(.pill)
                                    .help(String(localized: "Move \(s.trashed.array.count) items from the Trash back where they were"))
                            }
                        }
                        if opened.contains(key) {
                            ForEach(Array(problems.enumerated()), id: \.offset) { _, p in
                                let path = p.path.string ?? ""
                                HStack(alignment: .firstTextBaseline, spacing: 8) {
                                    Text(verbatim: (path as NSString).lastPathComponent).font(.app(12, weight: .semibold))
                                        .help(path)
                                    Text(tr(p.reason.string ?? p.result.string ?? ""))
                                        .font(.app(12))
                                        .foregroundStyle(p.result.string == "failed" ? Color.warn : Color.dim)
                                    Spacer()
                                }
                                .padding(.leading, 30)
                            }
                        }
                        }
                        .padding(.horizontal, 14).padding(.vertical, 10)
                    }
                    if sessions == nil {
                        ProgressView().brightScroller().padding(40)
                    } else if let error {
                        ErrorBanner(message: error) { Task { await load() } }
                    } else if sessions?.isEmpty == true {
                        EmptyState(symbol: "clock", title: String(localized: "Nothing cleaned yet"),
                                   message: String(localized: "Every cleanup and removal is listed here, with a way to put back what went to the Trash."))
                    }
                }
                .padding(8)
            }
            .brightScroller()
            .glassPanel()
            if let putBackNote { Text(verbatim: putBackNote).font(.app(12)).foregroundStyle(Color.warn) }
            if let log {
                HStack {
                    Text("Every deleted file is listed in \((log as NSString).abbreviatingWithTildeInPath)")
                        .font(.app(12)).foregroundStyle(.dim)
                    Button(String(localized: "Show in Finder")) { reveal(log) }.buttonStyle(.link).font(.app(12))
                }
            }
        }
        .padding(.horizontal, Layout.side).padding(.bottom, Layout.bottom)
        .task { await load() }
    }

    /// The sidebar icon of the tool that made a session.
    static func icon(_ command: String) -> String {
        switch command {
        case "uninstall": Module.uninstaller.icon
        case "space_lens": Module.spaceLens.icon
        case "projects": Module.tool("purge").icon
        case "installers": Module.tool("installer").icon
        default: Module.tool(command).icon
        }
    }

    private func putBack(_ items: [JSON]) async {
        let request = items.map { ["from": $0.from.string ?? "", "to": $0.to.string ?? ""] }
        let reply = await Engine.ask(["op": "put_back", "items": request])
        let failed = reply.ok.bool ? reply.results.strings.filter { $0 != "ok" } : [reply.error.string ?? "?"]
        putBackNote = failed.isEmpty ? nil : String(localized: "\(failed.count) items were not put back: \(failed[0])")
        await load()
    }

    private func load() async {
        let history = await Engine.ask(["op": "history"])
        error = nil
        if history.ok.bool {
            sessions = history.sessions.array
            months = history.months.array
            log = history.logs.deletions.string
        } else {
            error = history.error.string ?? String(localized: "The engine did not answer.")
            sessions = []
        }
    }

    /// Freed per month (the last 12), with this month's and the all-time total.
    private var monthly: some View {
        let current = Self.month.string(from: Date())
        let thisMonth = months.first { $0.month.string == current }?.bytes.double ?? 0
        let allTime = months.compactMap(\.bytes.double).reduce(0, +)
        return VStack(alignment: .leading, spacing: 12) {
            HStack {
                Text("Freed this month: \(bytes(thisMonth))").font(.app(14, weight: .semibold))
                Spacer()
                Text("All time: \(bytes(allTime))").font(.app(13)).foregroundStyle(.dim)
            }
            Chart(Array(months.suffix(12)), id: \.month.string) { m in
                // a lone month would otherwise draw one bar across the whole chart
                BarMark(x: .value("Month", Self.label(m.month.string)), y: .value("Freed", m.bytes.double ?? 0), width: .fixed(48))
                    .foregroundStyle(.white.opacity(0.85))
                    .cornerRadius(4)
            }
            .chartYAxis {
                AxisMarks { v in
                    AxisGridLine().foregroundStyle(.white.opacity(0.2))
                    AxisValueLabel { Text(verbatim: bytes(v.as(Double.self))).foregroundStyle(.dim) }
                }
            }
            .chartXAxis {
                AxisMarks { _ in AxisValueLabel().foregroundStyle(.dim) }
            }
            .frame(height: 130)
        }
        .padding(16)
        .glassPanel()
    }

    /// The engine counts months in UTC; so does this.
    static let month: DateFormatter = {
        let f = DateFormatter()
        f.locale = Locale(identifier: "en_US_POSIX")
        f.timeZone = TimeZone(identifier: "UTC")
        f.dateFormat = "yyyy-MM"
        return f
    }()

    /// "2026-11" -> "Nov 2026" in the user's language.
    static func label(_ month: String?) -> String {
        guard let date = month.flatMap(Self.month.date(from:)) else { return month ?? "" }
        var style = Date.FormatStyle.dateTime.month(.abbreviated).year()
        style.timeZone = TimeZone(identifier: "UTC") ?? .current
        return date.formatted(style)
    }

    static func when(_ s: JSON) -> String {
        s.started_unix.double.map { Date(timeIntervalSince1970: $0).formatted(date: .abbreviated, time: .shortened) } ?? ""
    }
}

/// Settings, a page of the main window: tabs as glass pills, each tab a short grouped form
/// with every explanation in its section's footer. The forms are drawn dark (white text) and
/// without their own background, so the page's gradient shows through like everywhere else.
struct SettingsPage: View {
    enum Pane: CaseIterable {
        case general, cleaning, automation, dashboard, permissions
        var title: String {
            switch self {
            case .general: String(localized: "General")
            case .cleaning: String(localized: "Cleaning")
            case .automation: String(localized: "Automation")
            case .dashboard: String(localized: "Dashboard")
            case .permissions: String(localized: "Permissions")
            }
        }
        var icon: String {
            switch self {
            case .general: "gearshape"
            case .cleaning: "sparkles"
            case .automation: "calendar.badge.clock"
            case .dashboard: "gauge.with.dots.needle.50percent"
            case .permissions: "lock.shield"
            }
        }
    }

    @SceneStorage("settingsPane") private var paneName = "general"
    private var pane: Pane { Pane.allCases.first { "\($0)" == paneName } ?? .general }

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            PageHeader(art: "settings", symbol: Module.settings.icon, title: String(localized: "Settings"))
            HStack(spacing: 8) {
                ForEach(Pane.allCases, id: \.self) { p in
                    Button { paneName = "\(p)" } label: {
                        Label(p.title, systemImage: p.icon)
                            .font(.app(13, weight: p == pane ? .semibold : .regular))
                            .padding(.horizontal, 14).padding(.vertical, 7)
                            .background(Capsule().fill(.white.opacity(p == pane ? 0.22 : 0.06)))
                            .overlay(Capsule().strokeBorder(.white.opacity(0.18)))
                            .contentShape(Capsule())
                    }
                    .buttonStyle(.plain)
                    .accessibilityAddTraits(p == pane ? .isSelected : [])
                }
            }
            GeometryReader { page in ScrollView {
                Group {
                    switch pane {
                    case .general: GeneralSettings()
                    case .cleaning: CleaningSettings()
                    case .automation: AutomationSettings()
                    case .dashboard: DashboardSettings()
                    case .permissions: PermissionsSettings()
                    }
                }
                .scrollContentBackground(.hidden)
                .padding(.top, -12)
                // a grouped form keeps about 31 points round its sections and centres them at
                // 704 points at most: sized to that and moved left, the sections line up with
                // the tabs and the header above instead of floating in the middle
                .frame(width: min(page.size.width, 704) + 62)
                .offset(x: -31)
                .frame(width: page.size.width, alignment: .leading)
            }
            }
            // grouped forms in the dark look: white text on translucent rows over the gradient;
            // menus in the window's white, switches drawn to match the glass
            .brightScroller()
            .toggleStyle(GlassSwitch())
            .id(pane)
        }
        .padding(.horizontal, Layout.side)
        .padding(.bottom, Layout.bottom)
    }
}

/// A footer note in Settings.
struct Note: View {
    let text: LocalizedStringKey
    init(_ text: LocalizedStringKey) { self.text = text }
    var body: some View { Text(text).font(.app(12)).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true) }
}

private struct GeneralSettings: View {
    @AppStorage("language") private var language = ""
    @AppStorage(Automation.menuBarKey) private var menuBar = true
    @AppStorage(Updater.autoKey) private var checkUpdates = true
    @State private var languageChanged = false
    @State private var engineVersion: String?
    @State private var textScale = TextScale.shared

    var body: some View {
        Form {
            Section {
                Picker(String(localized: "Language"), selection: $language) {
                    Text("System").tag("")
                    Text(verbatim: "English").tag("en")
                    Text(verbatim: "Tiếng Việt").tag("vi")
                }
                .onChange(of: language) {
                    UserDefaults.standard.set(language.isEmpty ? nil : [language], forKey: "AppleLanguages")
                    languageChanged = true
                }
                Picker(String(localized: "Text size"), selection: $textScale.value) {
                    ForEach(TextScale.choices, id: \.scale) { Text($0.name).tag($0.scale) }
                }
                Toggle(String(localized: "Show in menu bar"), isOn: $menuBar)
            } footer: {
                if languageChanged {
                    HStack {
                        Note("The new language shows after a relaunch.")
                        Spacer()
                        Button(String(localized: "Relaunch"), action: relaunch)
                    }
                }
            }
            Section(String(localized: "Updates")) {
                Toggle(String(localized: "Check for updates automatically"), isOn: $checkUpdates)
                LabeledContent(String(localized: "Version")) {
                    HStack(spacing: 12) {
                        Text(verbatim: Updater.current).foregroundStyle(.secondary)
                        Button(String(localized: "Check Now")) { Task { await Updater.check(manual: true) } }
                    }
                }
                LabeledContent(String(localized: "OpenCleaner engine")) {
                    Text(verbatim: engineVersion ?? "—").foregroundStyle(.secondary)
                }
            }
        }
        .formStyle(.grouped)
        .fixedSize(horizontal: false, vertical: true)
        // each tab fits its window; .hidden still shows them when a mouse is attached
        // (System Settings › Show scroll bars: Automatic)
        .scrollIndicators(.never)
        .task { engineVersion = await Engine.ask(["op": "version"]).version.string }
    }

    private func relaunch() { relaunchApp() }
}

/// Starts a new copy of the app, then quits this one (stays open if the copy did not start).
func relaunchApp() {
    let config = NSWorkspace.OpenConfiguration()
    config.createsNewApplicationInstance = true
    NSWorkspace.shared.openApplication(at: Bundle.main.bundleURL, configuration: config) { app, _ in
        guard app != nil else { return }
        DispatchQueue.main.async { NSApp.terminate(nil) }
    }
}

private struct CleaningSettings: View {
    @AppStorage(Engine.fastKey) private var fastScans = false
    @AppStorage("largeMiB") private var largeMiB = 500
    @AppStorage("idleDays") private var idleDays = 180
    @State private var exclusions: [String] = []
    /// The saved list could not be read: the engine removes nothing until it is saved again.
    @State private var exclusionsError: String?

    var body: some View {
        Form {
            Section {
                Toggle(String(localized: "Faster scans"), isOn: $fastScans)
                    .onChange(of: fastScans) { Engine.setFast(fastScans) }
            } footer: {
                Note("Off (recommended): scans and removals give way to your other apps, so the Mac stays smooth. On: they take every CPU core to finish sooner, and other apps may lag or freeze until they are done.")
            }
            Section {
                if let exclusionsError {
                    Label(exclusionsError, systemImage: "exclamationmark.triangle.fill")
                        .foregroundStyle(.red)
                    Text("Nothing is removed until the list is saved again: add or remove a folder.")
                        .foregroundStyle(.secondary)
                }
                if exclusions.isEmpty {
                    Text("No excluded folders").foregroundStyle(.secondary)
                }
                ForEach(exclusions, id: \.self) { path in
                    HStack(spacing: 10) {
                        Image(nsImage: fileIcon(path)).resizable().frame(width: 18, height: 18)
                        Text(verbatim: (path as NSString).abbreviatingWithTildeInPath).lineLimit(1).truncationMode(.middle)
                        Spacer()
                        Button { saveExclusions(exclusions.filter { $0 != path }) } label: {
                            Image(systemName: "minus.circle.fill").foregroundStyle(.secondary)
                        }
                        .buttonStyle(.plain)
                        .help(String(localized: "Remove"))
                        .accessibilityLabel(String(localized: "Remove"))
                    }
                }
            } header: {
                HStack {
                    Text("Excluded folders")
                    Spacer()
                    Button(String(localized: "Add Folder…"), action: addExclusion).controlSize(.small)
                }
            } footer: {
                Note("OpenCleaner never scans or removes anything inside these folders.")
            }
            Section(String(localized: "Large & Old Files")) {
                Picker(String(localized: "At least"), selection: $largeMiB) {
                    ForEach([100, 250, 500, 1000, 5000], id: \.self) { mib in
                        Text(verbatim: mib < 1000 ? "\(mib) MB" : "\(mib / 1000) GB").tag(mib)
                    }
                }
                Picker(String(localized: "Not opened for"), selection: $idleDays) {
                    ForEach([30, 90, 180, 365], id: \.self) { Text("\($0) days").tag($0) }
                }
            }
        }
        .formStyle(.grouped)
        .fixedSize(horizontal: false, vertical: true)
        // each tab fits its window; .hidden still shows them when a mouse is attached
        // (System Settings › Show scroll bars: Automatic)
        .scrollIndicators(.never)
        .task {
            let reply = await Engine.ask(["op": "exclusions"])
            exclusions = reply.paths.strings
            exclusionsError = reply.ok.bool ? nil : reply.error.string
        }
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
        Task {
            let reply = await Engine.ask(["op": "set_exclusions", "paths": paths])
            if reply.ok.bool {
                exclusions = reply.paths.strings
                exclusionsError = nil
            }
        }
    }
}

private struct DashboardSettings: View {
    @AppStorage("refreshOnFocus") private var refreshOnFocus = false
    @AppStorage("autoRefresh") private var autoRefresh = false
    @AppStorage("refreshInterval") private var refreshInterval = 5

    var body: some View {
        Form {
            Section {
                Toggle(String(localized: "Refresh when the app comes to the front"), isOn: $refreshOnFocus)
            } header: {
                Note("By default the dashboard reads once, the first time you open it. Press the refresh button for new numbers, so they may be minutes old.")
            } footer: {
                Note("On: numbers are fresh whenever you come back to the app. Cost: every switch back takes a short CPU sample and redraws the dashboard (at most once per 10 seconds), which uses a little battery.")
            }
            Section {
                Toggle(String(localized: "Refresh automatically"), isOn: $autoRefresh)
                Picker(String(localized: "Every"), selection: $refreshInterval) {
                    ForEach([3, 5, 10, 30], id: \.self) { Text("\($0) seconds").tag($0) }
                }
                .disabled(!autoRefresh)
            } footer: {
                Note("On: live numbers while the dashboard is on screen (paused when hidden). Cost: the app keeps working in the background the whole time, which uses CPU and battery and adds a little load to what it measures.")
            }
        }
        .formStyle(.grouped)
        .fixedSize(horizontal: false, vertical: true)
        // each tab fits its window; .hidden still shows them when a mouse is attached
        // (System Settings › Show scroll bars: Automatic)
        .scrollIndicators(.never)
    }
}

/// System status. One-shot reads (launch, focus, reload button) bump `replay` so the dashboard
/// plays its intro animation; the optional `--watch` stream updates values quietly.
@MainActor @Observable
final class StatusStream {
    var snapshot = JSON(nil)
    var error: String?
    private(set) var replay = 0
    private(set) var loading = false
    private(set) var trashLoading = false
    private(set) var updatedAt: Date?
    @ObservationIgnored private var ticker: Task<Void, Never>?
    @ObservationIgnored private var lastRead = Date.distantPast
    @ObservationIgnored private let readStatus: () async -> JSON
    @ObservationIgnored private let readTrash: () async -> JSON

    init(readStatus: @escaping () async -> JSON = { await Engine.ask(["op": "status", "skip_trash": true]) },
         readTrash: @escaping () async -> JSON = { await Engine.ask(["op": "trash_size"]) }) {
        self.readStatus = readStatus
        self.readTrash = readTrash
    }

    private func apply(_ json: JSON) {
        var fields = json.raw as? [String: Any] ?? [:]
        fields["trash_size"] = snapshot.trash_size.raw
        snapshot = JSON(fields)
        updatedAt = Date()
        lastRead = Date()
    }

    /// A large Trash must not hold live metrics or the Refresh button hostage.
    private func refreshTrash() {
        guard !trashLoading else { return }
        trashLoading = true
        Task {
            let json = await readTrash()
            var fields = snapshot.raw as? [String: Any] ?? [:]
            fields["trash_size"] = json.trash_size.raw
            snapshot = JSON(fields)
            trashLoading = false
        }
    }

    /// Focus-driven reads pass `ifOlderThan` so switching windows does not re-read every time.
    func refresh(ifOlderThan age: TimeInterval = 0, quiet: Bool = false) {
        guard !loading, Date().timeIntervalSince(lastRead) >= age else { return }
        loading = true
        refreshTrash()
        Task {
            let json = await readStatus()
            if json.ok.bool {
                apply(json)
                if !quiet { replay += 1 }
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
                self?.refresh(quiet: true)
            }
        }
    }

    func stop() {
        ticker?.cancel()
        ticker = nil
    }
}
