import AppKit
import SwiftUI

struct Volume: Identifiable {
    let url: URL
    let name: String
    let total: Double
    let free: Double
    var id: String { url.path }
    var isStartup: Bool { url.path == "/" }
    var used: Double { total - free }

    static func all() -> [Volume] {
        let keys: [URLResourceKey] = [.volumeNameKey, .volumeTotalCapacityKey, .volumeAvailableCapacityForImportantUsageKey]
        let urls = FileManager.default.mountedVolumeURLs(includingResourceValuesForKeys: keys, options: [.skipHiddenVolumes]) ?? []
        return urls.compactMap { url in
            guard let v = try? url.resourceValues(forKeys: Set(keys)), let total = v.volumeTotalCapacity, total > 0 else { return nil }
            return Volume(url: url, name: v.volumeName ?? url.lastPathComponent, total: Double(total), free: Double(v.volumeAvailableCapacityForImportantUsage ?? 0))
        }
    }
}

/// Lives as long as the window, so results and the sidebar badge survive switching modules.
@MainActor
final class SpaceLensModel: ObservableObject {
    @Published var volumes = Volume.all()
    @Published var volume: Volume?
    /// folders opened so far; `nil` is the startup disk overview
    @Published var stack: [String?] = []
    @Published var selection: Set<String> = []
    @Published var error: String?
    @Published private var cache: [String: JSON] = [:]
    /// keys whose `cache` entry came from disk (an earlier launch) and is being re-scanned
    @Published private var stale: [String: Date] = [:]
    /// keys whose `cache` entry is a scan still in progress (sizes are still growing)
    @Published private var live: Set<String> = []

    var usedLabel: Text? {
        volumes.first(where: \.isStartup).map { Text(verbatim: bytes($0.used)) }
    }

    var current: JSON? { stack.last.map { cache[$0 ?? ""] } ?? nil }
    /// When the shown results were really taken, if they are an older scan being refreshed.
    var staleDate: Date? { stack.last.map { stale[$0 ?? ""] } ?? nil }
    var isLive: Bool { stack.last.map { live.contains($0 ?? "") } ?? false }
    var entries: [JSON] { current?.entries.array ?? [] }
    var selectedSize: Double {
        entries.filter { selection.contains($0.path.string ?? "") }.compactMap(\.size.double).reduce(0, +)
    }

    func scan(_ v: Volume) {
        volume = v
        stack = []
        open(v.isStartup ? nil : v.url.path)
    }

    func open(_ path: String?) {
        stack.append(path)
        selection = []
        load(path)
    }

    func back() {
        guard !stack.isEmpty else { return }
        stack.removeLast()
        selection = []
        cancelPrefetch()
        if stack.isEmpty { volume = nil } else if let current { prefetchChildren(of: current) }
    }

    func startOver() {
        cancelPrefetch()
        stack = []
        volume = nil
        selection = []
    }

    /// One scan per folder at a time; opening a folder that is being prefetched restarts it in the foreground.
    private var inflight: [String: Task<JSON, Error>] = [:]
    /// keys in `inflight` that run at utility QoS (prefetch)
    private var backgroundKeys: Set<String> = []
    private var queue: [String] = []
    private var draining = false

    /// The user moved: drop speculative scans so they stop competing for the disk, and so a
    /// folder they open is scanned at full priority instead of joining a throttled prefetch.
    /// A cancelled scan restarts quickly: the file system metadata it touched is still cached.
    private func cancelPrefetch() {
        queue = []
        for key in backgroundKeys { inflight[key]?.cancel(); inflight[key] = nil }
        backgroundKeys = []
    }

    func load(_ path: String?, force: Bool = false) {
        let key = path ?? ""
        cancelPrefetch()
        if force { cache[key] = nil; stale[key] = nil }
        error = nil
        if let ready = cache[key], stale[key] == nil {
            prefetchChildren(of: ready)
            return
        }
        // show the previous launch's scan right away; the fresh one replaces it
        if !force, cache[key] == nil, let old = ResultCache.load(Self.cacheKey(key)) {
            cache[key] = old.json
            stale[key] = old.date
        }
        Task {
            do {
                prefetchChildren(of: try await scan(key, background: false))
            } catch {
                if stack.last.map({ $0 ?? "" }) == key { self.error = error.localizedDescription }
            }
        }
    }

