//! Automation while the app or its menu bar item runs: scheduled System Junk cleaning, the menu
//! bar item, the low disk alert and the Trash / Downloads watch. Checked on launch, free space
//! every minute, the rest every hour. No launchd agent: nothing runs when the app is quit.

import AppKit
import SwiftUI
import UserNotifications

/// The pure parts, tested in AutomationTests.
enum AutoClean {
    /// Rules an unattended clean may touch: selected by default, something to remove, never the Trash.
    static func safeRules(_ plan: JSON) -> [JSON] {
        plan.groups.array.flatMap(\.rules.array).filter {
            $0.default.bool && $0.status.string == "ready" && $0.id.string != nil
                && $0.id.string != "trash" && $0.group.string != "trash"
        }
    }

    static func isDue(last: Date?, now: Date, monthly: Bool, calendar: Calendar = .current) -> Bool {
        guard let last, let next = calendar.date(byAdding: monthly ? .month : .weekOfYear, value: 1, to: last) else { return true }
        return now >= next
    }

    /// One alert per dip below `threshold`; armed again once free space is 2 GB above it.
    static func lowDisk(free: Double, threshold: Double, notified: Bool) -> (post: Bool, notified: Bool) {
        if free < threshold { return (!notified, true) }
        return (false, free > threshold + 2e9 ? false : notified)
    }
}

@MainActor
final class Automation: ObservableObject {
    static let shared = Automation()

    static let autoCleanKey = "autoClean", periodKey = "autoCleanPeriod", lastCleanKey = "autoCleanLast"
    static let menuBarKey = "menuBar"
    static let lowDiskKey = "lowDiskAlert", lowDiskGBKey = "lowDiskGB", lowDiskNotifiedKey = "lowDiskNotified"
    static let watchKey = "watchFolders", watchGBKey = "watchGB"

    @Published private(set) var freeBytes: Double?
    @Published private(set) var busy = false
    /// The page the main window should show next (set by a notification click).
    @Published var openModule: Module?
    /// Set by a view that has `openWindow`, so a click can reopen a closed main window.
    var openWindow: (() -> Void)?
    private var loop: Task<Void, Never>?
    private let defaults = UserDefaults.standard

    func start() {
        guard loop == nil else { return }
        defaults.register(defaults: [Self.menuBarKey: true, Self.lowDiskKey: true, Self.lowDiskGBKey: 10, Self.watchGBKey: 5, Self.periodKey: "weekly"])
        loop = Task {
            var hourly = Date.distantPast
            while !Task.isCancelled {
                await updateFree()
                if Date().timeIntervalSince(hourly) >= 3600 {
                    hourly = Date()
                    Task { await runScheduleIfDue() }
                    Task { await watchFolders() }
                }
                try? await Task.sleep(for: .seconds(60))
            }
        }
    }

    func show(_ module: Module?) {
        openModule = module
        NSApp.activate()
        if let openWindow { openWindow() } else { NSApp.windows.first { $0.identifier?.rawValue.contains("main") == true }?.makeKeyAndOrderFront(nil) }
    }

    func updateFree() async {
        freeBytes = await Task.detached {
            (try? URL(fileURLWithPath: "/").resourceValues(forKeys: [.volumeAvailableCapacityForImportantUsageKey]))?
                .volumeAvailableCapacityForImportantUsage.map(Double.init)
        }.value
        guard let free = freeBytes, defaults.bool(forKey: Self.lowDiskKey) else { return }
        let step = AutoClean.lowDisk(free: free, threshold: Double(defaults.integer(forKey: Self.lowDiskGBKey)) * 1e9,
                                     notified: defaults.bool(forKey: Self.lowDiskNotifiedKey))
        defaults.set(step.notified, forKey: Self.lowDiskNotifiedKey)
        if step.post {
            Self.notify(String(localized: "Your startup disk is almost full"),
                        String(localized: "\(bytes(free)) free. Click to clean System Junk."), info: ["module": "clean"])
        }
    }

    func runScheduleIfDue() async {
        guard defaults.bool(forKey: Self.autoCleanKey), !busy,
              AutoClean.isDue(last: defaults.object(forKey: Self.lastCleanKey) as? Date, now: Date(),
                              monthly: defaults.string(forKey: Self.periodKey) == "monthly") else { return }
        busy = true
        defer { busy = false }
        // stamped before running: a failing clean is retried next period, not every hour
        defaults.set(Date(), forKey: Self.lastCleanKey)
        guard let found = await Self.scan() else { return }
        await finish(found)
    }

