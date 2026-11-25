//! Layout: sidebar of modules grouped by purpose, one full-bleed page per module. Every module owns
//! a two-color gradient that fills the whole window; content floats on it as Liquid Glass.
//! Decorative art is optional: drop `Resources/Art/<id>.heic` (or `.png`) in and it replaces the SF Symbol.

import AppKit
import SwiftUI

@main
struct OpenCleanerApp: App {
    @AppStorage(Automation.menuBarKey) private var menuBar = true
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate
    init() {
        // needed when launched as a bare executable (`swift run`); a no-op inside the .app
        NSApplication.shared.setActivationPolicy(.regular)
        NSApplication.shared.activate()
        Engine.setFast(UserDefaults.standard.bool(forKey: Engine.fastKey))
        // people who used an earlier version (it saved a window frame) skip the welcome
        let d = UserDefaults.standard
        if d.object(forKey: Welcome.key) == nil,
           ["windowFitsScreen", "windowFitsScreen2", "NSWindow Frame main"].contains(where: { d.object(forKey: $0) != nil }) {
            d.set(true, forKey: Welcome.key)
        }
    }

    /// Tall enough for the whole sidebar where the screen allows (880), no wider than the
    /// pages use (1280), centered; at least 1080 x 720 where the screen has room.
    static func fitted(_ screen: CGRect) -> CGRect {
        let w = max(min(screen.width * 0.8, 1280), min(1080, screen.width))
        let h = max(min(screen.height * 0.9, 880), min(720, screen.height))
        return CGRect(x: screen.midX - w / 2, y: screen.midY - h / 2, width: w, height: h)
    }

    var body: some Scene {
        Window("OpenCleaner", id: "main") {
            MainView()
                // SwiftUI saves the frame under the window's id but opens it centered on the
                // main screen again: put it back where it was (a screen gone since: left alone)
                .background(WindowProbe { window in
                    // once: forget the size saved by earlier versions (a fixed 1080 x 720, then
                    // 0.3.4's 1600 x 1100)
                    if !UserDefaults.standard.bool(forKey: "windowFitsScreen2") {
                        UserDefaults.standard.set(true, forKey: "windowFitsScreen2")
                        UserDefaults.standard.removeObject(forKey: "NSWindow Frame main")
                        if let screen = window.screen ?? NSScreen.main {
                            window.setFrame(Self.fitted(screen.visibleFrame), display: true)
                        }
                        return
                    }
                    guard let saved = UserDefaults.standard.string(forKey: "NSWindow Frame main") else { return }
                    let f = saved.split(separator: " ").prefix(4).compactMap { Double($0) }
                    guard f.count == 4 else { return }
                    let frame = NSRect(x: f[0], y: f[1], width: f[2], height: f[3])
                    if NSScreen.screens.contains(where: { $0.visibleFrame.intersects(frame) }) {
                        window.setFrame(frame, display: true)
                    }
                })
        }
        .windowStyle(.hiddenTitleBar)
        .defaultWindowPlacement { _, context in
            WindowPlacement(size: Self.fitted(context.defaultDisplay.visibleRect).size)
        }
        .windowResizability(.contentMinSize)
        .commands {
            CommandGroup(after: .appInfo) {
                Button(String(localized: "Check for Updates…")) { Task { await Updater.check(manual: true) } }
            }
            // Settings is a page of the main window, not a window of its own
            CommandGroup(replacing: .appSettings) {
                Button(String(localized: "Settings…")) { Automation.shared.openModule = .settings }
                    .keyboardShortcut(",")
            }
        }

        // actions only: numbers there would go stale between readings
        MenuBarExtra(isInserted: $menuBar) { QuickMenu() } label: { MenuBarIcon() }
    }
}

/// The broom from design/MenuBarIcon.png as a template image (macOS tints it for the bar).
struct MenuBarIcon: View {
    var body: some View {
        if let image = Bundle.main.image(forResource: "MenuBarIcon") {
            Image(nsImage: { image.isTemplate = true; return image }())
                .accessibilityLabel(Text(verbatim: "OpenCleaner"))
        } else {
            Image(systemName: "sparkles").accessibilityLabel(Text(verbatim: "OpenCleaner"))
        }
    }
}

