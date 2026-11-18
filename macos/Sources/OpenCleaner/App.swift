//! Layout: sidebar of modules grouped by purpose, one full-bleed page per module. Every module owns
//! a two-color gradient that fills the whole window; content floats on it as Liquid Glass.
//! Decorative art is optional: drop `Resources/Art/<id>.png` in and it replaces the SF Symbol.

import AppKit
import SwiftUI

@main
struct OpenCleanerApp: App {
    init() {
        // needed when launched as a bare executable (`swift run`); a no-op inside the .app
        NSApplication.shared.setActivationPolicy(.regular)
        NSApplication.shared.activate()
    }

    var body: some Scene {
        Window("OpenCleaner", id: "main") {
            MainView()
        }
        .windowStyle(.hiddenTitleBar)
        .defaultSize(width: 1080, height: 720)
        .windowResizability(.contentMinSize)
        .commands {
            CommandGroup(after: .appInfo) {
                Button(String(localized: "Check for Updates…")) { Task { await Updater.check(manual: true) } }
            }
        }

        Settings {
            SettingsView()
        }
    }
}

// MARK: navigation

enum Module: Hashable {
    case dashboard, spaceLens, uninstaller, history
    case tool(String)

    var title: String {
        switch self {
        case .dashboard: String(localized: "Dashboard")
        case .spaceLens: String(localized: "Space Lens")
        case .uninstaller: String(localized: "Uninstaller")
        case .history: String(localized: "History")
        case .tool(let id): CLI.tools.first { $0.id == id }?.title ?? id
        }
    }

    var icon: String {
        switch self {
        case .dashboard: "square.grid.2x2"
        case .spaceLens: "circle.hexagongrid"
        case .uninstaller: "xmark.app"
        case .history: "clock.arrow.circlepath"
        case .tool(let id): CLI.tools.first { $0.id == id }?.icon ?? "terminal"
        }
    }

    var group: String {
        switch self {
        case .dashboard, .history: ""
        case .spaceLens: "Files"
        case .uninstaller: "Applications"
        case .tool(let id): CLI.tools.first { $0.id == id }?.group ?? "Tools"
        }
    }

    var artID: String {
        switch self {
        case .dashboard: "dashboard"
        case .spaceLens: "spaceLens"
        case .uninstaller: "uninstaller"
        case .history: "history"
        case .tool(let id): id
        }
    }

    var palette: [Color] {
        switch self {
        case .dashboard: [Color(hex: "#3A3FB8"), Color(hex: "#1B1446")]
        case .spaceLens: [Color(hex: "#0E8A9E"), Color(hex: "#123063")]
        case .uninstaller: [Color(hex: "#2F6FD6"), Color(hex: "#2A1857")]
        case .history: [Color(hex: "#4D5B73"), Color(hex: "#1C2233")]
        case .tool(let id): (CLI.tools.first { $0.id == id }?.colors ?? []).map(Color.init(hex:)).paddedPalette
        }
    }

    /// Sidebar: dashboard, then groups in a fixed order (unknown manifest groups go last), history.
    static var sections: [(group: String, modules: [Module])] {
        let modules: [Module] = [.spaceLens, .uninstaller] + CLI.tools.map { .tool($0.id) }
        var order = ["Cleanup", "Speed", "Applications", "Files", "Security"]
        for m in modules where !order.contains(m.group) { order.append(m.group) }
        return [("", [.dashboard])]
            + order.compactMap { g in
                let items = modules.filter { $0.group == g }
                return items.isEmpty ? nil : (g, items)
            }
            + [(" ", [.history])]
    }
}

struct MainView: View {
    @State private var module = Module.dashboard
    @State private var version: String?
    @StateObject private var lens = SpaceLensModel()

