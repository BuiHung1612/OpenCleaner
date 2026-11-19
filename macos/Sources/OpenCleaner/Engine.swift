//! The Rust engine (engine/), linked into the app. Requests and events are JSON; see
//! engine/src/api.rs for what it answers.

import COcEngine
import Foundation

enum Engine {
    nonisolated(unsafe) private static let handle = oc_engine_new(nil)

    /// A quick request, answered at once.
    static func call(_ request: [String: Any]) -> JSON {
        guard let data = try? JSONSerialization.data(withJSONObject: request),
              let text = String(data: data, encoding: .utf8),
              let reply = oc_call(handle, text) else { return JSON(nil) }
        defer { oc_free(reply) }
        return JSON(data: Data(bytes: reply, count: strlen(reply)))
    }

    static var version: String? { call(["op": "version"]).version.string }

    /// Settings key: scans take every core at normal priority instead of making room.
    static let fastKey = "fastScans"
    static func setFast(_ fast: Bool) { _ = call(["op": "priority", "fast": fast]) }

    /// A long request as a job: every event until "done". Ending the stream early (the
    /// consumer stops or its task is cancelled) cancels the job.
    static func run(_ request: [String: Any]) -> AsyncStream<JSON> {
        AsyncStream { continuation in
            guard let data = try? JSONSerialization.data(withJSONObject: request),
                  let text = String(data: data, encoding: .utf8) else {
                continuation.finish()
                return
            }
            let box = Unmanaged.passRetained(Listener(continuation)).toOpaque()
            let job = oc_start(handle, text, { ctx, event in
                guard let ctx, let event else { return }
                let listener = Unmanaged<Listener>.fromOpaque(ctx)
                let json = JSON(data: Data(bytes: event, count: strlen(event)))
                if json.event.string == "done" {
                    listener.takeRetainedValue().continuation.finish()
                } else {
                    listener.takeUnretainedValue().continuation.yield(json)
                }
            }, box)
            continuation.onTermination = { _ in oc_cancel(handle, job) }
        }
    }

    /// Keeps the continuation alive for the engine's callback until "done".
    private final class Listener {
        let continuation: AsyncStream<JSON>.Continuation
        init(_ continuation: AsyncStream<JSON>.Continuation) { self.continuation = continuation }
    }
}
