// SPDX-License-Identifier: GPL-3.0-or-later
import Foundation
import AppKit
import Combine

/// What the server last told us is going on.
enum Tracking: Equatable {
    case idle
    case running(Timesheet, others: Int)

    /// Reduces `GET /timesheets/active` to what we display (newest wins).
    static func fromActive(_ list: [Timesheet]) -> Tracking {
        let running = list.filter { $0.isRunning }
            .sorted { ($0.begin, $0.id) > ($1.begin, $1.id) }
        guard let first = running.first else { return .idle }
        return .running(first, others: running.count - 1)
    }

    var runningTimesheet: Timesheet? {
        if case .running(let t, _) = self { return t }
        return nil
    }
}

enum Phase: Equatable {
    case notConfigured
    case signInRequired
    case connected
}

/// The app's single source of truth for the UI. Holds only what the server
/// confirmed; when the server can't be reached the last state is shown frozen
/// and marked "Not confirmed".
@MainActor
final class AppStore: ObservableObject {
    // Connection
    @Published private(set) var phase: Phase = .notConfigured
    @Published private(set) var serverURL: String = ""
    @Published private(set) var username: String?

    // Tracking state (server-confirmed)
    @Published private(set) var tracking: Tracking = .idle
    @Published private(set) var confirmed: Bool = false
    @Published private(set) var lastConfirmedAt: Date?
    @Published private(set) var clockOffset: TimeInterval = 0

    // UI feedback
    @Published private(set) var busy: Bool = false
    @Published var lastError: String?
    @Published private(set) var now: Date = Date()

    // Pickers / selection
    @Published private(set) var projects: [Project] = []
    @Published private(set) var activities: [Activity] = []
    @Published var selectedProject: Int? { didSet { persistSelection() } }
    @Published var selectedActivity: Int? { didSet { persistSelection() } }
    @Published private(set) var recent: [Timesheet] = []

    private var client: KimaiClient?
    private var pollTask: Task<Void, Never>?
    private var tickTimer: Timer?

    private let defaults = UserDefaults.standard
    private enum Keys {
        static let server = "serverURL"
        static let lastProject = "lastProject"
        static let lastActivity = "lastActivity"
    }

    init() {
        serverURL = defaults.string(forKey: Keys.server) ?? ""
        selectedProject = defaults.object(forKey: Keys.lastProject) as? Int
        selectedActivity = defaults.object(forKey: Keys.lastActivity) as? Int

        if !serverURL.isEmpty, let token = Keychain.loadToken() {
            client = KimaiClient(baseURL: serverURL, token: token)
            phase = .connected
        }
        startTicking()
        observeWake()
        Task { await bootstrap() }
    }

    // MARK: - Derived display

    /// Local wall clock adjusted by the server clock offset.
    var serverNow: Date { now.addingTimeInterval(clockOffset) }

    /// Seconds elapsed on the running entry, computed from the server `begin`.
    var elapsed: TimeInterval? {
        guard let t = tracking.runningTimesheet else { return nil }
        return max(0, serverNow.timeIntervalSince(t.begin))
    }

    var isRunning: Bool { tracking.runningTimesheet != nil }

    // MARK: - Lifecycle

    private func bootstrap() async {
        guard phase == .connected else { return }
        await refresh(reason: .startup)
        await loadProjects()
        if let p = selectedProject { await loadActivities(project: p) }
        startPolling()
    }

    private func startTicking() {
        tickTimer?.invalidate()
        let t = Timer(timeInterval: 1, repeats: true) { [weak self] _ in
            Task { @MainActor in self?.now = Date() }
        }
        RunLoop.main.add(t, forMode: .common)
        tickTimer = t
    }

