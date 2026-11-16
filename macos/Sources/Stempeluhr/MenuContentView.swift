// SPDX-License-Identifier: GPL-3.0-or-later
import SwiftUI

/// The popover shown when the menu-bar item is clicked.
struct MenuContentView: View {
    @EnvironmentObject var store: AppStore

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            switch store.phase {
            case .notConfigured, .signInRequired:
                ConnectView()
            case .connected:
                connected
            }
        }
        .frame(width: 300)
    }

    // MARK: - Connected

    @ViewBuilder private var connected: some View {
        header
        Divider()
        if store.isRunning {
            runningBody
        } else {
            idleBody
        }
        Divider()
        footer
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: 6) {
            if store.isRunning, let t = store.tracking.runningTimesheet, let e = store.elapsed {
                HStack(alignment: .firstTextBaseline) {
                    Text(Fmt.hms(e))
                        .font(.system(size: 30, weight: .semibold, design: .rounded))
                        .monospacedDigit()
                    Spacer()
                    dot(color: Color(hex: t.project.color) ?? .green)
                }
                Text(Fmt.label(t.project))
                    .font(.headline)
                Text(Fmt.label(t.activity) + (t.project.parent.map { " · \($0)" } ?? ""))
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
                Text("Since \(Fmt.clock(t.begin))")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            } else {
                Text("Not tracking")
                    .font(.system(size: 20, weight: .semibold, design: .rounded))
                    .foregroundStyle(.secondary)
            }

            if !store.confirmed, let at = store.lastConfirmedAt {
                Label("Not confirmed · as of \(Fmt.asOf(at))", systemImage: "wifi.slash")
                    .font(.caption)
                    .foregroundStyle(.orange)
            }
        }
        .padding(12)
    }

    // MARK: - Running

    private var runningBody: some View {
        VStack(spacing: 8) {
            Button(action: { Task { await store.stop() } }) {
                Label("Stop", systemImage: "stop.fill")
                    .frame(maxWidth: .infinity)
            }
            .controlSize(.large)
            .buttonStyle(.borderedProminent)
            .tint(.red)
            .disabled(store.busy)

            if case .running(_, let others) = store.tracking, others > 0 {
                Text("\(others) more entr\(others == 1 ? "y" : "ies") also running")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
        .padding(12)
    }

    // MARK: - Idle

    private var idleBody: some View {
        VStack(alignment: .leading, spacing: 10) {
            pickerRow(title: "Project") {
                Picker("Project", selection: Binding(
                    get: { store.selectedProject ?? -1 },
                    set: { store.selectProject($0) }
                )) {
                    if store.selectedProject == nil { Text("Choose…").tag(-1) }
                    ForEach(store.projects) { p in
                        Text(p.customer.map { "\(p.name) · \($0)" } ?? p.name).tag(p.id)
                    }
                }
                .labelsHidden()
            }

            pickerRow(title: "Activity") {
                Picker("Activity", selection: Binding(
                    get: { store.selectedActivity ?? -1 },
                    set: { store.selectedActivity = $0 }
                )) {
                    if store.selectedActivity == nil { Text("Choose…").tag(-1) }
                    ForEach(store.activities) { a in
                        Text(a.name).tag(a.id)
                    }
                }
                .labelsHidden()
                .disabled(store.activities.isEmpty)
            }

            Button(action: { Task { await store.start() } }) {
                Label("Start", systemImage: "play.fill")
                    .frame(maxWidth: .infinity)
            }
            .controlSize(.large)
            .buttonStyle(.borderedProminent)
            .disabled(store.busy || store.selectedProject == nil || store.selectedActivity == nil)
        }
        .padding(12)
    }

    private func pickerRow<Content: View>(title: String, @ViewBuilder _ content: () -> Content) -> some View {
        HStack {
            Text(title)
                .font(.subheadline)
                .foregroundStyle(.secondary)
                .frame(width: 60, alignment: .leading)
            content()
        }
    }

    // MARK: - Footer

    private var footer: some View {
        VStack(spacing: 0) {
            if let err = store.lastError {
                Text(err)
                    .font(.caption)
                    .foregroundStyle(.red)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 12)
                    .padding(.top, 8)
            }
            HStack(spacing: 14) {
                Button(action: { Task { await store.refresh(reason: .manual) } }) {
                    Image(systemName: "arrow.clockwise")
                }
                .help("Refresh")

                if let host = URL(string: store.serverURL)?.host {
                    Text(host).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                }
                Spacer()
                Menu {
                    Button("Disconnect…") { store.disconnect() }
                    Divider()
                    Button("Quit Stempeluhr") { NSApplication.shared.terminate(nil) }
                } label: {
                    Image(systemName: "ellipsis.circle")
                }
                .menuStyle(.borderlessButton)
                .fixedSize()
            }
            .buttonStyle(.borderless)
            .padding(12)
        }
    }

    private func dot(color: Color) -> some View {
        Circle().fill(color).frame(width: 12, height: 12)
    }
}

extension Color {
    /// Parses `#rrggbb`; returns nil on anything else.
    init?(hex: String?) {
        guard var s = hex else { return nil }
        if s.hasPrefix("#") { s.removeFirst() }
        guard s.count == 6, let v = Int(s, radix: 16) else { return nil }
        self = Color(
            red: Double((v >> 16) & 0xff) / 255,
            green: Double((v >> 8) & 0xff) / 255,
            blue: Double(v & 0xff) / 255
        )
    }
}