    /// Menu bar "Clean System Junk now": scan, confirm the size, clean.
    func cleanNow() async {
        guard !busy else { return }
        busy = true
        defer { busy = false }
        guard let found = await Self.scan() else {
            return Self.notify(String(localized: "Nothing to clean"), "")
        }
        NSApp.activate()
        let alert = NSAlert()
        alert.messageText = String(localized: "Remove \(bytes(found.bytes)) of junk? This cannot be undone.")
        alert.informativeText = String(localized: "Only the items selected by default. The Trash is not emptied.")
        alert.addButton(withTitle: String(localized: "Clean"))
        alert.addButton(withTitle: String(localized: "Cancel"))
        guard alert.runModal() == .alertFirstButtonReturn else { return }
        await finish(found)
    }

    private func finish(_ found: (plan: Int, rules: [String], bytes: Double)) async {
        var outcome = JSON(nil)
        for await event in Engine.run(["op": "clean", "plan": found.plan, "rules": found.rules]) where event.event.string == "cleaned" {
            outcome = event
        }
        if let error = outcome.error.string {
            Self.notify(String(localized: "Cleaning failed"), error)
        } else {
            Self.notify(String(localized: "Freed \(bytes(outcome.bytes.double ?? 0))"), "")
        }
        await updateFree()
    }

    /// A System Junk scan reduced to the safe rules; nil when there is nothing to clean.
    nonisolated private static func scan() async -> (plan: Int, rules: [String], bytes: Double)? {
        var plan = JSON(nil)
        for await event in Engine.run(["op": "scan"]) where event.event.string == "plan" { plan = event }
        let rules = AutoClean.safeRules(plan)
        guard let id = plan.plan.double, !rules.isEmpty else { return nil }
        return (Int(id), rules.compactMap(\.id.string), rules.compactMap(\.bytes.double).reduce(0, +))
    }

    /// Once a week at most per folder.
    func watchFolders() async {
        guard defaults.bool(forKey: Self.watchKey) else { return }
        let home = FileManager.default.homeDirectoryForCurrentUser
        let due = [".Trash", "Downloads"].map { home.appendingPathComponent($0).path }.filter {
            (defaults.object(forKey: "watchNotified." + $0) as? Date).map { Date().timeIntervalSince($0) >= 7 * 86400 } ?? true
        }
        guard !due.isEmpty else { return }
        let limit = Double(defaults.integer(forKey: Self.watchGBKey)) * 1e9
        var sizes: [String: Double] = [:]
        // engine job thread, not the main thread
        for await event in Engine.run(["op": "size", "paths": due]) {
            if let path = event.path.string, let b = event.report.bytes.double { sizes[path] = b }
        }
        for (path, size) in sizes where size > limit {
            defaults.set(Date(), forKey: "watchNotified." + path)
            let title = path.hasSuffix(".Trash")
                ? String(localized: "The Trash holds \(bytes(size))")
                : String(localized: "Downloads holds \(bytes(size))")
            Self.notify(title, String(localized: "Click to review it."), info: ["path": path])
        }
    }

    nonisolated static func notify(_ title: String, _ body: String, info: [String: String] = [:]) {
        guard Bundle.main.bundleIdentifier != nil else { return } // `swift run`: no bundle, no notifications
        UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound]) { ok, _ in
            guard ok else { return }
            let content = UNMutableNotificationContent()
            content.title = title
            content.body = body
            content.userInfo = info
            UNUserNotificationCenter.current().add(UNNotificationRequest(identifier: UUID().uuidString, content: content, trigger: nil))
        }
    }
}

/// Starts automation at launch, keeps the app alive for the menu bar item, routes notification clicks.
final class AppDelegate: NSObject, NSApplicationDelegate, UNUserNotificationCenterDelegate {
    func applicationDidFinishLaunching(_ notification: Notification) {
        if Bundle.main.bundleIdentifier != nil { UNUserNotificationCenter.current().delegate = self }
        Task { @MainActor in Automation.shared.start() }
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        !UserDefaults.standard.bool(forKey: Automation.menuBarKey)
    }

    func userNotificationCenter(_ center: UNUserNotificationCenter, willPresent notification: UNNotification,
                                withCompletionHandler done: @escaping (UNNotificationPresentationOptions) -> Void) {
        done([.banner, .sound])
    }