    /// Starts mapping the startup disk (~30 s) while the volume picker is shown. Full priority:
    /// the user came to this page to scan, and Scan then joins this run instead of restarting it.
    func warmStartupDisk() {
        guard cache[""] == nil || stale[""] != nil, inflight[""] == nil else { return }
        Task { _ = try? await scan("", background: false) }
    }

    private static func cacheKey(_ key: String) -> [String] { ["space", key] }

    struct EngineFailure: LocalizedError {
        let errorDescription: String?
    }

    /// One engine scan. A foreground scan of a folder with nothing to show yet publishes its
    /// partial listings as it goes, so the map fills in instead of waiting on a spinner.
    private func engineScan(_ key: String, background: Bool) async throws -> JSON {
        var result = JSON(nil)
        let request: [String: Any] = key.isEmpty ? ["op": "space"] : ["op": "space", "path": key]
        for await event in Engine.run(request) {
            switch event.event.string {
            case "progress" where !background && (cache[key] == nil || live.contains(key)):
                cache[key] = event
                live.insert(key)
            case "space":
                result = event
            default:
                break
            }
        }
        try Task.checkCancellation()
        if let message = result.error.string { throw EngineFailure(errorDescription: message) }
        guard let raw = result.raw else { throw CancellationError() }
        if let data = try? JSONSerialization.data(withJSONObject: raw) {
            try? ResultCache.save(data, for: Self.cacheKey(key))
        }
        return result
    }

    private func scan(_ key: String, background: Bool) async throws -> JSON {
        if let cached = cache[key], stale[key] == nil, !live.contains(key) { return cached }
        let task = inflight[key] ?? Task { try await engineScan(key, background: background) }
        if inflight[key] == nil, background { backgroundKeys.insert(key) }
        inflight[key] = task
        // a cancelled prefetch must not unregister the foreground scan that replaced it
        defer {
            if inflight[key] == task {
                inflight[key] = nil
                backgroundKeys.remove(key)
            }
        }
        do {
            let json = try await task.value
            cache[key] = json
            stale[key] = nil
            live.remove(key)
            return json
        } catch {
            // a half-measured listing must not pass for a finished one
            if live.remove(key) != nil { cache[key] = nil }
            throw error
        }
    }

    /// Quietly scans the biggest subfolders next (the engine lists entries largest first), so the folder
    /// the user most likely opens is already there. Replaces the queue when they navigate.
    private func prefetchChildren(of json: JSON) {
        queue = Array(json.entries.array.filter(\.is_dir.bool).compactMap(\.path.string).filter { cache[$0] == nil }.prefix(4))
        drain()
    }

    private func drain() {
        guard !draining else { return }
        draining = true
        Task {
            while !queue.isEmpty {
                _ = try? await scan(queue.removeFirst(), background: true)
            }
            draining = false
        }
    }

    /// Moves one item to the Trash. macOS *copies* what it cannot remove (read-only or
    /// protected volumes) into the Trash and still reports success: that copy is taken
    /// back out and the move counts as failed.
    static func moveToTrash(_ path: String) -> Bool {
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
    func removeSelected() {
        var failed: [String] = []
        let removable = Set(entries.filter(\.selectable.bool).compactMap(\.path.string))
        for path in selection where removable.contains(path) {
            if !Self.moveToTrash(path) { failed.append((path as NSString).lastPathComponent) }
        }
        selection = []
        error = failed.isEmpty ? nil : String(localized: "Could not move to Trash: \(failed.joined(separator: ", "))")
        cache = [:]
        stale = [:]
        live = []
        volumes = Volume.all()
        load(stack.last ?? nil, force: true)
    }
}

struct SpaceLensPage: View {
    @ObservedObject var model: SpaceLensModel
    @State private var hovered: String?
    @State private var confirming = false

