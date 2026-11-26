import AppKit
import Darwin
import DiskArbitration
import SwiftUI

struct Volume: Identifiable {
    let url: URL
    let name: String
    let total: Double
    let free: Double
    var id: String { url.path }
    var isStartup: Bool { url.path == "/" }
    var used: Double { total - free }

    /// The innermost volume `path` is on ("/Volumes/X" before "/").
    static func holding(_ path: String, in volumes: [Volume]) -> Volume? {
        volumes.filter { v in
            let root = v.url.path
            return path == root || path.hasPrefix(root.hasSuffix("/") ? root : root + "/")
        }
        .max { $0.url.path.count < $1.url.path.count }
    }

    static func all() -> [Volume] {
        let keys: [URLResourceKey] = [.volumeNameKey]
        let urls = FileManager.default.mountedVolumeURLs(includingResourceValuesForKeys: keys, options: [.skipHiddenVolumes]) ?? []
        let session = DASessionCreate(kCFAllocatorDefault)
        return urls.compactMap { url in
            // Match the Dashboard's immediately available bytes, without purgeable space.
            let path = url.path == "/" ? "/System/Volumes/Data" : url.path
            var stat = statfs()
            guard !isDiskImage(url, session), statfs(path, &stat) == 0, stat.f_blocks > 0 else { return nil }
            let name = try? url.resourceValues(forKeys: Set(keys)).volumeName
            return Volume(url: url, name: name ?? url.lastPathComponent,
                          total: Double(stat.f_blocks) * Double(stat.f_bsize), free: Double(stat.f_bavail) * Double(stat.f_bsize))
        }
    }
}

/// A mounted .dmg (an installer, say): a file on another disk, not a disk to clean.
private func isDiskImage(_ url: URL, _ session: DASession?) -> Bool {
    guard let session, let disk = DADiskCreateFromVolumePath(kCFAllocatorDefault, session, url as CFURL),
          let info = DADiskCopyDescription(disk) as? [CFString: Any] else { return false }
    return info[kDADiskDescriptionDeviceModelKey] as? String == "Disk Image"
}

/// Lives as long as the window, so results and the sidebar badge survive switching modules.
@MainActor @Observable
final class SpaceLensModel {
    var volumes: [Volume] = []
    var volume: Volume?
    /// folders opened so far; `nil` is the startup disk overview
    var stack: [String?] = []
    var selection: Set<String> = []
    var error: String?
    /// After a move to the Trash: how many items went.
    var trashed = 0
    var trashError: String?
    private(set) var removing = false
    private(set) var trashProcessed = 0
    private(set) var trashTotal = 0
    private(set) var isLoading = false
    @ObservationIgnored private var active = true
    @ObservationIgnored private var job: Task<Void, Never>?
    @ObservationIgnored private var generation = UUID()
    func refreshVolumes() async {
        let fresh = await Task.detached(priority: .userInitiated) { Volume.all() }.value
        volumes = fresh
        if let id = volume?.id { volume = fresh.first { $0.id == id } ?? volume }
    }
    private var cache: [String: JSON] = [:]
    private var scannedAt: [String: Date] = [:]
    /// Older results shown while a new scan verifies their sizes.
    private var stale: [String: Date] = [:]
    /// keys whose `cache` entry is a scan still in progress (sizes are still growing)
    private var live: Set<String> = []

    var current: JSON? { stack.last.map { cache[$0 ?? ""] } ?? nil }
    /// When the shown results were really taken, if they are an older scan being refreshed.
    var staleDate: Date? { stack.last.map { stale[$0 ?? ""] } ?? nil }
    var isLive: Bool { stack.last.map { live.contains($0 ?? "") } ?? false }
    var entries: [JSON] { current?.entries.array ?? [] }
    var selectedSize: Double {
        entries.filter { selection.contains($0.path.string ?? "") }.compactMap(\.size.double).reduce(0, +)
    }

    func scan(_ v: Volume) {
        guard !removing else { return }
        volume = v
        stack = []
        open(v.isStartup ? nil : v.url.path)
    }

    /// A folder dropped on the page, shown on the volume it lives on (Back leaves it).
    func openDropped(_ path: String) {
        guard !removing else { return }
        volume = Volume.holding(path, in: volumes) ?? volumes.first(where: \.isStartup)
        stack = []
        open(path)
    }

