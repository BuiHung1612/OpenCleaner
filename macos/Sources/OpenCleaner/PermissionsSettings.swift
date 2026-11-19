import SwiftUI
import UserNotifications

/// Settings › Permissions: what OpenCleaner may need from macOS, whether it has it, and a
/// button straight to the right pane of System Settings.
struct PermissionsSettings: View {
    @State private var fullDisk: Bool?
    @State private var notifications: Bool?

    var body: some View {
        Form {
            Section {
                row(icon: "externaldrive.badge.checkmark",
                    title: String(localized: "Full Disk Access"),
                    why: String(localized: "Measure and empty the Trash, clean Mail attachments and read which apps have privacy permissions."),
                    granted: fullDisk,
                    pane: "com.apple.preference.security?Privacy_AllFiles")
                row(icon: "app.badge.checkmark",
                    title: String(localized: "App Management"),
                    why: String(localized: "Uninstall apps and slim them. macOS asks the first time; allow it here if you declined."),
                    granted: nil,
                    pane: "com.apple.preference.security?Privacy_AppBundles")
                row(icon: "bell.badge",
                    title: String(localized: "Notifications"),
                    why: String(localized: "Report scheduled cleaning and warn when the disk is almost full."),
                    granted: notifications,
                    pane: "com.apple.Notifications-Settings.extension?id=\(Bundle.main.bundleIdentifier ?? "")")
            } footer: {
                Note("Not in the list? Click + under the list and choose OpenCleaner in Applications. After you change a permission, come back here: the status updates. Some readings (like the Trash size) need the app to be reopened.")
            }
        }
        .formStyle(.grouped)
        .fixedSize(horizontal: false, vertical: true)
        .scrollIndicators(.hidden)
        .task { await refresh() }
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in
            Task { await refresh() }
        }
    }

    private func row(icon: String, title: String, why: String, granted: Bool?, pane: String) -> some View {
        HStack(alignment: .top, spacing: 12) {
            Image(systemName: icon).font(.title2).foregroundStyle(.secondary).frame(width: 28)
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 6) {
                    Text(verbatim: title).fontWeight(.medium)
                    status(granted)
                }
                Text(verbatim: why).font(.callout).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            }
            Spacer()
            Button(String(localized: "Open Settings")) {
                if let url = URL(string: "x-apple.systempreferences:\(pane)") { NSWorkspace.shared.open(url) }
            }
        }
        .padding(.vertical, 4)
        .accessibilityElement(children: .combine)
    }

    @ViewBuilder private func status(_ granted: Bool?) -> some View {
        switch granted {
        case true?: Label(String(localized: "Allowed"), systemImage: "checkmark.circle.fill").foregroundStyle(.green)
        case false?: Label(String(localized: "Not allowed"), systemImage: "exclamationmark.circle.fill").foregroundStyle(.orange)
        case nil: Label(String(localized: "Asked when needed"), systemImage: "questionmark.circle").foregroundStyle(.secondary)
        }
    }

    private func refresh() async {
        fullDisk = Self.hasFullDiskAccess()
        let settings = await UNUserNotificationCenter.current().notificationSettings()
        notifications = switch settings.authorizationStatus {
        case .authorized, .provisional, .ephemeral: true
        case .denied: false
        default: nil  // not asked yet: asked the first time something is reported
        }
    }

    /// The TCC database opens only with Full Disk Access (reading it is all this does).
    static func hasFullDiskAccess() -> Bool {
        let tcc = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Library/Application Support/com.apple.TCC/TCC.db")
        guard let handle = try? FileHandle(forReadingFrom: tcc) else { return false }
        try? handle.close()
        return true
    }
}
