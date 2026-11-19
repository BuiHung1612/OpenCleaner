//! Layout: sidebar of modules grouped by purpose, one full-bleed page per module. Every module owns
//! a two-color gradient that fills the whole window; content floats on it as Liquid Glass.
//! Decorative art is optional: drop `Resources/Art/<id>.heic` (or `.png`) in and it replaces the SF Symbol.

import AppKit
import SwiftUI

@main
struct OpenCleanerApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate
    init() {
        // needed when launched as a bare executable (`swift run`); a no-op inside the .app
        NSApplication.shared.setActivationPolicy(.regular)
        NSApplication.shared.activate()
        Engine.setFast(UserDefaults.standard.bool(forKey: Engine.fastKey))
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
        case .tool(let id): Tools.all.first { $0.id == id }?.title ?? id
        }
    }

    var icon: String {
        switch self {
        case .dashboard: "gauge.with.dots.needle.50percent"
        case .spaceLens: "circle.hexagongrid"
        case .uninstaller: "xmark.app"
        case .history: "clock.arrow.circlepath"
        case .tool(let id): Tools.all.first { $0.id == id }?.icon ?? "terminal"
        }
    }

    var group: String {
        switch self {
        case .dashboard, .history: ""
        case .spaceLens: "Files"
        case .uninstaller: "Applications"
        case .tool(let id): Tools.all.first { $0.id == id }?.group ?? "Tools"
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
        // start colors kept dark enough for white text (WCAG AA) at the light end of the gradient
        case .spaceLens: [Color(hex: "#0C7889"), Color(hex: "#123063")]
        case .uninstaller: [Color(hex: "#2961BA"), Color(hex: "#2A1857")]
        case .history: [Color(hex: "#4D5B73"), Color(hex: "#1C2233")]
        case .tool(let id): (Tools.all.first { $0.id == id }?.colors ?? []).map(Color.init(hex:)).paddedPalette
        }
    }

    /// Sidebar: dashboard, then groups in a fixed order (unknown manifest groups go last), history.
    static var sections: [(group: String, modules: [Module])] {
        let modules: [Module] = [.spaceLens, .uninstaller] + Tools.all.map { .tool($0.id) }
        var order = ["Cleanup", "Speed", "Applications", "Files", "Security"]
        for m in modules where !order.contains(m.group) { order.append(m.group) }
        return [("", [.dashboard])]
            + order.compactMap { g in
                let items = modules.filter { $0.group == g }
                return items.isEmpty ? nil : (g, items)
            }
            + [(" ", [.history])]
    }

    /// Sidebar order, for ⌘1…⌘9.
    static var ordered: [Module] { sections.flatMap(\.modules) }
}

struct MainView: View {
    @State private var module = Module.dashboard
    @State private var version: String?
    // @Observable models: a page redraws only for what it reads, never the whole window
    @State private var lens = SpaceLensModel()
    @State private var status = StatusStream()
    @State private var junk = JunkModel()
    @State private var apps = AppsModel()
    @State private var installers = PicksModel()
    @State private var projects = PicksModel()
    @State private var slim = PicksModel()
    @State private var largeOld = PicksModel()
    @State private var duplicates = PicksModel()
    @AppStorage("largeMiB") private var largeMiB = 500
    @AppStorage("idleDays") private var idleDays = 180
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        HStack(spacing: 0) {
            sidebar
                .frame(width: 236 * max(1, TextScale.shared.value))  // room for larger text
                .padding(.leading, 14).padding(.top, 44).padding(.bottom, 14)
            page
                .id(module)
                .transition(.opacity)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .frame(minWidth: 980, minHeight: 640)
        .font(TextScale.shared.value == 1 ? nil : .app(13))  // text without its own size; nil keeps Default untouched
        .background(Backdrop(colors: module.palette))
        // light glass reads like frosted glass on the gradient; text is forced white on top
        .preferredColorScheme(.light)
        .foregroundStyle(.white)
        .tint(.white)
        .animation(reduceMotion ? nil : .easeInOut(duration: 0.35), value: module)
        .task { version = await Engine.ask(["op": "version"]).version.string }
        .task { Updater.startAutomaticChecks(); CrashReports.offer() }
        .onReceive(Automation.shared.$openModule.compactMap { $0 }) { module = $0; Automation.shared.openModule = nil }
    }