    func open(_ path: String?) {
        guard !removing else { return }
        stack.append(path)
        selection = []
        trashed = 0
        trashError = nil
        load(path)
    }

    func back() {
        guard !removing, !stack.isEmpty else { return }
        cancelScan()
        stack.removeLast()
        selection = []
        trashed = 0
        trashError = nil
        if stack.isEmpty {
            volume = nil
            Task { await refreshVolumes() }
        } else { load(stack.last ?? nil) }
    }

    func startOver() {
        guard !removing else { return }
        cancelScan()
        stack = []
        volume = nil
        selection = []
        Task { await refreshVolumes() }
    }

    private func cancelScan() {
        job?.cancel()
        job = nil
        generation = UUID()
        for key in live { cache[key] = nil }
        live = []
        isLoading = false
    }

    func pause() {
        active = false
        cancelScan()
    }

    func resume() async {
        guard !Task.isCancelled else { return }
        active = true
        await refreshIfVisible()
    }

    func refreshIfVisible() async {
        guard active, !removing, !Task.isCancelled else { return }
        await refreshVolumes()
        guard active, !removing, !Task.isCancelled, !stack.isEmpty else { return }
        load(stack.last ?? nil)
    }

    func load(_ path: String?, force: Bool = false) {
        guard active, !removing else { return }
        let key = path ?? ""
        cancelScan()
        let token = generation
        if force { cache[key] = nil; stale[key] = nil }
        error = nil
        stale[key] = cache[key] == nil ? nil : scannedAt[key]
        isLoading = true
        job = Task {
            do {
                // Both previous-session and in-memory results are previews, never proof of freshness.
                if !force, cache[key] == nil {
                    let old = await ResultCache.read(Self.cacheKey(key))
                    guard generation == token, !Task.isCancelled else { return }
                    if let old {
                        cache[key] = old.json
                        scannedAt[key] = old.date
                        stale[key] = old.date
                    }
                }
                var result = JSON(nil)
                let request: [String: Any] = key.isEmpty ? ["op": "space"] : ["op": "space", "path": key]
                for await event in Engine.run(request) {
                    guard generation == token, !Task.isCancelled else { return }
                    switch event.event.string {
                    case "progress" where cache[key] == nil || live.contains(key):
                        cache[key] = event
                        live.insert(key)
                    case "space": result = event
                    case "error": throw EngineFailure(errorDescription: event.error.string)
                    default: break
                    }
                }
                guard generation == token, !Task.isCancelled else { return }
                if let message = result.error.string { throw EngineFailure(errorDescription: message) }
                guard result.raw != nil else { throw EngineFailure(errorDescription: String(localized: "The scan did not finish.")) }
                cache[key] = result
                selection.formIntersection(Set(result.entries.array.filter(\.selectable.bool).compactMap(\.path.string)))
                scannedAt[key] = Date()
                stale[key] = nil
                live.remove(key)
                await ResultCache.write(result, for: Self.cacheKey(key))
                guard generation == token, !Task.isCancelled else { return }
                isLoading = false
            } catch {
                guard generation == token, !Task.isCancelled else { return }
                if live.remove(key) != nil { cache[key] = nil }
                self.error = error.localizedDescription
                isLoading = false
            }
        }
    }

    private static func cacheKey(_ key: String) -> [String] { ["space", key] }

    struct EngineFailure: LocalizedError {
        let errorDescription: String?
    }
    /// Moves one item to the Trash. macOS *copies* what it cannot remove (read-only or
    /// protected volumes) into the Trash and still reports success: that copy is taken
    /// back out and the move counts as failed.
    nonisolated static func moveToTrash(_ path: String) -> Bool {
        // never ask macOS to move what cannot move: it may copy it into the Trash first
        let parent = (path as NSString).deletingLastPathComponent
        let volume = try? URL(fileURLWithPath: parent).resourceValues(forKeys: [.volumeIsReadOnlyKey])
        guard FileManager.default.isWritableFile(atPath: parent), volume?.volumeIsReadOnly != true else { return false }
        var result: NSURL?
        do {
            try FileManager.default.trashItem(at: URL(fileURLWithPath: path), resultingItemURL: &result)
        } catch {
            return false
        }
        guard FileManager.default.fileExists(atPath: path) else { return true }
        if let copy = result?.path,
           copy.split(separator: "/").contains(where: { $0 == ".Trash" || $0 == ".Trashes" }) {
            try? FileManager.default.removeItem(atPath: copy)
        }
        return false
    }