    private func startPolling() {
        pollTask?.cancel()
        pollTask = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 45_000_000_000)
                if Task.isCancelled { break }
                await self?.refresh(reason: .timer)
            }
        }
    }

    private func observeWake() {
        NSWorkspace.shared.notificationCenter.addObserver(
            forName: NSWorkspace.didWakeNotification, object: nil, queue: .main
        ) { [weak self] _ in
            Task { @MainActor in await self?.refresh(reason: .resume) }
        }
    }

    enum RefreshReason { case startup, timer, resume, manual, write }

    // MARK: - Connection management

    /// Tests `server` + `token`; on success stores them and connects.
    func connect(server: String, token: String) async -> Bool {
        lastError = nil
        let probe = KimaiClient(baseURL: server, token: token)
        do {
            let (_, user) = try await probe.testConnection()
            let normalized = probe.baseURL
            Keychain.saveToken(token)
            defaults.set(normalized, forKey: Keys.server)
            serverURL = normalized
            username = user.username
            client = probe
            phase = .connected
            await refresh(reason: .startup)
            await loadProjects()
            if let p = selectedProject { await loadActivities(project: p) }
            startPolling()
            return true
        } catch {
            lastError = (error as? KimaiError)?.errorDescription ?? error.localizedDescription
            return false
        }
    }

    func disconnect() {
        pollTask?.cancel()
        Keychain.deleteToken()
        defaults.removeObject(forKey: Keys.server)
        client = nil
        phase = .notConfigured
        tracking = .idle
        confirmed = false
        username = nil
        projects = []
        activities = []
        recent = []
        serverURL = ""
    }

    // MARK: - Reads

    func refresh(reason: RefreshReason) async {
        guard let client else { return }
        do {
            let reply = try await client.active()
            if let date = reply.serverDate {
                clockOffset = KimaiTime.clockOffset(serverDate: date, received: reply.received)
            }
            tracking = Tracking.fromActive(reply.value)
            confirmed = true
            lastConfirmedAt = reply.received
            lastError = nil
            if phase == .signInRequired { phase = .connected }
        } catch let e as KimaiError {
            switch e {
            case .unauthorized:
                phase = .signInRequired
                confirmed = false
            default:
                // Keep showing the last known state, dimmed and frozen.
                confirmed = false
                lastError = e.errorDescription
            }
        } catch {
            confirmed = false
            lastError = error.localizedDescription
        }
    }

    func loadProjects() async {
        guard let client else { return }
        do { projects = try await client.projects() }
        catch { /* leave previous list; surfaced on next action */ }
    }

    func loadActivities(project: Int) async {
        guard let client else { return }
        do {
            activities = try await client.activities(project: project)
            // Drop a stale activity selection that no longer belongs here.
            if let a = selectedActivity, !activities.contains(where: { $0.id == a }) {
                selectedActivity = activities.first?.id
            } else if selectedActivity == nil {
                selectedActivity = activities.first?.id
            }
        } catch { activities = [] }
    }

    func loadRecent() async {
        guard let client else { return }
        let since = Calendar.current.date(byAdding: .day, value: -30, to: Date()) ?? Date()
        do { recent = Array((try await client.history(since: since)).prefix(8)) }
        catch { /* ignore */ }
    }

    // MARK: - Writes (never retried; server is re-checked afterwards)

    func start() async {
        guard let client, !busy else { return }
        guard let p = selectedProject, let a = selectedActivity else {
            lastError = "Pick a project and activity first"
            return
        }
        busy = true; defer { busy = false }
        lastError = nil

        // Check server truth first; don't create a second entry.
        await refresh(reason: .write)
        if isRunning { return }

        let req = StartRequest(project: p, activity: a, description: nil)
        do {
            _ = try await client.start(req, now: serverNow)
        } catch {
            lastError = (error as? KimaiError)?.errorDescription ?? error.localizedDescription
        }
        await refresh(reason: .write)
    }

    func stop() async {
        guard let client, !busy else { return }
        busy = true; defer { busy = false }
        lastError = nil

        await refresh(reason: .write)
        guard let id = tracking.runningTimesheet?.id else { return }
        do {
            _ = try await client.stop(id: id)
        } catch let e as KimaiError {
            // Already stopped elsewhere counts as success.
            if case .validation = e {} else { lastError = e.errorDescription }
        } catch {
            lastError = error.localizedDescription
        }
        await refresh(reason: .write)
    }

    func toggle() async {
        if isRunning { await stop() } else { await start() }
    }

    func selectProject(_ id: Int) {
        selectedProject = id
        Task { await loadActivities(project: id) }
    }

    // MARK: - Helpers

    private func persistSelection() {
        if let p = selectedProject { defaults.set(p, forKey: Keys.lastProject) }
        if let a = selectedActivity { defaults.set(a, forKey: Keys.lastActivity) }
    }
}
