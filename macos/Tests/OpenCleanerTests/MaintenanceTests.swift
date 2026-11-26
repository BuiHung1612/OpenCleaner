import AppKit
import Testing
@testable import OpenCleaner

@Test func adminResultsKeepEachTasksFailureAndRejectMissingResults() {
    let output = """
    OPEN_CLEANER_TASK_RESULT\tdns\t1
    DNS service did not answer
    OPEN_CLEANER_TASK_RESULT\tother\t0
    OPEN_CLEANER_TASK_RESULT\tmemory\t0
    """
    let results = Privileged.taskResults(output, ids: ["dns", "memory", "missing"])
    #expect(results["dns"]?.contains("exit code 1") == true)
    #expect(results["dns"]?.contains("DNS service did not answer") == true)
    #expect(results["memory"] == "✓")
    #expect(results["missing"] != "✓" && results["other"] == nil)
    #expect(Privileged.taskResults("OPEN_CLEANER_TASK_RESULT\tdns\tinvalid", ids: ["dns"])["dns"] != "✓")
}

@MainActor
@Test func waitingForAppleScriptLeavesTheUIExecutorFree() async throws {
    // No authorization or maintenance: a plain AppleScript delay and return value.
    let operation = Task { await Privileged.execute("delay 1\nreturn \"finished\"") }
    let start = Date()
    try await Task.sleep(for: .milliseconds(20))
    #expect(Date().timeIntervalSince(start) < 0.8)
    switch await operation.value {
    case .done(let output): #expect(output == "finished")
    default: Issue.record("The harmless AppleScript did not complete")
    }
    if case .cancelled = await Privileged.execute("error number -128") {} else {
        Issue.record("User cancellation was not preserved")
    }
    if case .failed(let message) = await Privileged.execute("error \"test failure\" number 42") {
        #expect(message.contains("test failure"))
    } else { Issue.record("An AppleScript error was reported as success") }
}

@Test func adminBatchResultsSurviveTheShellAndAppleScriptBoundary() async throws {
    var script = try #require(Engine.call(["op": "optimize", "tasks": ["dns", "memory"]]).admin_script.string)
    let replacements = [
        "'/usr/bin/dscacheutil' '-flushcache'": "false",
        "'/usr/bin/killall' '-HUP' 'mDNSResponder'": "true",
        "'/usr/sbin/purge'": "true",
    ]
    for (command, replacement) in replacements {
        #expect(script.contains(command))
        script = script.replacingOccurrences(of: command, with: replacement)
    }
    // Replace all maintenance with shell builtins and omit authorization for this test.
    let source = Privileged.source(for: script).replacingOccurrences(of: " with administrator privileges", with: "")
    switch await Privileged.execute(source) {
    case .done(let output):
        let results = Privileged.taskResults(output, ids: ["dns", "memory"])
        #expect(results["dns"]?.contains("exit code 1") == true)
        #expect(results["memory"] == "✓")
    default: Issue.record("The harmless batch did not return task results")
    }
}