    /// Selected items go to the Trash (recoverable), never deleted outright.
    func removeSelected(moveToTrash: @escaping @Sendable (String) -> Bool = { SpaceLensModel.moveToTrash($0) }) async {
        guard !removing, !isLoading else { return }
        let removable = Set(entries.filter(\.selectable.bool).compactMap(\.path.string))
        let paths = selection.intersection(removable).sorted()
        guard !paths.isEmpty else { return }
        cancelScan()
        removing = true
        var failed: [String] = []
        trashProcessed = 0
        trashTotal = paths.count
        trashed = 0
        trashError = nil
        for path in paths {
            let moved = await withCheckedContinuation { reply in
                DispatchQueue.global(qos: .userInitiated).async {
                    reply.resume(returning: moveToTrash(path))
                }
            }
            if moved { trashed += 1 } else { failed.append((path as NSString).lastPathComponent) }
            trashProcessed += 1
        }
        selection = []
        trashError = failed.isEmpty ? nil : String(localized: "Could not move to Trash: \(failed.joined(separator: ", "))")
        cache = [:]
        scannedAt = [:]
        stale = [:]
        live = []
        await refreshVolumes()
        removing = false
        if active { load(stack.last ?? nil, force: true) }
    }
}

/// The row or bubble under the pointer. Its own object, so hovering redraws the rows and
/// bubbles that read it, not the whole page.
@MainActor @Observable
final class Hover {
    var path: String?
}