struct QuickMenu: View {
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        Button(String(localized: "Clean System Junk…")) { Task { await Automation.shared.cleanNow() } }
        Button(String(localized: "Open OpenCleaner")) {
            openWindow(id: "main")
            NSApp.activate()
        }
        Divider()
        Button(String(localized: "Quit OpenCleaner")) { NSApp.terminate(nil) }
    }
}

// MARK: navigation

enum Module: Hashable {
    case dashboard, spaceLens, uninstaller, history, settings
    case tool(String)

    var title: String {
        switch self {
        case .dashboard: String(localized: "Dashboard")
        case .spaceLens: String(localized: "Space Lens")
        case .uninstaller: String(localized: "Uninstaller")
        case .history: String(localized: "History")
        case .settings: String(localized: "Settings")
        case .tool(let id): Tools.all.first { $0.id == id }?.title ?? id
        }
    }

    var icon: String {
        switch self {
        case .dashboard: "gauge.with.dots.needle.50percent"
        case .spaceLens: "circle.hexagongrid"
        case .uninstaller: "xmark.app"
        case .history: "clock.arrow.circlepath"
        case .settings: "gearshape"
        case .tool(let id): Tools.all.first { $0.id == id }?.icon ?? "terminal"
        }
    }

    var group: String {
        switch self {
        case .dashboard, .history, .settings: ""
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
        case .settings: "settings"
        case .tool(let id): id
        }
    }

    var palette: [Color] {
        switch self {
        case .dashboard: [Color(hex: "#3A3FB8"), Color(hex: "#1B1446")]
        // start colors kept dark enough for white text (WCAG AA) at the light end of the gradient
        case .spaceLens: [Color(hex: "#0C7889"), Color(hex: "#123063")]
        case .uninstaller: [Color(hex: "#2961BA"), Color(hex: "#2A1857")]
        case .history: [Color(hex: "#8E3FA3"), Color(hex: "#241240")]
        case .settings: [Color(hex: "#13806E"), Color(hex: "#0F2A3D")]
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
            + [(" ", [.history, .settings])]
    }

    /// Sidebar order, for ⌘1…⌘9.
    static var ordered: [Module] { sections.flatMap(\.modules) }
}

/// First launch only: what OpenCleaner may need from macOS, before anything is scanned.
struct Welcome: View {
    static let key = "welcomed"
    @Environment(\.dismiss) private var dismiss
    @AppStorage(Automation.menuBarKey) private var menuBar = true

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack(spacing: 14) {
                Image(nsImage: NSApp.applicationIconImage).resizable().frame(width: 56, height: 56)
                    .accessibilityHidden(true)
                VStack(alignment: .leading, spacing: 4) {
                    Text("Welcome to OpenCleaner").font(.title2.bold())
                    Text("Nothing is removed until you have reviewed it and said so. A few settings help before you start.")
                        .foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                }
            }
            PermissionsSettings()
            Toggle(String(localized: "Show in menu bar"), isOn: $menuBar).padding(.horizontal, 8)
            HStack {
                Spacer()
                Button(String(localized: "Get Started")) { dismiss() }
                    .buttonStyle(.borderedProminent).keyboardShortcut(.defaultAction)
            }
        }
        .padding(24)
        .frame(width: 640)
    }
}

