import SwiftUI

/// Intro → Scan (dry run) → review → Run, the CleanMyMac flow, for any tool in cli.json.
struct ToolPage: View {
    let tool: Tool
    @State private var session: Session?
    @State private var confirming = false

    var body: some View {
        VStack(spacing: 0) {
            if let session {
                SessionView(session: session, module: .tool(tool.id))
            } else {
                Intro(module: .tool(tool.id), subtitle: tool.subtitle, points: tool.points)
            }
            Group {
                if let session {
                    Observe(session: session) { bar(for: $0) }
                } else {
                    bar(for: nil)
                }
            }
            .padding(.bottom, 28)
        }
        .padding(.horizontal, 32)
        .confirmationDialog(tool.confirm.map(tr) ?? "", isPresented: $confirming) {
            Button(tool.title, role: .destructive) { start(tool.run) }
        }
    }

    /// Takes the session as a parameter so the bar re-renders when it starts or finishes.
    private func bar(for current: Session?) -> some View {
        // busy until the process has really exited, so Run never shows before the scan ends
        let running = current != nil && current?.exitCode == nil
        let scanned = current?.isPreview == true && !running
        return ActionBar {
            if current != nil, !running {
                Button(String(localized: "Start Over")) { session = nil }.buttonStyle(.pill)
            }
        } center: {
            if running {
                BigButton(title: "", busy: true) {}
            } else if tool.terminal {
                BigButton(title: String(localized: "Open")) { CLI.openInTerminal(tool.run) }
            } else if current == nil, !tool.preview.isEmpty {
                BigButton(title: String(localized: "Scan")) { start(tool.preview, preview: true) }
            } else if current == nil || scanned {
                BigButton(title: String(localized: "Run")) { tool.confirm == nil ? start(tool.run) : (confirming = true) }
            }
        } trailing: {
            if running {
                Button(String(localized: "Stop")) { current?.interrupt() }.buttonStyle(.pill)
            } else if tool.terminal, !tool.preview.isEmpty, current == nil {
                Button(String(localized: "Preview")) { start(tool.preview, preview: true) }.buttonStyle(.pill)
            }
        }
    }

    private func start(_ args: [String], preview: Bool = false) {
        let s = Session(title: tool.title, args: args)
        s.isPreview = preview
        session = s
    }
}

/// Live output of a command: mo's closing report up top, the full log below, and a reply box
/// while it runs (password, y/n, Return).
struct SessionView: View {
    @ObservedObject var session: Session
    let module: Module
    @State private var input = ""
    @State private var secure = false

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack(spacing: 16) {
                // a finished real run swaps the tool art for the "done" art
                if session.exitCode == 0, !session.isPreview {
                    Art(id: "done", symbol: "checkmark.seal", size: 72)
                } else {
                    Art(id: module.artID, symbol: module.icon, size: 72)
                }
                VStack(alignment: .leading, spacing: 4) {
                    Text(session.title).font(.system(size: 24, weight: .bold))
                    Text(status).foregroundStyle(.dim)
                }
            }
            .padding(.top, 36)

            if !session.summary.isEmpty {
                VStack(alignment: .leading, spacing: 4) {
                    ForEach(session.summary, id: \.self) { Text($0) }
                }
                .font(.system(size: 15, weight: .medium))
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(18)
                .glassPanel()
            }

            ScrollViewReader { proxy in
                ScrollView {
                    Text(session.text.isEmpty ? " " : session.text)
                        .font(.system(size: 13, design: .monospaced))
                        .lineSpacing(3)
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(16)
                    Color.clear.frame(height: 1).id("end")
                }
                .brightScroller()
                .onChange(of: session.text) {
                    proxy.scrollTo("end")
                    // last non-empty line, read from the end instead of splitting the whole log
                    let lastLine = String(session.text.reversed().drop { $0 == "\n" }.prefix { $0 != "\n" }.reversed())
                    secure = lastLine.lowercased().contains("password")
                }
            }
            .glassPanel()

            if session.running {
                HStack {
                    ZStack(alignment: .leading) {
                        // macOS ignores placeholder colors, so draw our own readable one
                        if input.isEmpty {
                            Text(secure ? String(localized: "Mac password") : String(localized: "Reply (y / n…), or leave empty and press Return"))
                                .foregroundStyle(.dim)
                                .allowsHitTesting(false)
                        }
                        if secure {
                            SecureField("", text: $input)
                        } else {
                            TextField("", text: $input)
                        }
                    }
                    .textFieldStyle(.plain)
                    .font(.system(size: 14))
                    .onSubmit(send)
                    Button(String(localized: "Send"), action: send).buttonStyle(.pill)
                    Button(String(localized: "Open in Terminal")) {
                        session.interrupt()
                        CLI.openInTerminal(session.args)
                    }
                    .buttonStyle(.pill)
                    .help(String(localized: "Use this when the command needs arrow-key selection"))
                }
                .padding(.leading, 16).padding(6)
                .glassEffect(.clear.tint(.white.opacity(0.07)), in: .capsule)
            }
        }
        .task { session.start() }
        .onDisappear { if session.running { session.interrupt() } }
    }

    private var status: String {
        if session.running { return session.isPreview ? String(localized: "Scanning…") : String(localized: "Running…") }
        switch session.exitCode {
        case nil: return String(localized: "Starting…")
        case 0: return session.isPreview ? String(localized: "Scan complete, nothing was changed") : String(localized: "Done")
        case let code?: return String(localized: "Exited with code \(code)")
        }
    }

    private func send() {
        session.send(input)
        input = ""
    }
}

/// Re-renders `content` whenever the session publishes (running, exit code, output).
struct Observe<Content: View>: View {
    @ObservedObject var session: Session
    @ViewBuilder var content: (Session) -> Content

    var body: some View { content(session) }
}