    var body: some View {
        VStack(spacing: 0) {
            if model.volume == nil {
                chooser
            } else {
                results
            }
        }
        .padding(.horizontal, 28)
        .confirmationDialog(
            String(localized: "Move \(model.selection.count) items (\(bytes(model.selectedSize))) to the Trash?"),
            isPresented: $confirming
        ) {
            Button(String(localized: "Move to Trash"), role: .destructive) { model.removeSelected() }
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
        // start mapping the startup disk while the user is still looking at this page
        .onAppear { model.warmStartupDisk() }
    }

    private var results: some View {
        VStack(spacing: 14) {
            HStack(spacing: 10) {
                Button { model.back() } label: { Image(systemName: "chevron.left") }.buttonStyle(.pill)
                Text(breadcrumb).font(.system(size: 15, weight: .semibold)).lineLimit(1).truncationMode(.head)
                Spacer()
                Button { model.load(model.stack.last ?? nil, force: true) } label: { Image(systemName: "arrow.clockwise") }.buttonStyle(.pill)
                Button(String(localized: "Start Over")) { model.startOver() }.buttonStyle(.pill)
            }
            .padding(.top, 36)

            if let date = model.staleDate, model.current != nil {
                RefreshBanner(date: date, error: model.error)
            } else if model.isLive {
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small).brightScroller()
                    Text("Measuring… sizes keep growing until the scan finishes.")
                }
                .font(.system(size: 12)).foregroundStyle(.dim)
                .padding(.horizontal, 14).padding(.vertical, 6)
                .glassEffect(.clear.tint(.white.opacity(0.07)), in: .capsule)
                .frame(maxWidth: .infinity, alignment: .leading)
            }

            if let current = model.current {
                HStack(spacing: 24) {
                    list.frame(width: 420)
                    VStack(spacing: 10) {
                        BubbleChart(entries: model.entries, hovered: $hovered) { e in
                            if e.is_dir.bool, let p = e.path.string { model.open(p) }
                        }
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                        if current.scan_status.string == "partial" {
                            Text("Some folders could not be read fully; sizes are at least this big.")
                                .font(.system(size: 12)).foregroundStyle(.dim)
                                .multilineTextAlignment(.center)
                        }
                    }
                }
                .id(currentKey)
                .transition(.opacity.combined(with: .scale(scale: 0.98)))
            } else {
                VStack(spacing: 12) {
                    if let error = model.error {
                        Image(systemName: "exclamationmark.triangle").font(.largeTitle)
                        Text(error)
                    } else {
                        Art(id: "scanning", symbol: "magnifyingglass", size: 180)
                            .phaseAnimator([false, true]) { art, up in
                                art.scaleEffect(up ? 1.04 : 0.96).opacity(up ? 1 : 0.8)
                            } animation: { _ in .easeInOut(duration: 1.2) }
                        ProgressView().controlSize(.regular).brightScroller()
                        Text("Mapping your files. Large drives can take about a minute.").font(.system(size: 14)).foregroundStyle(.dim)
                    }
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            }

            bottomBar
        }
        .animation(.easeOut(duration: 0.25), value: currentKey)
        .animation(.easeOut(duration: 0.25), value: model.current == nil)
    }

    private var currentKey: String { model.stack.last.map { $0 ?? "/" } ?? "" }

    private var breadcrumb: String {
        guard let last = model.stack.last else { return "" }
        return last.map { ($0 as NSString).abbreviatingWithTildeInPath } ?? model.volume?.name ?? "/"
    }

    private var list: some View {
        ScrollView {
            LazyVStack(spacing: 4) {
                ForEach(Array(model.entries.enumerated()), id: \.offset) { _, e in
                    EntryRow(
                        entry: e,
                        selected: model.selection.contains(e.path.string ?? ""),
                        highlighted: hovered == e.path.string,
                        toggle: {
                            guard e.selectable.bool, let p = e.path.string else { return }
                            if model.selection.contains(p) { model.selection.remove(p) } else { model.selection.insert(p) }
                        },
                        open: { if e.is_dir.bool, let p = e.path.string { model.open(p) } }
                    )
                    .onHover { hovered = $0 ? e.path.string : nil }
                }
            }
            .padding(8)
        }
        .brightScroller()
        .glassPanel()
    }

    private var bottomBar: some View {
        ZStack(alignment: .leading) {
            if let v = model.volume {
                VStack(alignment: .leading, spacing: 6) {
                    Text(verbatim: "\(v.name): \(bytes(v.used)) / \(bytes(v.total))").font(.system(size: 13, weight: .medium))
                    UsageBar(fraction: v.used / v.total).frame(width: 220)
                }
            }
            ActionBar {
                EmptyView()
            } center: {
                if !model.selection.isEmpty { BigButton(title: String(localized: "Remove")) { confirming = true } }
            } trailing: {
                if !model.selection.isEmpty {
                    Text(verbatim: bytes(model.selectedSize)).font(.system(size: 17, weight: .semibold)).monospacedDigit()
                }
            }
        }
        .padding(.bottom, 12)
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
            Text(volume.name).font(.system(size: 15, weight: .semibold)).lineLimit(1)
            UsageBar(fraction: volume.used / volume.total)
            Text("\(bytes(volume.free)) free of \(bytes(volume.total))").font(.system(size: 12)).foregroundStyle(.dim)
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
            .onAppear { withAnimation(.easeOut(duration: 0.9)) { shown = fraction } }
            .onChange(of: fraction) { shown = fraction }
    }
}

struct EntryRow: View {
    let entry: JSON
    let selected: Bool
    let highlighted: Bool
    let toggle: () -> Void
    let open: () -> Void