    /// A floating glass panel, not a system sidebar, so the module gradient shows through it.
    private var sidebar: some View {
        VStack(alignment: .leading, spacing: 0) {
            ScrollView {
                VStack(alignment: .leading, spacing: 2) {
                    ForEach(Module.sections, id: \.group) { section in
                        let header = section.group.trimmingCharacters(in: .whitespaces)
                        if !header.isEmpty {
                            Text(tr(header)).font(.app(12)).foregroundStyle(.dim)
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
                Text("Engine \(version ?? "—")").font(.app(12)).foregroundStyle(.dim)
                Spacer()
                SettingsLink { Label(String(localized: "Settings"), systemImage: "gearshape").labelStyle(.iconOnly) }
                    .buttonStyle(.borderless)
                    .help(String(localized: "Settings"))
            }
            .padding(.horizontal, 20).padding(.vertical, 12)
        }
        .glassEffect(.clear.tint(.white.opacity(0.06)), in: .rect(cornerRadius: 22))
    }

    private func sidebarRow(_ m: Module) -> some View {
        let number = (Module.ordered.firstIndex(of: m) ?? 99) + 1
        return Button { module = m } label: {
            HStack(spacing: 10) {
                Image(systemName: m.icon).frame(width: 20)
                Text(m.title).lineLimit(1).layoutPriority(1)
                Spacer(minLength: 4)
                if m == .spaceLens { UsedBadge(lens: lens) }
            }
            .font(.app(14))
            .padding(.horizontal, 12).padding(.vertical, 8)
            .background(RoundedRectangle(cornerRadius: 10).fill(.white.opacity(module == m ? 0.18 : 0)))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .keyboardShortcut(number <= 9 ? KeyboardShortcut(KeyEquivalent(Character("\(number)")), modifiers: .command) : nil)
        .accessibilityLabel(m.title)
        .accessibilityAddTraits(module == m ? .isSelected : [])
    }

    /// Its own view, so a Space Lens rescan redraws this label and not the window.
    private struct UsedBadge: View {
        let lens: SpaceLensModel
        var body: some View {
            if let v = lens.volumes.first(where: \.isStartup) {
                Text(verbatim: shortBytes(v.used)).font(.app(12)).foregroundStyle(.dim).fixedSize()
                    .help(String(localized: "Used on the startup disk"))
            }
        }
    }

    @ViewBuilder private var page: some View {
        switch module {
        case .dashboard: DashboardPage(stream: status)
        case .spaceLens: SpaceLensPage(model: lens)
        case .uninstaller: UninstallerPage(model: apps)
        case .history: HistoryPage()
        case .tool(let id):
            if let tool = Tools.all.first(where: { $0.id == id }) {
                switch id {
                case "clean": JunkPage(tool: tool, model: junk)
                case "installer": PicksPage(tool: tool, request: ["op": "installers"], kind: .trash, model: installers)
                case "purge": PicksPage(tool: tool, request: ["op": "projects"], kind: .delete, model: projects)
                case "slim": PicksPage(tool: tool, request: ["op": "slim"], kind: .slim, model: slim)
                case "large_old": PicksPage(tool: tool, request: ["op": "large_old", "min_mib": largeMiB, "days": idleDays], kind: .trash, model: largeOld)
                case "duplicates": PicksPage(tool: tool, request: ["op": "duplicates"], kind: .trash, model: duplicates)
                case "optimize": OptimizePage(tool: tool)
                case "touchid": TouchIDPage(tool: tool)
                case "startup": StartupPage(tool: tool)
                default: Intro(module: .tool(tool.id), subtitle: tool.subtitle, points: tool.points)
                }
            }
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

/// Settings › General › Text size. macOS has no Dynamic Type for apps, so every font goes
/// through `Font.app`, which reads this; a body that reads it redraws when it changes.
@MainActor @Observable final class TextScale {
    static let shared = TextScale()
    static let key = "textScale"
    static let choices: [(scale: Double, name: LocalizedStringResource)] = [
        (0.9, "Small"), (1, "Default"), (1.15, "Large"), (1.3, "Extra Large"),
    ]

    var value = UserDefaults.standard.object(forKey: key) as? Double ?? 1 {
        didSet { UserDefaults.standard.set(value, forKey: Self.key) }
    }
}

extension Font {
    /// The system font at `size` points (as designed at Default) times the chosen text size.
    @MainActor static func app(_ size: CGFloat, weight: Font.Weight = .regular) -> Font {
        .system(size: size * TextScale.shared.value, weight: weight)
    }
}

extension ShapeStyle where Self == Color {
    /// Secondary text on the gradient. `.secondary` is 50% white, too faint on the lighter glass.
    /// Only on the gradient: on a light system surface (Settings) use `.secondary`.
    static var dim: Color { .white.opacity(0.88) }
    /// Needs attention (in use, could not be removed…).
    static var warn: Color { Color(hex: "#FFD54F") }
    /// Went well.
    static var ok: Color { Color(hex: "#7CF2C8") }
}

extension View {
    /// Scroll areas and spinners sit on colored glass: ask for the light-on-dark style, not the default black one.
    /// Only for scroll views whose content has no glass of its own (it would turn dark too).
    func brightScroller() -> some View {
        environment(\.colorScheme, .dark)
    }

    /// The one glass surface used for every panel.
    func glassPanel(radius: CGFloat = 20) -> some View {
        modifier(GlassPanel(radius: radius))
    }

    /// A row ticked by clicking anywhere on it: one VoiceOver element, announced as a button
    /// that is selected or not.
    func checkRow(_ on: Bool, toggle: @escaping () -> Void) -> some View {
        contentShape(Rectangle())
            .onTapGesture(perform: toggle)
            .accessibilityElement(children: .combine)
            .accessibilityAddTraits(on ? [.isButton, .isSelected] : .isButton)
            .accessibilityValue(on ? String(localized: "Selected") : String(localized: "Not selected"))
            .accessibilityAction(.default, toggle)
    }
}

/// Clear glass, or a solid dark panel when Reduce Transparency is on (white text stays readable).
private struct GlassPanel: ViewModifier {
    let radius: CGFloat
    @Environment(\.accessibilityReduceTransparency) private var solid

    func body(content: Content) -> some View {
        if solid {
            content.background(.black.opacity(0.4), in: .rect(cornerRadius: radius))
        } else {
            content.glassEffect(.clear.tint(.white.opacity(0.07)), in: .rect(cornerRadius: radius))
        }
    }
}

/// Optional artwork from `Resources/Art/<id>.heic` or `.png`; falls back to a symbol on a glass disc.
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
        let urls: [URL?] = ["heic", "png"].flatMap { (ext: String) -> [URL?] in
            [Bundle.main.url(forResource: id, withExtension: ext, subdirectory: "Art"), repoArt.appendingPathComponent("\(id).\(ext)")]
        }
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
/// Return presses it (it is the page's default action).
struct BigButton: View {
    let title: String
    var busy = false
    var action: () -> Void

    var body: some View {
        Button(action: action) {
            ZStack {
                if busy {
                    ProgressView().controlSize(.large).brightScroller()
                } else {
                    Text(title).font(.app(17, weight: .semibold))
                        .multilineTextAlignment(.center).minimumScaleFactor(0.7).padding(12)
                }
            }
            .frame(width: 104, height: 104)
            .contentShape(.circle)
        }
        .buttonStyle(.plain)
        .glassEffect(.clear.tint(.white.opacity(0.18)).interactive(), in: .circle)
        .overlay(Circle().strokeBorder(.white.opacity(0.45), lineWidth: 2).padding(-6))
        .disabled(busy)
        .keyboardShortcut(busy ? nil : .defaultAction)
        .accessibilityLabel(busy ? String(localized: "Working…") : title)
    }
}

/// Secondary action: same glass and white label everywhere, so it never fights the big button.
struct PillStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.app(14, weight: .semibold))
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
/// Same height and bottom margin on every page, so the big button never moves.
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
        .padding(.bottom, Layout.bottom)
    }
}

/// Page margins shared by every module page.
enum Layout {
    static let side: CGFloat = 32
    static let top: CGFloat = 36
    static let bottom: CGFloat = 20
}

/// The header of a module page once it shows results: art, title, one status line, and an
/// optional accessory on the right (a search field, a button).
struct PageHeader<Accessory: View>: View {
    let art: String
    let symbol: String
    let title: String
    var status = ""
    @ViewBuilder var accessory: Accessory

    var body: some View {
        HStack(spacing: 16) {
            Art(id: art, symbol: symbol, size: 72)
            VStack(alignment: .leading, spacing: 4) {
                Text(verbatim: title).font(.app(28, weight: .bold)).lineLimit(1)
                if !status.isEmpty {
                    Text(verbatim: status).font(.app(15)).foregroundStyle(.dim).lineLimit(2)
                }
            }
            .layoutPriority(1)
            Spacer(minLength: 12)
            accessory
        }
        .padding(.top, Layout.top)
    }
}

extension PageHeader where Accessory == EmptyView {
    init(art: String, symbol: String, title: String, status: String = "") {
        self.init(art: art, symbol: symbol, title: title, status: status) { EmptyView() }
    }
}

/// Shown in place of an empty list.
struct EmptyState: View {
    let symbol: String
    let title: String
    var message = ""

    var body: some View {
        VStack(spacing: 8) {
            Art(id: "empty", symbol: symbol, size: 110).accessibilityHidden(true)
            Text(verbatim: title).font(.app(15, weight: .semibold))
            if !message.isEmpty {
                Text(verbatim: message).font(.app(13)).foregroundStyle(.dim).multilineTextAlignment(.center)
            }
        }
        .frame(maxWidth: .infinity)
        .padding(32)
        .accessibilityElement(children: .combine)
    }
}

/// A page waiting on a search: the scanning art, a spinner and what is going on.
struct LoadingState: View {
    let text: String
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        VStack(spacing: 12) {
            if reduceMotion {
                Art(id: "scanning", symbol: "magnifyingglass", size: 180)
            } else {
                Art(id: "scanning", symbol: "magnifyingglass", size: 180)
                    .phaseAnimator([false, true]) { art, up in
                        art.scaleEffect(up ? 1.04 : 0.96).opacity(up ? 1 : 0.8)
                    } animation: { _ in .easeInOut(duration: 1.2) }
            }
            ProgressView().controlSize(.regular).brightScroller()
            Text(verbatim: text).font(.app(14)).foregroundStyle(.dim)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .accessibilityElement(children: .combine)
    }
}

/// A failed search or scan, with the way to run it again.
struct ErrorBanner: View {
    let message: String
    var retry: (() -> Void)?

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(Color.warn)
            Text(verbatim: message).font(.app(13)).fixedSize(horizontal: false, vertical: true)
            Spacer()
            if let retry { Button(String(localized: "Try Again"), action: retry).buttonStyle(.pill) }
        }
        .padding(.horizontal, 16).padding(.vertical, 10)
        .glassPanel(radius: 16)
    }
}

/// After a move to the Trash: it can be undone there.
struct TrashNote: View {
    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: "arrow.uturn.backward.circle")
            Text("You can put them back from the Trash.").font(.app(13))
            Spacer()
            Button(String(localized: "Open Trash"), action: openTrash).buttonStyle(.pill)
        }
        .foregroundStyle(.dim)
    }
}

