// SPDX-License-Identifier: GPL-3.0-or-later
import Foundation

/// Entry point. Normally launches the SwiftUI menu-bar app. With
/// `selftest <baseURL> <token>` it runs a headless check of the Kimai client
/// (connect, list, start, stop) and exits — used for CI / manual verification
/// against `build-aux/mock-kimai.py`.
@main
struct Main {
    static func main() {
        let args = CommandLine.arguments
        if args.count >= 2, args[1] == "selftest" {
            runSelfTest(args: args)
            return
        }
        StempeluhrApp.main()
    }

    private static func runSelfTest(args: [String]) {
        guard args.count >= 4 else {
            FileHandle.standardError.write(Data("usage: Stempeluhr selftest <baseURL> <token>\n".utf8))
            exit(2)
        }
        let base = args[2], token = args[3]
        let sem = DispatchSemaphore(value: 0)
        var ok = false
        Task {
            ok = await SelfTest.run(base: base, token: token)
            sem.signal()
        }
        sem.wait()
        exit(ok ? 0 : 1)
    }
}

enum SelfTest {
    static func run(base: String, token: String) async -> Bool {
        let client = KimaiClient(baseURL: base, token: token)
        func log(_ s: String) { print("• \(s)") }
        do {
            let (v, u) = try await client.testConnection()
            log("connected: Kimai \(v.version), user \(u.username ?? "?"), tz \(u.timezone ?? "?")")

            let projects = try await client.projects()
            log("projects: \(projects.count) — \(projects.map(\.name).prefix(3).joined(separator: ", "))")
            guard let p = projects.first else { print("✗ no projects"); return false }

            let activities = try await client.activities(project: p.id)
            log("activities for \(p.name): \(activities.map(\.name).joined(separator: ", "))")
            guard let a = activities.first else { print("✗ no activities"); return false }

            let before = try await client.active()
            log("active before: \(before.value.count) running, clock offset \(before.serverDate.map { String(format: "%.0fs", KimaiTime.clockOffset(serverDate: $0, received: before.received)) } ?? "n/a")")

            let now = Date().addingTimeInterval(before.serverDate.map { KimaiTime.clockOffset(serverDate: $0, received: before.received) } ?? 0)
            let started = try await client.start(StartRequest(project: p.id, activity: a.id, description: "selftest"), now: now)
            log("started entry #\(started.value.id) at \(Fmt.clock(started.value.begin))")

            let during = try await client.active()
            guard Tracking.fromActive(during.value).runningTimesheet?.id == started.value.id else {
                print("✗ started entry not reported as active"); return false
            }
            log("active during: running #\(started.value.id) ✓")

            let stopped = try await client.stop(id: started.value.id)
            log("stopped entry #\(stopped.value.id), end \(stopped.value.end.map(Fmt.clock) ?? "nil")")

            let after = try await client.active()
            guard Tracking.fromActive(after.value).runningTimesheet == nil else {
                print("✗ still running after stop"); return false
            }
            log("active after: idle ✓")

            print("✓ self-test passed")
            return true
        } catch {
            print("✗ \((error as? KimaiError)?.errorDescription ?? error.localizedDescription)")
            return false
        }
    }
}