    var body: some View {
        HStack(spacing: 0) {
            sidebar
                .frame(width: 236)
                .padding(.leading, 14).padding(.top, 44).padding(.bottom, 14)
            page
                .id(module)
                .transition(.opacity)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .frame(minWidth: 980, minHeight: 640)
        .background(Backdrop(colors: module.palette))
        // light glass reads like frosted glass on the gradient; text is forced white on top
        .preferredColorScheme(.light)
        .foregroundStyle(.white)
        .tint(.white)
        .animation(.easeInOut(duration: 0.35), value: module)
        .task { version = await CLI.version() }
        .task { Updater.startAutomaticChecks() }
    }

    /// A floating glass panel, not a system sidebar, so the module gradient shows through it.
    private var sidebar: some View {
        VStack(alignment: .leading, spacing: 0) {
            ScrollView {
                VStack(alignment: .leading, spacing: 2) {
                    ForEach(Module.sections, id: \.group) { section in
                        let header = section.group.trimmingCharacters(in: .whitespaces)
                        if !header.isEmpty {
                            Text(tr(header)).font(.system(size: 12)).foregroundStyle(.dim)
                                .padding(.horizontal, 12).padding(.top, 12).padding(.bottom, 2)
                        } else if section.group == " " {
                            Spacer().frame(height: 12)
                        }
                        ForEach(section.modules, id: \.self) { m in sidebarRow(m) }
                    }
                }
                .padding(10)
            }
            .brightScroller()
            HStack {
                Text("Engine \(version ?? "—")").font(.system(size: 12)).foregroundStyle(.dim)
                Spacer()
                SettingsLink { Image(systemName: "gearshape") }.buttonStyle(.borderless)
            }
            .padding(.horizontal, 20).padding(.vertical, 12)
        }
        .glassEffect(.clear.tint(.white.opacity(0.06)), in: .rect(cornerRadius: 22))
    }

    private func sidebarRow(_ m: Module) -> some View {
        Button { module = m } label: {
            HStack(spacing: 10) {
                Image(systemName: m.icon).frame(width: 20)
                Text(m.title).lineLimit(1)
                Spacer()
                if m == .spaceLens, let used = lens.usedLabel { used.font(.system(size: 12)).foregroundStyle(.dim) }
            }
            .font(.system(size: 14))
            .padding(.horizontal, 12).padding(.vertical, 8)
            .background(RoundedRectangle(cornerRadius: 10).fill(.white.opacity(module == m ? 0.18 : 0)))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    @ViewBuilder private var page: some View {
        switch module {
        case .dashboard: DashboardPage()
        case .spaceLens: SpaceLensPage(model: lens)
        case .uninstaller: UninstallerPage()
        case .history: HistoryPage()
        case .tool(let id):
            if let tool = CLI.tools.first(where: { $0.id == id }) { ToolPage(tool: tool) }
        }
    }
}

// MARK: theme

/// Full-window gradient with two soft light blobs, so the glass on top has something to refract.
struct Backdrop: View {
    let colors: [Color]

    var body: some View {
        ZStack {
            LinearGradient(colors: colors, startPoint: .topLeading, endPoint: .bottomTrailing)
            RadialGradient(colors: [colors[0].opacity(0.9), .clear], center: .init(x: 0.85, y: 0.1), startRadius: 0, endRadius: 520)
            RadialGradient(colors: [.white.opacity(0.10), .clear], center: .init(x: 0.2, y: 0.95), startRadius: 0, endRadius: 420)
        }
        .ignoresSafeArea()
    }
}

extension Color {
    /// `#RRGGBB`; anything else becomes gray rather than crashing on a bad manifest.
    init(hex: String) {
        let v = UInt32(hex.trimmingCharacters(in: CharacterSet(charactersIn: "#")), radix: 16) ?? 0x555555
        self.init(red: Double(v >> 16 & 0xFF) / 255, green: Double(v >> 8 & 0xFF) / 255, blue: Double(v & 0xFF) / 255)
    }
}

extension Array where Element == Color {
    var paddedPalette: [Color] {
        switch count {
        case 0: [Color(hex: "#4D5B73"), Color(hex: "#1C2233")]
        case 1: [self[0], self[0].opacity(0.6)]
        default: self
        }
    }
}

extension ShapeStyle where Self == Color {
    /// Secondary text. `.secondary` is 50% white, too faint on the lighter glass; this stays readable.
    static var dim: Color { .white.opacity(0.78) }
}

extension View {
    /// Scroll areas and spinners sit on colored glass: ask for the light-on-dark style, not the default black one.
    /// Only for scroll views whose content has no glass of its own (it would turn dark too).
    func brightScroller() -> some View {
        environment(\.colorScheme, .dark)
    }

    /// The one glass surface used for every panel.
    func glassPanel(radius: CGFloat = 20) -> some View {
        glassEffect(.clear.tint(.white.opacity(0.07)), in: .rect(cornerRadius: radius))
    }
}

/// Optional artwork from `Resources/Art/<id>.png`; falls back to a symbol on a glass disc.
struct Art: View {
    let id: String
    let symbol: String
    var size: CGFloat = 260

    var body: some View {
        if let image = Self.image(id) {
            Image(nsImage: image).resizable().scaledToFit().frame(width: size, height: size)
        } else {
            Image(systemName: symbol)
                .font(.system(size: size * 0.34, weight: .light))
                .symbolRenderingMode(.hierarchical)
                .frame(width: size * 0.72, height: size * 0.72)
                .glassEffect(.clear.tint(.white.opacity(0.06)), in: .circle)
                .frame(width: size, height: size)
        }
    }

    /// Loaded once per id: bodies re-run on every live-output tick, and a PNG read each time adds up.
    @MainActor private static var images: [String: NSImage?] = [:]

    @MainActor static func image(_ id: String) -> NSImage? {
        if let hit = images[id] { return hit }
        let repoArt = URL(fileURLWithPath: #filePath).deletingLastPathComponent().appendingPathComponent("../../Resources/Art")
        let urls = [Bundle.main.url(forResource: id, withExtension: "png", subdirectory: "Art"), repoArt.appendingPathComponent("\(id).png")]
        let image = urls.compactMap { $0 }.lazy.compactMap { NSImage(contentsOf: $0) }.first
        images[id] = image
        return image
    }
}

/// Finder icon for a path, cached: rows and bubbles re-render on every hover.
@MainActor private let iconCache: NSCache<NSString, NSImage> = {
    let c = NSCache<NSString, NSImage>()
    c.countLimit = 500
    return c
}()

@MainActor func fileIcon(_ path: String) -> NSImage {
    if let hit = iconCache.object(forKey: path as NSString) { return hit }
    let icon = NSWorkspace.shared.icon(forFile: path)
    iconCache.setObject(icon, forKey: path as NSString)
    return icon
}

/// The big round call-to-action at the bottom of a module page.
struct BigButton: View {
    let title: String
    var busy = false
    var action: () -> Void

    var body: some View {
        Button(action: action) {
            ZStack {
                if busy { ProgressView().controlSize(.large).brightScroller() } else { Text(title).font(.system(size: 17, weight: .semibold)) }
            }
            .frame(width: 104, height: 104)
            .contentShape(.circle)
        }
        .buttonStyle(.plain)
        .glassEffect(.clear.tint(.white.opacity(0.18)).interactive(), in: .circle)
        .overlay(Circle().strokeBorder(.white.opacity(0.45), lineWidth: 2).padding(-6))
        .disabled(busy)
    }
}

/// Secondary action: same glass and white label everywhere, so it never fights the big button.
struct PillStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.system(size: 14, weight: .semibold))
            .foregroundStyle(.white)
            .padding(.horizontal, 20)
            .frame(minWidth: 40, minHeight: 40)
            .glassEffect(.clear.tint(.white.opacity(configuration.isPressed ? 0.3 : 0.16)).interactive(), in: .capsule)
    }
}

extension ButtonStyle where Self == PillStyle {
    static var pill: PillStyle { PillStyle() }
}

/// Bottom bar: the big round button stays dead center, secondary actions sit on either side.
struct ActionBar<Leading: View, Center: View, Trailing: View>: View {
    @ViewBuilder var leading: Leading
    @ViewBuilder var center: Center
    @ViewBuilder var trailing: Trailing