    func userNotificationCenter(_ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse,
                                withCompletionHandler done: @escaping () -> Void) {
        let info = response.notification.request.content.userInfo
        let module = info["module"] as? String
        let path = info["path"] as? String
        Task { @MainActor in
            if let path { NSWorkspace.shared.open(URL(fileURLWithPath: path)) } else { Automation.shared.show(module.map { .tool($0) }) }
            done()
        }
    }
}

// MARK: menu bar

struct MenuBarLabel: View {
    @ObservedObject var auto = Automation.shared
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        Text("\(Image(systemName: "internaldrive")) \(bytes(auto.freeBytes))")
            .onAppear { auto.openWindow = { openWindow(id: "main") } }
    }
}

struct MenuBarView: View {
    @ObservedObject var auto = Automation.shared
    @State private var status = JSON(nil)
    @State private var window: NSWindow?

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            row(String(localized: "CPU"), percent(status.cpu.usage.double))
            row(String(localized: "Memory"), percent(status.memory.used_percent.double))
            row(String(localized: "Disk"), String(localized: "\(bytes(auto.freeBytes)) free"))
            Divider()
            Button(auto.busy ? String(localized: "Cleaning…") : String(localized: "Clean System Junk now")) {
                Task { await auto.cleanNow() }
            }
            .disabled(auto.busy)
            Button(String(localized: "Open OpenCleaner")) { auto.show(nil) }
            Divider()
            Button(String(localized: "Quit OpenCleaner")) { NSApp.terminate(nil) }
        }
        .buttonStyle(.borderless)
        .padding(14)
        .frame(width: 240, alignment: .leading)
        .background(WindowReader { window = $0 })
        // the menu window is kept when it closes; read only while it shows
        .task {
            while !Task.isCancelled {
                if window?.isVisible ?? true {
                    let s = await Task.detached { Engine.call(["op": "status"]) }.value
                    if s.ok.bool { status = s }
                }
                try? await Task.sleep(for: .seconds(2))
            }
        }
    }

    private func row(_ title: String, _ value: String) -> some View {
        HStack { Text(verbatim: title); Spacer(); Text(verbatim: value).monospacedDigit().foregroundStyle(.secondary) }
    }
}

private struct WindowReader: NSViewRepresentable {
    let found: (NSWindow?) -> Void
    func makeNSView(context: Context) -> NSView {
        let view = NSView()
        DispatchQueue.main.async { found(view.window) }
        return view
    }
    func updateNSView(_ view: NSView, context: Context) {}
}

// MARK: settings

struct AutomationSettings: View {
    @AppStorage(Automation.autoCleanKey) private var autoClean = false
    @AppStorage(Automation.periodKey) private var period = "weekly"
    @AppStorage(Automation.menuBarKey) private var menuBar = true
    @AppStorage(Automation.lowDiskKey) private var lowDisk = true
    @AppStorage(Automation.lowDiskGBKey) private var lowDiskGB = 10
    @AppStorage(Automation.watchKey) private var watch = false
    @AppStorage(Automation.watchGBKey) private var watchGB = 5

    var body: some View {
        Section(String(localized: "Automation")) {
            Toggle(String(localized: "Clean automatically"), isOn: $autoClean)
            Picker(String(localized: "How often"), selection: $period) {
                Text("Weekly").tag("weekly")
                Text("Monthly").tag("monthly")
            }
            .disabled(!autoClean)
            Text("Runs System Junk while OpenCleaner or its menu bar item is open, removing only the items selected by default. The Trash is never emptied.")
                .font(.system(size: 12)).foregroundStyle(.dim)
            Toggle(String(localized: "Show in menu bar"), isOn: $menuBar)
            Toggle(String(localized: "Warn when the startup disk is almost full"), isOn: $lowDisk)
            Picker(String(localized: "Below"), selection: $lowDiskGB) {
                ForEach([5, 10, 20, 50], id: \.self) { Text("\($0) GB").tag($0) }
            }
            .disabled(!lowDisk)
            Toggle(String(localized: "Watch the Trash and Downloads"), isOn: $watch)
            Picker(String(localized: "Larger than"), selection: $watchGB) {
                ForEach([1, 2, 5, 10, 20], id: \.self) { Text("\($0) GB").tag($0) }
            }
            .disabled(!watch)
        }
    }
}