struct MainView: View {
    @AppStorage(Welcome.key) private var welcomed = false
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
    @State private var homeItems = PicksModel()
    @State private var devtools = DevToolsModel()
    @AppStorage("largeMiB") private var largeMiB = 500
    @AppStorage("idleDays") private var idleDays = 180
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        HStack(spacing: 0) {
            sidebar
                .frame(width: 236 * max(1, TextScale.shared.value))  // room for larger text
                .padding(.leading, 14).padding(.top, 12).padding(.bottom, 14)
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
        .sheet(isPresented: Binding(get: { !welcomed }, set: { if !$0 { welcomed = true } })) { Welcome() }
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
            // keep the scroller off the panel's rounded corners and edge
            .contentMargins(.vertical, 14, for: .scrollIndicators)
            .padding(.trailing, 4)
            .brightScroller()
            Text("Engine \(version ?? "—")").font(.app(12)).foregroundStyle(.dim)
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
        case .dashboard: DashboardPage(stream: status, junk: junk, devtools: devtools)
        case .spaceLens: SpaceLensPage(model: lens)
        case .uninstaller: UninstallerPage(model: apps)
        case .history: HistoryPage()
        case .settings: SettingsPage()
        case .tool(let id):
            if let tool = Tools.all.first(where: { $0.id == id }) {
                switch id {
                case "clean": JunkPage(tool: tool, model: junk)
                case "installer": PicksPage(tool: tool, request: ["op": "installers"], kind: .trash, model: installers)
                case "purge": PicksPage(tool: tool, request: ["op": "projects"], kind: .delete, model: projects)
                case "slim": PicksPage(tool: tool, request: ["op": "slim"], kind: .slim, model: slim)
                case "large_old": PicksPage(tool: tool, request: ["op": "large_old", "min_mib": largeMiB, "days": idleDays], kind: .trash, model: largeOld)
                case "duplicates": PicksPage(tool: tool, request: ["op": "duplicates"], kind: .trash, model: duplicates)
                case "home_items": PicksPage(tool: tool, request: ["op": "home_items"], kind: .trash, model: homeItems)
                case "optimize": OptimizePage(tool: tool)
                case "touchid": TouchIDPage(tool: tool)
                case "startup": StartupPage(tool: tool)
                case "hogs": HogsPage(tool: tool)
                case "devtools": DevToolsPage(tool: tool, model: devtools, junk: junk)
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

/// A switch for the gradient: mint when on, faint glass when off, white knob either way.
struct GlassSwitch: ToggleStyle {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.isEnabled) private var isEnabled

    func makeBody(configuration: Configuration) -> some View {
        HStack {
            configuration.label
            Spacer(minLength: 12)
            Capsule()
                .fill(configuration.isOn ? AnyShapeStyle(.ok) : AnyShapeStyle(.white.opacity(0.18)))
                .overlay(Capsule().strokeBorder(.white.opacity(0.25), lineWidth: 0.5))
                .frame(width: 38, height: 22)
                .overlay(alignment: configuration.isOn ? .trailing : .leading) {
                    Circle().fill(.white).shadow(color: .black.opacity(0.3), radius: 1.5, y: 1)
                        .padding(2)
                }
                .opacity(isEnabled ? 1 : 0.4)
                .animation(reduceMotion ? nil : .spring(duration: 0.25), value: configuration.isOn)
                .contentShape(Capsule())
                .onTapGesture { configuration.isOn.toggle() }
        }
        .accessibilityRepresentation { Toggle(isOn: configuration.$isOn) { configuration.label } }
    }
}

extension View {
    /// Scroll areas and spinners sit on colored glass: ask for the light-on-dark style, not the default black one.
    /// Only for scroll views whose content has no glass of its own (it would turn dark too).
    /// Also keeps the scroller off the rounded corners of the panel it sits in.
    func brightScroller() -> some View {
        environment(\.colorScheme, .dark)
            .contentMargins(.vertical, 14, for: .scrollIndicators)
    }

    /// The one glass surface used for every panel.
    func glassPanel(radius: CGFloat = 20) -> some View {
        modifier(GlassPanel(radius: radius))
    }

    /// A row ticked by clicking anywhere on it: one VoiceOver element, announced as a button
    /// that is selected, partly selected (some of what it holds) or not.
    func checkRow(_ on: Bool, partial: Bool = false, toggle: @escaping () -> Void) -> some View {
        contentShape(Rectangle())
            .onTapGesture(perform: toggle)
            .accessibilityElement(children: .combine)
            .accessibilityAddTraits(on ? [.isButton, .isSelected] : .isButton)
            .accessibilityValue(partial ? String(localized: "Partly selected")
                                : on ? String(localized: "Selected") : String(localized: "Not selected"))
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
        // decoration: VoiceOver would read the symbol's name ("Scissors", "Hammer")
        Group {
            if let image = Self.image(id) {
                Image(nsImage: image).resizable().scaledToFit().frame(width: size, height: size)
            } else {
                Image(systemName: symbol)
                    .font(.app(size * 0.34, weight: .light))
                    .symbolRenderingMode(.hierarchical)
                    .frame(width: size * 0.72, height: size * 0.72)
                    .glassEffect(.clear.tint(.white.opacity(0.06)), in: .circle)
                    .frame(width: size, height: size)
            }
        }
        .accessibilityHidden(true)
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
    /// Return presses it. Only for safe steps (Scan, Review): a stray Return (typing in the
    /// search field) must never start a removal, even one that still asks first.
    var returnKey = false
    var action: () -> Void

    var body: some View {
        Button(action: action) {
            ZStack {
                if busy {
                    ProgressView().controlSize(.large).brightScroller()
                } else {
                    Text(title).font(.system(size: Self.fitting(title, size: 17 * TextScale.shared.value, width: 80), weight: .semibold))
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
        .keyboardShortcut(busy || !returnKey ? nil : .defaultAction)
        .accessibilityLabel(busy ? String(localized: "Working…") : title)
    }

    /// `size`, or smaller so the longest word fits `width`: lines break between words only
    /// ("Run Selected" at Extra Large would otherwise split "Selected").
    static func fitting(_ title: String, size: CGFloat, width: CGFloat) -> CGFloat {
        let font = NSFont.systemFont(ofSize: size, weight: .semibold)
        let widest = title.split(separator: " ").map { (String($0) as NSString).size(withAttributes: [.font: font]).width }.max() ?? 0
        return widest > width ? (size * width / widest).rounded(.down) : size
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
    /// false: nothing to offer yet, so the bar gives its height to the list above
    var shown = true
    @ViewBuilder var leading: Leading
    @ViewBuilder var center: Center
    @ViewBuilder var trailing: Trailing

    var body: some View {
        HStack(spacing: 28) {
            HStack(spacing: 12) { leading }.frame(maxWidth: .infinity, alignment: .trailing)
            center
            HStack(spacing: 12) { trailing }.frame(maxWidth: .infinity, alignment: .leading)
        }
        .frame(height: shown ? 124 : 0)
        .opacity(shown ? 1 : 0)
        .padding(.bottom, Layout.bottom)
    }
}

/// Page margins shared by every module page.
enum Layout {
    static let side: CGFloat = 32
    static let top: CGFloat = 12
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

/// File and disk sizes (decimal units, like Finder). Zero reads "0 KB", not "Zero KB".
func bytes(_ n: Double?) -> String {
    guard let n else { return "—" }
    return byteFormatter(.file, zeroKB: n == 0).string(fromByteCount: Int64(n))
}

/// Memory (binary units, like Activity Monitor): 16 GB of RAM reads "16 GB", not "17.18 GB".
func memBytes(_ n: Double?) -> String {
    guard let n else { return "—" }
    return byteFormatter(.memory, zeroKB: n == 0).string(fromByteCount: Int64(n))
}

/// No decimals ("443 GB"): for tight spots like the sidebar.
func shortBytes(_ n: Double) -> String {
    let f = byteFormatter(.file, zeroKB: n == 0)
    f.isAdaptive = false
    return f.string(fromByteCount: Int64(n))
}

private func byteFormatter(_ style: ByteCountFormatter.CountStyle, zeroKB: Bool) -> ByteCountFormatter {
    let f = ByteCountFormatter()
    f.countStyle = style
    if zeroKB {
        f.allowsNonnumericFormatting = false
        f.allowedUnits = .useKB
    }
    return f
}

/// "1.2 GB of 3 GB selected" for a row partly ticked, else its full size.
func sizeText(_ full: Double, selected: Double, partial: Bool) -> String {
    partial ? String(localized: "\(bytes(selected)) of \(bytes(full)) selected") : bytes(full)
}

/// A time span in the app's language, two units at most ("1 day, 8 hr" / "1 ngày, 8 giờ").
func duration(_ seconds: Double) -> String {
    let f = DateComponentsFormatter()
    f.unitsStyle = .short
    f.allowedUnits = seconds >= 86_400 ? [.day, .hour] : [.hour, .minute]
    f.maximumUnitCount = 2
    var calendar = Calendar.current
    calendar.locale = Locale(identifier: Bundle.main.preferredLocalizations.first ?? "en")
    f.calendar = calendar
    return f.string(from: seconds) ?? ""
}

/// Runs `found` with the window this view is in, once it is in one.
struct WindowProbe: NSViewRepresentable {
    let found: (NSWindow) -> Void

    func makeNSView(context: Context) -> NSView { Probe(found: found) }
    func updateNSView(_ nsView: NSView, context: Context) {}

    final class Probe: NSView {
        let found: (NSWindow) -> Void
        init(found: @escaping (NSWindow) -> Void) {
            self.found = found
            super.init(frame: .zero)
        }

        required init?(coder: NSCoder) { nil }

        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            // after SwiftUI has placed the window
            if let window { DispatchQueue.main.async { self.found(window) } }
        }
    }
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