    var body: some View {
        HStack(spacing: 28) {
            HStack(spacing: 12) { leading }.frame(maxWidth: .infinity, alignment: .trailing)
            center
            HStack(spacing: 12) { trailing }.frame(maxWidth: .infinity, alignment: .leading)
        }
        .frame(height: 124)
    }
}

/// Large centered intro used by module pages before they have results.
struct Intro: View {
    let module: Module
    let subtitle: String
    var points: [String] = []

    var body: some View {
        HStack(spacing: 48) {
            VStack(alignment: .leading, spacing: 14) {
                Text(module.title).font(.system(size: 34, weight: .bold))
                Text(subtitle).font(.system(size: 15)).foregroundStyle(.dim).fixedSize(horizontal: false, vertical: true)
                ForEach(points, id: \.self) { p in
                    Label(p, systemImage: "checkmark.circle.fill").font(.system(size: 14))
                }
            }
            .frame(maxWidth: 360, alignment: .leading)
            Art(id: module.artID, symbol: module.icon)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

func bytes(_ n: Double?) -> String {
    guard let n else { return "—" }
    return ByteCountFormatter.string(fromByteCount: Int64(n), countStyle: .file)
}

func percent(_ n: Double?) -> String {
    n.map { String(format: "%.0f%%", $0) } ?? "—"
}

/// mo prints sizes like "512.4MB"; good enough for sorting and totals.
func parseSize(_ s: String?) -> Double {
    guard let s else { return 0 }
    let units: [(String, Double)] = [("TB", 1e12), ("GB", 1e9), ("MB", 1e6), ("KB", 1e3), ("B", 1)]
    for (u, m) in units where s.hasSuffix(u) {
        return (Double(s.dropLast(u.count).trimmingCharacters(in: .whitespaces)) ?? 0) * m
    }
    return Double(s) ?? 0
}

func reveal(_ path: String?) {
    guard let path else { return }
    NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: path)])
}
