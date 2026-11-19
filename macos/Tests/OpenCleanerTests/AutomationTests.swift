import Foundation
import Testing
@testable import OpenCleaner

@Test func unattendedCleanTakesOnlyDefaultReadyRulesAndNeverTheTrash() {
    let plan = JSON([
        "plan": 7,
        "groups": [
            ["id": "caches", "rules": [
                ["id": "user_caches", "group": "caches", "default": true, "status": "ready", "bytes": 100],
                ["id": "off_by_default", "group": "caches", "default": false, "status": "ready", "bytes": 50],
                ["id": "app_running", "group": "caches", "default": true, "status": "in_use", "bytes": 10],
                ["id": "nothing", "group": "caches", "default": true, "status": "empty", "bytes": 0],
            ]],
            ["id": "trash", "rules": [
                ["id": "trash", "group": "trash", "default": true, "status": "ready", "bytes": 999],
                ["id": "other_trash", "group": "trash", "default": true, "status": "ready", "bytes": 999],
            ]],
        ],
    ] as [String: Any])
    #expect(AutoClean.safeRules(plan).compactMap(\.id.string) == ["user_caches"])
    #expect(AutoClean.safeRules(JSON(nil)).isEmpty)
}

@Test func scheduleIsDueAfterAWeekOrAMonth() {
    var cal = Calendar(identifier: .gregorian)
    cal.timeZone = TimeZone(identifier: "UTC")!
    let jan1 = cal.date(from: DateComponents(year: 2026, month: 1, day: 1))!
    let day = 86400.0
    #expect(AutoClean.isDue(last: nil, now: jan1, monthly: false, calendar: cal))
    #expect(!AutoClean.isDue(last: jan1, now: jan1 + 6 * day, monthly: false, calendar: cal))
    #expect(AutoClean.isDue(last: jan1, now: jan1 + 7 * day, monthly: false, calendar: cal))
    #expect(!AutoClean.isDue(last: jan1, now: jan1 + 30 * day, monthly: true, calendar: cal))
    #expect(AutoClean.isDue(last: jan1, now: jan1 + 31 * day, monthly: true, calendar: cal))
}

@Test func lowDiskAlertsOnceUntilSpaceRecovers() {
    let t = 10e9
    #expect(AutoClean.lowDisk(free: 9e9, threshold: t, notified: false) == (true, true))
    #expect(AutoClean.lowDisk(free: 8e9, threshold: t, notified: true) == (false, true))
    #expect(AutoClean.lowDisk(free: 11e9, threshold: t, notified: true) == (false, true))
    #expect(AutoClean.lowDisk(free: 12.5e9, threshold: t, notified: true) == (false, false))
    #expect(AutoClean.lowDisk(free: 9e9, threshold: t, notified: false) == (true, true))
}