struct SpaceLensPage: View {
    let model: SpaceLensModel
    @State private var hover = Hover()
    @State private var confirming = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        VStack(spacing: 0) {
            if model.volume == nil {
                chooser
            } else {
                results
            }
        }
        .padding(.horizontal, Layout.side)
        .task { await model.resume() }
        .onDisappear { model.pause() }
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in
            Task { await model.refreshIfVisible() }
        }
        // a folder dragged in from Finder opens here
        .contentShape(Rectangle())
        .dropDestination(for: URL.self) { urls, _ in
            guard !model.removing else { return false }
            var dir: ObjCBool = false
            guard let url = urls.first, FileManager.default.fileExists(atPath: url.path, isDirectory: &dir), dir.boolValue else { return false }
            model.openDropped(url.path)
            return true
        }
        .confirmationDialog(
            String(localized: "Move \(model.selection.count) items (\(bytes(model.selectedSize))) to the Trash?"),
            isPresented: $confirming
        ) {
            Button(String(localized: "Move to Trash"), role: .destructive) { Task { await model.removeSelected() } }
        }
    }

    private var chooser: some View {
        VStack(spacing: 32) {
            Intro(module: .spaceLens, subtitle: String(localized: "See what takes up space on each drive as an interactive map, then send what you don't need to the Trash."))
                .frame(maxHeight: 360)
            GlassEffectContainer(spacing: 16) {
                HStack(spacing: 16) {
                    ForEach(model.volumes) { v in
                        VolumeCard(volume: v) { model.scan(v) }
                    }
                }
            }
            Spacer()
        }
        .padding(.top, 24)
    }

    private var results: some View {
        VStack(spacing: 14) {
            HStack(spacing: 10) {
                Button { model.back() } label: { Label(String(localized: "Back"), systemImage: "chevron.left").labelStyle(.iconOnly) }
                    .buttonStyle(.pill)
                    .help(String(localized: "Back"))
                    .keyboardShortcut("[", modifiers: .command)
                Text(breadcrumb).font(.app(15, weight: .semibold)).lineLimit(1).truncationMode(.head)
                Spacer()
                // `force` drops only this page's copy: the engine scan still takes unchanged
                // folders from its FSEvents-checked directory cache (dircache.rs). A home
                // folder that is slow here is slow there too: its event history since the
                // oldest record (up to a day) can be too long to replay within the replay's
                // 10 s budget, and the cache then falls back to stat-ing every folder.
                Button {
                    Task { await model.refreshVolumes() }
                    model.load(model.stack.last ?? nil, force: true)
                } label: {
                    Label(String(localized: "Scan Again"), systemImage: "arrow.clockwise").labelStyle(.iconOnly)
                }
                .buttonStyle(.pill)
                .help(String(localized: "Scan Again"))
                Button(String(localized: "Start Over")) { model.startOver() }.buttonStyle(.pill)
            }
            .disabled(model.removing)
            .padding(.top, Layout.top)

            if model.removing {
                VStack(alignment: .leading, spacing: 6) {
                    Text("Moving to Trash… \(model.trashProcessed) of \(model.trashTotal)")
                    ProgressView(value: Double(model.trashProcessed), total: Double(max(1, model.trashTotal)))
                }
                .font(.app(13)).padding(12).glassPanel(radius: 12)
            }
            if let failure = model.trashError { ErrorBanner(message: failure) }
            if model.trashed > 0 && !model.removing {
                VStack(alignment: .leading, spacing: 4) {
                    Text("\(model.trashed) items moved to the Trash.").font(.app(13, weight: .medium))
                    TrashNote()
                }
                .padding(.horizontal, 16).padding(.vertical, 10)
                .glassPanel(radius: 16)
            }

            if let date = model.staleDate, model.current != nil {
                RefreshBanner(date: date, error: model.error)
            } else if model.isLive {
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small).brightScroller()
                    Text("Measuring… sizes keep growing until the scan finishes.")
                }
                .font(.app(12)).foregroundStyle(.dim)
                .padding(.horizontal, 14).padding(.vertical, 6)
                .glassEffect(.clear.tint(.white.opacity(0.07)), in: .capsule)
                .frame(maxWidth: .infinity, alignment: .leading)
            }

            if let current = model.current {
                HStack(spacing: 24) {
                    list.frame(width: 420)
                    VStack(spacing: 10) {
                        BubbleChart(entries: model.entries, hover: hover) { e in
                            if e.is_dir.bool, let p = e.path.string { model.open(p) }
                        }
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                        if current.scan_status.string == "partial" {
                            Text("Some folders could not be read fully; sizes are at least this big.")
                                .font(.app(12)).foregroundStyle(.dim)
                                .multilineTextAlignment(.center)
                        }
                    }
                }
                .id(currentKey)
                .transition(.opacity.combined(with: .scale(scale: 0.98)))
            } else if let error = model.error {
                ErrorBanner(message: error) { model.load(model.stack.last ?? nil, force: true) }
                Spacer()
            } else {
                LoadingState(text: String(localized: "Mapping your files. Large drives can take about a minute."))
            }

            bottomBar
        }
        .animation(reduceMotion ? nil : .easeOut(duration: 0.25), value: currentKey)
        .animation(reduceMotion ? nil : .easeOut(duration: 0.25), value: model.current == nil)
    }

    private var currentKey: String { model.stack.last.map { $0 ?? "/" } ?? "" }

    private var breadcrumb: String {
        guard let last = model.stack.last else { return "" }
        // the overview's "Home" row, opened, is titled the same
        if last == NSHomeDirectory() { return String(localized: "Home") }
        return last.map { ($0 as NSString).abbreviatingWithTildeInPath } ?? model.volume?.name ?? "/"
    }

    private var list: some View {
        ScrollView {
            LazyVStack(spacing: 4) {
                // keyed by path: live scan events reorder and resize rows without replacing them
                ForEach(model.entries, id: \.path.string) { e in
                    EntryRow(
                        entry: e,
                        selected: model.selection.contains(e.path.string ?? ""),
                        hover: hover,
                        toggle: {
                            guard !model.removing, e.selectable.bool, let p = e.path.string else { return }
                            if model.selection.contains(p) { model.selection.remove(p) } else { model.selection.insert(p) }
                        },
                        open: { if e.is_dir.bool, let p = e.path.string { model.open(p) } }
                    )
                }
            }
            .padding(8)
        }
        .brightScroller()
        .glassPanel()
        .quickLookList()
    }

    private var bottomBar: some View {
        ZStack(alignment: .leading) {
            if let v = model.volume {
                VStack(alignment: .leading, spacing: 6) {
                    Text(verbatim: "\(v.name): \(bytes(v.used)) / \(bytes(v.total))").font(.app(13, weight: .medium))
                    UsageBar(fraction: v.used / v.total).frame(width: 220)
                }
            }
            ActionBar {
                EmptyView()
            } center: {
                if model.removing { BigButton(title: "", busy: true) {} }
                else if !model.selection.isEmpty {
                    BigButton(title: String(localized: "Move to Trash")) { confirming = true }
                        .disabled(model.isLoading)
                }
            } trailing: {
                if !model.selection.isEmpty {
                    Text(verbatim: bytes(model.selectedSize)).font(.app(17, weight: .semibold)).monospacedDigit()
                }
            }
        }
    }
}