func openTrash() {
    NSWorkspace.shared.open(FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".Trash"))
}

/// Large centered intro used by module pages before they have results.
struct Intro: View {
    let module: Module
    let subtitle: String
    var points: [String] = []

    var body: some View {
        HStack(spacing: 48) {
            VStack(alignment: .leading, spacing: 14) {
                Text(module.title).font(.app(34, weight: .bold))
                Text(subtitle).font(.app(15)).foregroundStyle(.dim).fixedSize(horizontal: false, vertical: true)
                ForEach(points, id: \.self) { p in
                    Label(p, systemImage: "checkmark.circle.fill").font(.app(14))
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

/// No decimals ("443 GB"): for tight spots like the sidebar.
func shortBytes(_ n: Double) -> String {
    let f = ByteCountFormatter()
    f.isAdaptive = false
    return f.string(fromByteCount: Int64(n))
}

func percent(_ n: Double?) -> String {
    n.map { String(format: "%.0f%%", $0) } ?? "—"
}

func reveal(_ path: String?) {
    guard let path else { return }
    NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: path)])
}

/// Shown while a page displays an older result and a fresh run is under way, so old numbers
/// never pass for new ones. With `error`, the refresh failed and the old result stays.
struct RefreshBanner: View {
    let date: Date
    var error: String?

    var body: some View {
        HStack(spacing: 8) {
            if let error {
                Image(systemName: "exclamationmark.triangle")
                Text(verbatim: String(localized: "Could not update, showing the result from \(Self.age(date)): \(error)"))
            } else {
                ProgressView().controlSize(.small).brightScroller()
                Text(verbatim: String(localized: "Showing the result from \(Self.age(date)). Updating…"))
            }
        }
        .font(.app(12))
        .foregroundStyle(.dim)
        .lineLimit(1)
        .padding(.horizontal, 14).padding(.vertical, 6)
        .glassEffect(.clear.tint(.white.opacity(0.07)), in: .capsule)
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    static func age(_ date: Date) -> String {
        date.formatted(.relative(presentation: .named))
    }
}
