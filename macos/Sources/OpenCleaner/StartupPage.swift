import AppKit
import SwiftUI

/// Startup Items: what starts at login (the user's own can be turned off; the system's need
/// administrator rights and are only shown), and which apps hold privacy permissions.
struct StartupPage: View {
    let tool: Tool
    @State private var items: [JSON] = []
    @State private var permissions = JSON(nil)
    @State private var asking: JSON?
    @State private var message: String?

    private static let services: [(id: String, title: LocalizedStringKey)] = [
        ("camera", "Camera"), ("microphone", "Microphone"), ("screen", "Screen Recording"),
        ("files", "Full Disk Access"), ("accessibility", "Accessibility"),
    ]

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack(spacing: 16) {
                Art(id: tool.id, symbol: tool.icon, size: 72)
                VStack(alignment: .leading, spacing: 4) {
                    Text(tool.title).font(.system(size: 24, weight: .bold))
                    Text(tr(tool.subtitle)).foregroundStyle(.dim)
                }
                Spacer()
            }
            .padding(.top, 36)
            if let message { Text(verbatim: message).foregroundStyle(Color(hex: "#FFD54F")) }
            ScrollView {
                VStack(alignment: .leading, spacing: 18) {
                    VStack(alignment: .leading, spacing: 4) {
                        heading("Starts at login")
                        ForEach(Array(items.enumerated()), id: \.offset) { _, item in row(item) }
                        // SMAppService login items have no public API to list them
                        HStack {
                            Text("Login items added in System Settings are not listed here.")
                                .font(.system(size: 12)).foregroundStyle(.dim)
                            Button("Open Login Items") { open("com.apple.LoginItems-Settings.extension") }
                                .buttonStyle(.link)
                        }
                        .padding(.horizontal, 12).padding(.top, 4)
                    }
                    permissionsSection
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(12)
            }
            .brightScroller()
            .glassPanel()
            .padding(.bottom, 28)
        }
        .padding(.horizontal, 32)
        .task { await refresh() }
        .confirmationDialog(
            String(localized: "Turn off \(asking?.name.string ?? "")?"),
            isPresented: Binding(get: { asking != nil }, set: { if !$0 { asking = nil } })
        ) {
            Button("Turn Off", role: .destructive) {
                if let item = asking { Task { await turnOff(item) } }
            }
        } message: {
            Text("It stops now and no longer starts at login. Its file goes to the Trash, so you can put it back.")
        }
    }

    private func heading(_ title: LocalizedStringKey) -> some View {
        Text(title).font(.system(size: 13, weight: .semibold)).foregroundStyle(.dim).padding(.leading, 12)
    }

    private func row(_ item: JSON) -> some View {
        HStack(alignment: .top, spacing: 12) {
            VStack(alignment: .leading, spacing: 2) {
                Text(verbatim: item.name.string ?? "").font(.system(size: 14, weight: .medium))
                Text(verbatim: item.program.string ?? item.label.string ?? "")
                    .font(.system(size: 12)).foregroundStyle(.dim).lineLimit(1).truncationMode(.middle)
            }
            Spacer()
            if item.scope.string != "user" {
                Label("System, needs admin", systemImage: "lock.fill").font(.system(size: 12)).foregroundStyle(.dim)
            } else if !item.enabled.bool {
                Text("Off").font(.system(size: 12)).foregroundStyle(.dim)
            } else {
                Button("Turn Off") { asking = item }.buttonStyle(.pill)
            }
        }
        .padding(.horizontal, 12).padding(.vertical, 8)
    }

    @ViewBuilder private var permissionsSection: some View {
        VStack(alignment: .leading, spacing: 4) {
            heading("Permissions")
            if permissions.needs_full_disk_access.bool {
                HStack {
                    Text("Seeing which apps hold permissions needs Full Disk Access for OpenCleaner.")
                        .font(.system(size: 12)).foregroundStyle(.dim)
                    Button("Open Privacy & Security") { open("com.apple.preference.security?Privacy_AllFiles") }
                        .buttonStyle(.link)
                }
                .padding(.horizontal, 12)
            } else {
                ForEach(Self.services, id: \.id) { service in
                    let apps = permissions.grants.array.filter { $0.service.string == service.id }
                        .compactMap(\.client.string).map(Self.appName).sorted()
                    HStack(alignment: .top, spacing: 12) {
                        Text(service.title).font(.system(size: 14, weight: .medium)).frame(width: 150, alignment: .leading)
                        Text(verbatim: apps.isEmpty ? "–" : apps.joined(separator: ", "))
                            .font(.system(size: 12)).foregroundStyle(.dim).fixedSize(horizontal: false, vertical: true)
                    }
                    .padding(.horizontal, 12).padding(.vertical, 6)
                }
            }
        }
    }

    /// A bundle id or program path, as people know it.
    private static func appName(_ client: String) -> String {
        if client.hasPrefix("/") { return URL(fileURLWithPath: client).lastPathComponent }
        guard let url = NSWorkspace.shared.urlForApplication(withBundleIdentifier: client) else { return client }
        return FileManager.default.displayName(atPath: url.path).replacingOccurrences(of: ".app", with: "")
    }

    private func open(_ pane: String) {
        if let url = URL(string: "x-apple.systempreferences:\(pane)") { NSWorkspace.shared.open(url) }
    }

    private func refresh() async {
        let (list, perms) = await Task.detached {
            (Engine.call(["op": "startup_items"]).items.array, Engine.call(["op": "permissions"]))
        }.value
        items = list
        permissions = perms
    }

    private func turnOff(_ item: JSON) async {
        let request: [String: Any] = ["op": "disable_startup", "path": item.path.string ?? "", "label": item.label.string ?? ""]
        let reply = await Task.detached { Engine.call(request) }.value
        message = reply.ok.bool ? nil : reply.error.string
        await refresh()
    }
}