struct VolumeCard: View {
    let volume: Volume
    let scan: () -> Void

    var body: some View {
        VStack(spacing: 12) {
            // custom art when provided, otherwise the drive icon macOS uses
            Image(nsImage: Art.image(volume.isStartup ? "volumeInternal" : "volumeExternal") ?? fileIcon(volume.url.path))
                .resizable().scaledToFit().frame(width: 88, height: 88)
            Text(volume.name).font(.app(15, weight: .semibold)).lineLimit(1)
            UsageBar(fraction: volume.used / volume.total)
            Text("\(bytes(volume.free)) free of \(bytes(volume.total))").font(.app(12)).foregroundStyle(.dim)
            Button(String(localized: "Scan"), action: scan).buttonStyle(.pill)
        }
        .padding(20)
        .frame(width: 210)
        .glassPanel(radius: 24)
    }
}

/// Fills from empty when it appears; later value changes jump without animation.
struct UsageBar: View {
    let fraction: Double
    @State private var shown = 0.0
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        Capsule().fill(.white.opacity(0.15))
            .overlay(alignment: .leading) {
                GeometryReader { g in
                    Capsule()
                        .fill(LinearGradient(colors: [Color(hex: "#FFD54F"), Color(hex: "#FF8A65")], startPoint: .leading, endPoint: .trailing))
                        .frame(width: g.size.width * min(max(shown, 0), 1))
                }
            }
            .frame(height: 6)
            .onAppear { withAnimation(reduceMotion ? nil : .easeOut(duration: 0.9)) { shown = fraction } }
            .accessibilityElement()
            .accessibilityValue(percent(fraction * 100))
            .onChange(of: fraction) { shown = fraction }
    }
}

struct EntryRow: View {
    let entry: JSON
    let selected: Bool
    let hover: Hover
    let toggle: () -> Void
    let open: () -> Void

    var body: some View {
        let path = entry.path.string ?? ""
        let name = entry.displayName
        HStack(spacing: 12) {
            // overview rows (Home, Applications…) are summaries: never offered for removal
            Button(action: toggle) {
                Image(systemName: selected ? "checkmark.circle.fill" : "circle").font(.app(17))
            }
            .buttonStyle(.plain)
            .opacity(entry.selectable.bool ? 1 : 0)
            .disabled(!entry.selectable.bool)
            Image(nsImage: fileIcon(path)).resizable().frame(width: 34, height: 34)
            VStack(alignment: .leading, spacing: 2) {
                Text(verbatim: name).font(.app(14, weight: .medium)).lineLimit(1)
                if entry.cleanable.bool {
                    Text("Safe to clean").font(.app(12)).foregroundStyle(Color.ok)
                } else if entry.scan_status.string == "partial" {
                    Text("Partly measured").font(.app(12)).foregroundStyle(.dim)
                }
            }
            Spacer()
            Text(verbatim: bytes(entry.size.double)).font(.app(14)).monospacedDigit()
            Image(systemName: "chevron.right").foregroundStyle(.dim).opacity(entry.is_dir.bool ? 1 : 0)
        }
        .padding(.horizontal, 12).padding(.vertical, 8)
        .background(RoundedRectangle(cornerRadius: 12).fill(.white.opacity(selected || hover.path == path ? 0.12 : 0)))
        .contentShape(Rectangle())
        .onTapGesture(perform: open)
        .onHover { inside in
            if inside { hover.path = path } else if hover.path == path { hover.path = nil }
        }
        // one VoiceOver element per row: "<name>, <size>", opened with the default action
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(Text(verbatim: "\(name), \(bytes(entry.size.double))"))
        .accessibilityValue(entry.selectable.bool ? (selected ? String(localized: "Selected") : String(localized: "Not selected")) : "")
        .accessibilityAddTraits(entry.is_dir.bool ? .isButton : [])
        .accessibilityAction(.default, open)
        .accessibilityAction(named: String(localized: "Open"), open)
        .accessibilityAction(named: selected ? String(localized: "Deselect") : String(localized: "Select"), toggle)
        .peekable(path)
        .contextMenu { FileMenu(path: path) }
    }
}