    var body: some View {
        HStack(spacing: 12) {
            // overview rows (Home, Applications…) are summaries: never offered for removal
            Button(action: toggle) {
                Image(systemName: selected ? "checkmark.circle.fill" : "circle").font(.system(size: 17))
            }
            .buttonStyle(.plain)
            .opacity(entry.selectable.bool ? 1 : 0)
            .disabled(!entry.selectable.bool)
            Image(nsImage: fileIcon(entry.path.string ?? "")).resizable().frame(width: 34, height: 34)
            VStack(alignment: .leading, spacing: 2) {
                Text(verbatim: entry.name.string ?? "?").font(.system(size: 14, weight: .medium)).lineLimit(1)
                if entry.cleanable.bool {
                    Text("Safe to clean").font(.system(size: 12)).foregroundStyle(Color(hex: "#7CF2C8"))
                } else if entry.scan_status.string == "partial" {
                    Text("Partly measured").font(.system(size: 12)).foregroundStyle(.dim)
                }
            }
            Spacer()
            Text(verbatim: bytes(entry.size.double)).font(.system(size: 14)).monospacedDigit()
            Image(systemName: "chevron.right").foregroundStyle(.dim).opacity(entry.is_dir.bool ? 1 : 0)
        }
        .padding(.horizontal, 12).padding(.vertical, 8)
        .background(RoundedRectangle(cornerRadius: 12).fill(.white.opacity(selected || highlighted ? 0.12 : 0)))
        .contentShape(Rectangle())
        .onTapGesture(perform: open)
        .contextMenu {
            Button(String(localized: "Show in Finder")) { reveal(entry.path.string) }
        }
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
            BubbleItem(id: e.path.string ?? "\(i)", name: e.name.string ?? "?", size: e.size.double ?? 0, entry: e)
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
    @Binding var hovered: String?
    let open: (JSON) -> Void
    @State private var appeared = false

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
                    bubble(items[b.index], b, rank: rank)
                        .position(x: g.size.width / 2 + b.center.x, y: g.size.height / 2 + b.center.y)
                }
            }
            .frame(width: g.size.width, height: g.size.height)
        }
        .onAppear { appeared = true }
    }

    private func bubble(_ item: BubbleItem, _ b: Bubble, rank: Int) -> some View {
        let isHovered = hovered == item.id
        let tint = item.entry == nil ? Color.white : Self.tints[rank % Self.tints.count]
        return Circle()
            .fill(RadialGradient(colors: [tint.opacity(isHovered ? 0.55 : 0.38), tint.opacity(isHovered ? 0.3 : 0.16)],
                                 center: .topLeading, startRadius: 0, endRadius: b.r * 2))
            .overlay(Circle().strokeBorder(.white.opacity(isHovered ? 0.6 : 0.3), lineWidth: 1))
            .overlay { label(item, r: b.r) }
            .frame(width: b.r * 2, height: b.r * 2)
            .contentShape(.circle)
            .scaleEffect(appeared ? (isHovered ? 1.05 : 1) : 0.2)
            .opacity(appeared ? 1 : 0)
            .animation(.spring(response: 0.55, dampingFraction: 0.7).delay(Double(rank) * 0.04), value: appeared)
            .animation(.spring(response: 0.3, dampingFraction: 0.7), value: isHovered)
            .onHover { hovered = $0 ? item.id : nil }
            .onTapGesture { if let e = item.entry { open(e) } }
            .help(item.name + " · " + bytes(item.size))
    }

    @ViewBuilder private func label(_ item: BubbleItem, r: CGFloat) -> some View {
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
                Text(verbatim: item.name).font(.system(size: 13, weight: .semibold)).lineLimit(1).minimumScaleFactor(0.7)
                Text(verbatim: bytes(item.size)).font(.system(size: 12)).monospacedDigit().foregroundStyle(.dim)
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