extension JSON {
    /// A Space Lens entry's name; the overview's rows (never selectable) are the engine's
    /// English area names ("Home", "Applications"…), shown in the user's language.
    var displayName: String {
        let name = self.name.string ?? "?"
        return self.selectable.bool ? name : tr(name)
    }
}

// MARK: bubble map

/// What the map draws: the folders that matter, plus one "Others" bubble for the long tail.
struct BubbleItem: Identifiable {
    let id: String
    let name: String
    let size: Double
    let entry: JSON?

    /// Entries under 1.5% of the total would be specks; they stay in the list, merged here.
    static func from(_ entries: [JSON]) -> [BubbleItem] {
        let sized = entries.filter { ($0.size.double ?? 0) > 0 }.sorted { ($0.size.double ?? 0) > ($1.size.double ?? 0) }
        let total = sized.compactMap(\.size.double).reduce(0, +)
        guard total > 0 else { return [] }
        let big = sized.prefix(14).filter { ($0.size.double ?? 0) >= total * 0.015 }
        var items = big.enumerated().map { i, e in
            BubbleItem(id: e.path.string ?? "\(i)", name: e.displayName, size: e.size.double ?? 0, entry: e)
        }
        let rest = total - big.compactMap(\.size.double).reduce(0, +)
        if rest >= total * 0.015 {
            items.append(BubbleItem(id: "others", name: String(localized: "Others"), size: rest, entry: nil))
        }
        return items
    }
}

struct BubbleChart: View {
    let entries: [JSON]
    let hover: Hover
    let open: (JSON) -> Void
    @State private var appeared = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    /// Hover re-renders the chart many times a second; the packing only changes with the folder,
    /// so the unit-radius layout is kept and just scaled.
    @MainActor private static var memo: (sizes: [Double], unit: [Bubble]) = ([], [])

    @MainActor static func layout(_ sizes: [Double], radius: CGFloat) -> [Bubble] {
        if memo.sizes != sizes { memo = (sizes, pack(sizes, radius: 1)) }
        return memo.unit.map { Bubble(index: $0.index, center: CGPoint(x: $0.center.x * radius, y: $0.center.y * radius), r: $0.r * radius) }
    }

    private static let tints = ["#5EE6D0", "#4FC3F7", "#7C9CFF", "#B388FF", "#64B5F6"].map(Color.init(hex:))

    var body: some View {
        let items = BubbleItem.from(entries)
        GeometryReader { g in
            let side = min(g.size.width, g.size.height)
            let bubbles = Self.layout(items.map(\.size), radius: side / 2 * 0.9)
            ZStack {
                Circle()
                    .fill(RadialGradient(colors: [.white.opacity(0.16), .white.opacity(0.03)], center: .topLeading, startRadius: 0, endRadius: side))
                    .glassEffect(.clear, in: .circle)
                    .frame(width: side, height: side)
                    .scaleEffect(appeared ? 1 : 0.92)
                ForEach(Array(bubbles.enumerated()), id: \.element.index) { rank, b in
                    BubbleView(item: items[b.index], r: b.r, rank: rank, appeared: appeared || reduceMotion,
                               tint: items[b.index].entry == nil ? Color.white : Self.tints[rank % Self.tints.count],
                               hover: hover, reduceMotion: reduceMotion, open: open)
                        .position(x: g.size.width / 2 + b.center.x, y: g.size.height / 2 + b.center.y)
                }
            }
            .frame(width: g.size.width, height: g.size.height)
        }
        .onAppear { appeared = true }
        // the list next to it says the same, row by row
        .accessibilityHidden(true)
    }
}

/// One bubble; reads the hover itself, so pointing at it redraws it, not the chart.
private struct BubbleView: View {
    let item: BubbleItem
    let r: CGFloat
    let rank: Int
    let appeared: Bool
    let tint: Color
    let hover: Hover
    let reduceMotion: Bool
    let open: (JSON) -> Void

    var body: some View {
        let isHovered = hover.path == item.id
        Circle()
            .fill(RadialGradient(colors: [tint.opacity(isHovered ? 0.55 : 0.38), tint.opacity(isHovered ? 0.3 : 0.16)],
                                 center: .topLeading, startRadius: 0, endRadius: r * 2))
            .overlay(Circle().strokeBorder(.white.opacity(isHovered ? 0.6 : 0.3), lineWidth: 1))
            .overlay { label }
            .frame(width: r * 2, height: r * 2)
            .contentShape(.circle)
            .scaleEffect(appeared ? (isHovered ? 1.05 : 1) : 0.2)
            .opacity(appeared ? 1 : 0)
            .animation(reduceMotion ? nil : .spring(response: 0.55, dampingFraction: 0.7).delay(Double(rank) * 0.04), value: appeared)
            .animation(reduceMotion ? nil : .spring(response: 0.3, dampingFraction: 0.7), value: isHovered)
            .onHover { inside in
                if inside { hover.path = item.id } else if hover.path == item.id { hover.path = nil }
            }
            .onTapGesture { if let e = item.entry { open(e) } }
            .help(item.name + " · " + bytes(item.size))
    }

    @ViewBuilder private var label: some View {
        let icon = Group {
            if let path = item.entry?.path.string {
                Image(nsImage: fileIcon(path)).resizable()
            } else {
                Image(systemName: "ellipsis.circle").resizable().foregroundStyle(.white.opacity(0.85))
            }
        }
        if r >= 48 {
            VStack(spacing: 4) {
                icon.frame(width: min(r * 0.42, 64), height: min(r * 0.42, 64))
                Text(verbatim: item.name).font(.app(13, weight: .semibold)).lineLimit(1).minimumScaleFactor(0.7)
                Text(verbatim: bytes(item.size)).font(.app(12)).monospacedDigit().foregroundStyle(.dim)
            }
            .frame(width: r * 1.5)
        } else if r >= 20 {
            icon.frame(width: r * 0.8, height: r * 0.8)
        }
    }
}

struct Bubble {
    let index: Int
    var center: CGPoint
    var r: CGFloat
}

/// Area-proportional circles, largest first, each dropped at the first free spot along an
/// outward spiral, then scaled to fit `radius`.
/// ponytail: greedy spiral, O(n²·steps); fine for the ≤15 bubbles shown, use d3-style front-chain packing if n grows
func pack(_ sizes: [Double], radius: CGFloat) -> [Bubble] {
    let order = sizes.indices.sorted { sizes[$0] > sizes[$1] }
    let maxSize = max(sizes.max() ?? 1, 1)
    var placed: [Bubble] = []
    for i in order {
        let r = max(CGFloat((sizes[i] / maxSize).squareRoot()), 0.06)
        let gap: CGFloat = 0.03
        var angle: CGFloat = 0, p = CGPoint.zero
        while placed.contains(where: { hypot($0.center.x - p.x, $0.center.y - p.y) < $0.r + r + gap }) {
            angle += 0.12
            let d = angle * 0.04
            p = CGPoint(x: cos(angle) * d, y: sin(angle) * d)
        }
        placed.append(Bubble(index: i, center: p, r: r))
    }
    guard !placed.isEmpty else { return [] }
    let minX = placed.map { $0.center.x - $0.r }.min() ?? 0, maxX = placed.map { $0.center.x + $0.r }.max() ?? 0
    let minY = placed.map { $0.center.y - $0.r }.min() ?? 0, maxY = placed.map { $0.center.y + $0.r }.max() ?? 0
    let mid = CGPoint(x: (minX + maxX) / 2, y: (minY + maxY) / 2)
    let extent = placed.map { hypot($0.center.x - mid.x, $0.center.y - mid.y) + $0.r }.max() ?? 1
    let k = radius / extent
    return placed.map { Bubble(index: $0.index, center: CGPoint(x: ($0.center.x - mid.x) * k, y: ($0.center.y - mid.y) * k), r: $0.r * k) }
}
