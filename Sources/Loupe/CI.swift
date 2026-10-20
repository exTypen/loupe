import SwiftUI
import AppKit
import Observation

/// One row in the checks list: a commit status, linked to its Forgejo Actions job when it has one.
struct Check: Identifiable, Hashable {
    var status: CommitStatus
    var job: ActionJob?
    var id: String { status.context }

    var state: CheckState { CheckState(status: status.status, description: status.description) }
}

enum CheckState: Int, Comparable {
    case failing, running, pending, success, skipped

    init(status: String, description: String?) {
        switch status {
        case "failure", "error": self = .failing
        case "success": self = .success
        case "skipped": self = .skipped
        case "warning": self = .failing
        default: self = (description ?? "").localizedCaseInsensitiveContains("running") || (description ?? "").localizedCaseInsensitiveContains("started") ? .running : .pending
        }
    }

    init(jobStatus: String) {
        switch jobStatus {
        case "failure": self = .failing
        case "success": self = .success
        case "skipped", "cancelled": self = .skipped
        case "running": self = .running
        default: self = .pending
        }
    }

    static func < (a: CheckState, b: CheckState) -> Bool { a.rawValue < b.rawValue }

    var title: String { ["Failing", "Running", "Queued", "Successful", "Skipped"][rawValue] }
    var isDone: Bool { self == .failing || self == .success || self == .skipped }
}

@MainActor @Observable
final class ChecksModel {
    let client: ForgejoClient
    let repo: RemoteRepo
    var sha: String
    var checks: [Check] = []
    var loading = false
    var error: String?
    var lastUpdated: Date?
    private var runs: [Int: ActionRun] = [:]           // index_in_repo → run
    private var jobs: [Int: [ActionJob]] = [:]         // run id → jobs (latest attempt)

    init(client: ForgejoClient, repo: RemoteRepo, sha: String) {
        self.client = client; self.repo = repo; self.sha = sha
    }

    var summary: (failing: Int, running: Int, queued: Int, passed: Int, total: Int) {
        (checks.filter { $0.state == .failing }.count, checks.filter { $0.state == .running }.count,
         checks.filter { $0.state == .pending }.count, checks.filter { $0.state == .success }.count, checks.count)
    }
    var allDone: Bool { !checks.isEmpty && checks.allSatisfy { $0.state.isDone } }

    func refresh() async {
        loading = checks.isEmpty
        defer { loading = false }
        do {
            async let statusesReq = client.combinedStatus(repo, sha: sha)
            async let runsReq = client.actionRuns(repo, headSHA: sha)
            let (statuses, runList) = try await (statusesReq, runsReq)
            runs = Dictionary(runList.map { ($0.index_in_repo, $0) }, uniquingKeysWith: { a, _ in a })

            // Only re-fetch jobs for runs that can still change; finished runs are immutable.
            let stale = runList.filter { jobs[$0.id] == nil || !["success", "failure", "cancelled", "skipped"].contains($0.status ?? "") }
            try await withThrowingTaskGroup(of: (Int, [ActionJob]).self) { g in
                for run in stale { g.addTask { (run.id, try await self.client.actionJobs(self.repo, runID: run.id)) } }
                for try await (id, js) in g { jobs[id] = js }
            }

            checks = statuses.map { Check(status: $0, job: job(for: $0)) }
                .sorted { ($0.state, $0.status.context.lowercased()) < ($1.state, $1.status.context.lowercased()) }
            lastUpdated = Date()
            error = nil
        } catch { self.error = error.localizedDescription }
    }

    /// target_url "/acme/web/actions/runs/292970/jobs/4" → run index 292970, job #4 of that run.
    private func job(for s: CommitStatus) -> ActionJob? {
        guard let url = s.target_url, let r = url.range(of: "/actions/runs/") else { return nil }
        let parts = url[r.upperBound...].split(separator: "/")
        guard let index = Int(parts.first ?? ""), let run = runs[index], let list = jobs[run.id] else { return nil }
        let jobNo = parts.count >= 3 && parts[1] == "jobs" ? Int(parts[2]) : nil
        // Job html_url carries ".../runs/<index>/jobs/<n>/attempt/<a>"; keep the newest attempt per n.
        var byNo: [Int: ActionJob] = [:]
        for j in list {
            guard let u = j.html_url, let jr = u.range(of: "/jobs/") else { continue }
            let n = Int(u[jr.upperBound...].split(separator: "/").first ?? "") ?? -1
            if (byNo[n]?.attempt ?? 0) <= (j.attempt ?? 0) { byNo[n] = j }
        }
        if let jobNo { return byNo[jobNo] }
        return byNo.count == 1 ? byNo.values.first : nil
    }

    /// Poll loop for the modal's lifetime: fast while anything is moving, slow once everything has settled.
    func poll() async {
        while !Task.isCancelled {
            await refresh()
            try? await Task.sleep(for: allDone ? .seconds(30) : .seconds(5))
        }
    }
}

// MARK: - Log tailing

@MainActor @Observable
final class JobLogModel {
    let client: ForgejoClient
    let repo: RemoteRepo
    var job: ActionJob
    var problems: [(line: String, range: NSRange)] = []
    var truncatedBytes = 0                // bytes before what we show (we start from the tail of huge logs)
    var error: String?
    @ObservationIgnored let storage = NSTextStorage()
    @ObservationIgnored private var offset = 0
    @ObservationIgnored private var pending = Data()   // bytes after the last newline (a chunk can split a line or a UTF-8 char)
    var version = 0                                      // bumps on every append so the view can follow the tail

    static let initialTail = 16 << 20   // detail page fetches the whole log; only beyond 16 MB start from the tail

    init(client: ForgejoClient, repo: RemoteRepo, job: ActionJob) {
        self.client = client; self.repo = repo; self.job = job
    }

    var isRunning: Bool { !CheckState(jobStatus: job.status).isDone }

    func poll() async {
        do {
            let (data, start, total) = try await client.actionLog(repo, jobID: job.id, range: "bytes=-\(Self.initialTail)")
            truncatedBytes = start
            offset = (total ?? data.count)
            append(data)
        } catch { self.error = error.localizedDescription }
        while !Task.isCancelled {
            if let j = try? await client.actionJob(repo, id: job.id) { job = j }
            let running = isRunning
            if let (data, _, total) = try? await client.actionLog(repo, jobID: job.id, range: "bytes=\(offset)-"), !data.isEmpty {
                offset = total ?? offset + data.count
                append(data)
            }
            if !running { flushPending(); return }      // finished: one last read above, then stop
            try? await Task.sleep(for: .seconds(2))
        }
    }

    func loadEarlier() async {
        guard truncatedBytes > 0 else { return }
        do {
            let (data, _, _) = try await client.actionLog(repo, jobID: job.id, range: "bytes=0-\(truncatedBytes - 1)")
            let text = Self.format(String(decoding: data, as: UTF8.self))
            storage.insert(text, at: 0)
            let shift = text.length
            problems = Self.findProblems(text, base: 0) + problems.map { ($0.line, NSRange(location: $0.range.location + shift, length: $0.range.length)) }
            truncatedBytes = 0
            version += 1
        } catch { self.error = error.localizedDescription }
    }

    private func append(_ data: Data) {
        pending.append(data)
        guard let nl = pending.lastIndex(of: UInt8(ascii: "\n")) else { return }
        let chunk = pending[...nl]
        pending = Data(pending[pending.index(after: nl)...])
        emit(String(decoding: chunk, as: UTF8.self))
    }

    private func flushPending() {
        if !pending.isEmpty { emit(String(decoding: pending, as: UTF8.self) + "\n"); pending = Data() }
    }

    private func emit(_ s: String) {
        let text = Self.format(s)
        let base = storage.length
        storage.append(text)
        problems += Self.findProblems(text, base: base)
        version += 1
    }

    private static let mono = NSFont.monospacedSystemFont(ofSize: 11.5, weight: .regular)
    private static let monoBold = NSFont.monospacedSystemFont(ofSize: 11.5, weight: .semibold)

    static func isProblem(_ l: Substring) -> Bool {
        l.hasPrefix("##[error]") || l.contains("❌") || l.hasPrefix("error") || l.contains("error:") || l.contains("Error:")
            || l.contains("FAILED") || l.contains("panicked at") || l.hasPrefix("thread '") && l.contains("panicked")
    }

    /// Strips runner timestamps and turns workflow commands into readable, coloured lines.
    static func format(_ s: String) -> NSAttributedString {
        let out = NSMutableAttributedString()
        for raw in s.split(separator: "\n", omittingEmptySubsequences: false).dropLast(s.hasSuffix("\n") ? 1 : 0) {
            var l = raw
            // "2026-10-19T14:33:38.5145900Z " prefix
            if l.count > 29, l.dropFirst(4).first == "-", let z = l.prefix(32).firstIndex(of: "Z") {
                l = l[l.index(after: z)...]
                if l.first == " " { l = l.dropFirst() }   // only the separator; runner indentation is meaningful
            }
            if l.hasPrefix("##[endgroup]") { continue }
            var color = NSColor.labelColor, font = mono
            if l.hasPrefix("##[group]") { l = "▸ " + l.dropFirst(9); color = .secondaryLabelColor; font = monoBold }
            else if l.hasPrefix("##[error]") { l = l.dropFirst(9); color = .systemRed; font = monoBold }
            else if l.hasPrefix("##[warning]") { l = l.dropFirst(11); color = .systemOrange }
            else if l.hasPrefix("⭐") { color = .controlAccentColor; font = monoBold }
            else if l.contains("✅") { color = .systemGreen }
            else if isProblem(l) { color = .systemRed }
            out.append(NSAttributedString(string: l + "\n", attributes: [.font: font, .foregroundColor: color]))
        }
        return out
    }

    static func findProblems(_ text: NSAttributedString, base: Int) -> [(line: String, range: NSRange)] {
        var found: [(String, NSRange)] = []
        let ns = text.string as NSString
        ns.enumerateSubstrings(in: NSRange(location: 0, length: ns.length), options: .byLines) { sub, range, _, stop in
            guard let sub, isProblem(Substring(sub)) || (text.attribute(.foregroundColor, at: range.location, effectiveRange: nil) as? NSColor) == .systemRed else { return }
            found.append((sub.trimmingCharacters(in: .whitespaces), NSRange(location: range.location + base, length: range.length)))
            if found.count >= 200 { stop.pointee = true }
        }
        return found
    }
}

struct LogTextView: NSViewRepresentable {
    var model: JobLogModel
    var version: Int
    @Binding var scrollTarget: NSRange?

    func makeNSView(context: Context) -> NSScrollView {
        let scroll = NSTextView.scrollableTextView()
        let tv = scroll.documentView as! NSTextView
        tv.isEditable = false
        tv.isSelectable = true
        tv.drawsBackground = true
        tv.backgroundColor = .textBackgroundColor
        tv.textContainerInset = NSSize(width: 8, height: 8)
        tv.layoutManager?.allowsNonContiguousLayout = true    // multi-MB logs stay responsive
        tv.layoutManager?.replaceTextStorage(model.storage)
        tv.isAutomaticLinkDetectionEnabled = false
        // No wrapping: logs read better with horizontal scroll, and layout is far cheaper.
        tv.isHorizontallyResizable = true
        tv.textContainer?.widthTracksTextView = false
        tv.textContainer?.containerSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
        scroll.hasHorizontalScroller = true
        return scroll
    }

    func updateNSView(_ scroll: NSScrollView, context: Context) {
        guard let tv = scroll.documentView as? NSTextView else { return }
        if let target = scrollTarget {
            tv.scrollRangeToVisible(target)
            tv.setSelectedRange(target)
            tv.showFindIndicator(for: target)
            DispatchQueue.main.async { scrollTarget = nil }
            context.coordinator.lastVersion = version
            return
        }
        // Follow the tail only when the user is already at the bottom (or on first fill).
        if version != context.coordinator.lastVersion {
            let atBottom = scroll.contentView.bounds.maxY >= tv.frame.height - 40 || context.coordinator.lastVersion == 0
            context.coordinator.lastVersion = version
            if atBottom { tv.scrollToEndOfDocument(nil) }
        }
    }

    func makeCoordinator() -> Coordinator { Coordinator() }
    final class Coordinator { var lastVersion = 0 }
}

// MARK: - Views

/// Toolbar badge in the PR header: aggregate CI state, opens the checks modal.
struct ChecksBadge: View {
    var model: ChecksModel
    var action: () -> Void

    var body: some View {
        let s = model.summary
        Button(action: action) {
            HStack(spacing: 5) {
                if s.total == 0 { Image(systemName: "circle.dashed").foregroundStyle(.secondary); Text("Checks") }
                else {
                    if s.failing > 0 { Label(String(s.failing), systemImage: "xmark.circle.fill").foregroundStyle(.red) }
                    if s.running > 0 { Label(String(s.running), systemImage: "circle.fill").foregroundStyle(.orange).symbolEffect(.pulse) }
                    if s.queued > 0 { Label(String(s.queued), systemImage: "circle.dashed").foregroundStyle(.secondary) }
                    Label(String(s.passed), systemImage: "checkmark.circle.fill").foregroundStyle(.green)
                }
            }
            .labelStyle(.titleAndIcon)
            .monospacedDigit()
        }
        .help("CI checks — \(s.passed) passed, \(s.running) running, \(s.queued) queued, \(s.failing) failing")
        .task { await model.refresh() }
    }
}

struct ChecksSheet: View {
    var model: ChecksModel
    var baseURL: String
    var dismiss: () -> Void
    @State private var openCheck: String?        // non-nil → full-size detail page
    @State private var filter = ""

    var body: some View {
        VStack(spacing: 0) {
            if let id = openCheck, let c = model.checks.first(where: { $0.id == id }) {
                // Detail page: replaces the list and fetches everything (steps, full log).
                CheckDetail(check: c, model: model, baseURL: baseURL) { openCheck = nil }
                    .id(c.id + "\(c.job?.id ?? 0)")
            } else {
                header
                Divider()
                list
            }
        }
        .task { await model.poll() }
        .onExitCommand { if openCheck != nil { openCheck = nil } else { dismiss() } }   // Esc: back, then close
    }

    // The list only shows metadata the status/jobs endpoints already return — no logs are fetched here.
    private var list: some View {
        List {
            ForEach([CheckState.failing, .running, .pending, .success, .skipped], id: \.self) { state in
                let rows = model.checks.filter { $0.state == state && (filter.isEmpty || $0.status.context.localizedCaseInsensitiveContains(filter)) }
                if !rows.isEmpty {
                    Section("\(state.title) · \(String(rows.count))") {
                        ForEach(rows) { c in
                            Button { openCheck = c.id } label: { CheckRow(check: c).contentShape(Rectangle()) }
                                .buttonStyle(.plain)
                        }
                    }
                }
            }
        }
        .listStyle(.inset)
        .overlay {
            if model.loading { ProgressView() }
            else if model.checks.isEmpty { ContentUnavailableView("No Checks", systemImage: "circle.dashed", description: Text("Nothing has reported a status for this commit.")) }
        }
    }

    private var header: some View {
        let s = model.summary
        return HStack(spacing: 12) {
            let busy = s.running + s.queued > 0
            Image(systemName: s.failing > 0 ? "xmark.octagon.fill" : busy ? "circle.fill" : "checkmark.seal.fill")
                .font(.title2)
                .foregroundStyle(s.failing > 0 ? .red : busy ? .orange : .green)
                .symbolEffect(.pulse, isActive: busy && s.failing == 0)
            VStack(alignment: .leading, spacing: 2) {
                Text([s.failing > 0 ? "\(s.failing) failing" : nil, s.running > 0 ? "\(s.running) running" : nil, s.queued > 0 ? "\(s.queued) queued" : nil]
                        .compactMap { $0 }.joined(separator: " · ").nonEmpty ?? "All checks passed").font(.headline)
                Text("\(s.passed) of \(s.total) successful · \(String(model.sha.prefix(8)))").font(.caption).foregroundStyle(.secondary)
            }
            TextField("Filter checks", text: $filter).textFieldStyle(.roundedBorder).frame(maxWidth: 260)
            Spacer()
            if let e = model.error { Label(e, systemImage: "exclamationmark.triangle").foregroundStyle(.orange).lineLimit(1).font(.caption) }
            if let t = model.lastUpdated {
                TimelineView(.periodic(from: .now, by: 1)) { _ in
                    Text("Updated \(t, format: .relative(presentation: .numeric))").font(.caption).foregroundStyle(.tertiary)
                }
            }
            Button { Task { await model.refresh() } } label: { Image(systemName: "arrow.clockwise") }.help("Refresh now")
            Button("Done") { dismiss() }.keyboardShortcut(.cancelAction)
        }
        .padding(14)
    }
}

struct StateIcon: View {
    var state: CheckState
    var body: some View {
        switch state {
        case .failing: Image(systemName: "xmark.circle.fill").foregroundStyle(.red)
        case .running: Image(systemName: "circle.fill").foregroundStyle(.orange).symbolEffect(.pulse).help("Running")
        case .pending: Image(systemName: "circle.dashed").foregroundStyle(.secondary).help("Queued / waiting")
        case .success: Image(systemName: "checkmark.circle.fill").foregroundStyle(.green)
        case .skipped: Image(systemName: "minus.circle").foregroundStyle(.tertiary)
        }
    }
}

struct CheckRow: View {
    var check: Check
    var body: some View {
        HStack(alignment: .center, spacing: 10) {
            StateIcon(state: check.state).frame(width: 16)
            VStack(alignment: .leading, spacing: 3) {
                let parts = check.status.context.components(separatedBy: " / ")
                HStack(spacing: 6) {
                    Text(parts.count > 1 ? parts.dropFirst().joined(separator: " / ") : check.status.context).fontWeight(.medium).lineLimit(1)
                    if parts.count > 1 { Text(parts[0]).foregroundStyle(.secondary).lineLimit(1) }
                }
                HStack(spacing: 10) {
                    if let d = check.status.description, !d.isEmpty { Text(d).lineLimit(1) }
                    if let j = check.job {
                        if let r = j.runs_on, !r.isEmpty { Label(r.joined(separator: ", "), systemImage: "server.rack").lineLimit(1) }
                        if let a = j.attempt, a > 1 { Label("attempt \(a)", systemImage: "arrow.counterclockwise") }
                        if let n = j.needs, !n.isEmpty { Label("needs \(n.joined(separator: ", "))", systemImage: "arrow.turn.down.right").lineLimit(1) }
                    } else if check.status.target_url != nil {
                        Label("external", systemImage: "arrow.up.right.square")
                    }
                    if let u = check.status.updated_at.flatMap(ISO8601DateFormatter().date(from:)) {
                        Text(u, format: .relative(presentation: .named))
                    }
                }
                .font(.caption).foregroundStyle(.secondary)
            }
            Spacer(minLength: 0)
            Image(systemName: "chevron.right").foregroundStyle(.tertiary)
        }
        .padding(.vertical, 4)
        // List aligns separators to a Text inside the row by default, which differs per row here.
        .alignmentGuide(.listRowSeparatorLeading) { _ in 0 }
        .help(check.status.context + (check.status.description.map { "\n" + $0 } ?? ""))
    }
}

struct CheckDetail: View {
    var check: Check
    var model: ChecksModel
    var baseURL: String
    var back: () -> Void
    @State private var log: JobLogModel?
    @State private var scrollTarget: NSRange?
    @State private var showProblems = true

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            VStack(alignment: .leading, spacing: 6) {
                HStack {
                    Button(action: back) { Label("Checks", systemImage: "chevron.left") }
                        .buttonStyle(.borderless).keyboardShortcut("[", modifiers: .command)
                    StateIcon(state: log.map { CheckState(jobStatus: $0.job.status) } ?? check.state)
                    Text(check.status.context).font(.headline).lineLimit(2).textSelection(.enabled)
                    Spacer()
                    if let log { Button("Copy Log") { NSPasteboard.general.clearContents(); NSPasteboard.general.setString(log.storage.string, forType: .string) } }
                    if let url = webURL { Button("Open in Forgejo") { NSWorkspace.shared.open(url) } }
                }
                if let d = check.status.description, !d.isEmpty { Text(d).font(.callout).foregroundStyle(.secondary).textSelection(.enabled) }
                if let j = log?.job ?? check.job {
                    HStack(spacing: 10) {
                        Text("Job “\(j.name)”").font(.caption)
                        if let r = j.runs_on, !r.isEmpty { Label(r.joined(separator: ", "), systemImage: "server.rack").font(.caption) }
                        if let a = j.attempt, a > 1 { Text("attempt \(a)").font(.caption) }
                        if let d = Self.duration(j.steps?.first?.started, j.steps?.last?.stopped) { Label(d, systemImage: "timer").font(.caption) }
                    }
                    .foregroundStyle(.secondary)
                }
            }
            .padding(12)
            Divider()

            if let log {
                if let steps = log.job.steps, !steps.isEmpty { StepsStrip(steps: steps) ; Divider() }
                if !log.problems.isEmpty {
                    DisclosureGroup(isExpanded: $showProblems) {
                        ScrollView {
                            LazyVStack(alignment: .leading, spacing: 2) {
                                ForEach(Array(log.problems.prefix(200).enumerated()), id: \.offset) { _, p in
                                    Button { scrollTarget = p.range } label: {
                                        Text(p.line).font(.system(size: 11, design: .monospaced)).foregroundStyle(.red).lineLimit(2)
                                            .frame(maxWidth: .infinity, alignment: .leading)
                                    }
                                    .buttonStyle(.plain)
                                }
                            }
                        }
                        .frame(maxHeight: 140)
                    } label: {
                        Label("\(String(log.problems.count)) problem line(s) — click to jump", systemImage: "exclamationmark.triangle.fill").foregroundStyle(.red).font(.callout)
                    }
                    .padding(.horizontal, 12).padding(.vertical, 6)
                    Divider()
                }
                if log.truncatedBytes > 0 {
                    HStack {
                        Text("Showing the last \(ByteCountFormatter.string(fromByteCount: Int64(JobLogModel.initialTail), countStyle: .file)) of the log.").font(.caption).foregroundStyle(.secondary)
                        Button("Load Full Log") { Task { await log.loadEarlier() } }.controlSize(.small)
                        Spacer()
                    }
                    .padding(.horizontal, 12).padding(.vertical, 4)
                }
                LogTextView(model: log, version: log.version, scrollTarget: $scrollTarget)
                if let e = log.error { Text(e).font(.caption).foregroundStyle(.red).padding(6) }
            } else if check.job == nil {
                ContentUnavailableView {
                    Label("External Status", systemImage: "arrow.up.right.square")
                } description: {
                    Text("Reported by a service outside Forgejo Actions, so there's no log to show here.")
                } actions: {
                    if let url = webURL { Button("Open Details") { NSWorkspace.shared.open(url) } }
                }
            }
        }
        .task(id: check.job?.id) {
            guard let j = check.job else { log = nil; return }
            let m = JobLogModel(client: model.client, repo: model.repo, job: j)
            log = m
            showProblems = CheckState(jobStatus: j.status) == .failing
            await m.poll()
        }
    }

    private var webURL: URL? {
        guard let t = check.status.target_url else { return nil }
        return URL(string: t.hasPrefix("http") ? t : baseURL + t)
    }

    static func duration(_ a: String?, _ b: String?) -> String? {
        let f = ISO8601DateFormatter()
        guard let a, let s = f.date(from: a) else { return nil }
        let e = b.flatMap(f.date(from:)) ?? Date()
        let secs = Int(e.timeIntervalSince(s))
        guard secs >= 0, secs < 86_400 * 3 else { return nil }
        return secs >= 60 ? "\(secs / 60)m \(secs % 60)s" : "\(secs)s"
    }
}

struct StepsStrip: View {
    var steps: [ActionStep]
    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 3) {
                ForEach(steps, id: \.number) { s in
                    HStack(spacing: 8) {
                        StateIcon(state: CheckState(jobStatus: s.status)).frame(width: 14)
                        Text(s.name).font(.callout).lineLimit(1).foregroundStyle(s.status == "skipped" ? .tertiary : .primary)
                        Spacer()
                        if let d = CheckDetail.duration(s.started, s.stopped), s.status != "skipped" {
                            Text(d).font(.caption.monospacedDigit()).foregroundStyle(.secondary)
                        }
                    }
                }
            }
            .padding(.horizontal, 12).padding(.vertical, 8)
        }
        .frame(maxHeight: 150)
        .fixedSize(horizontal: false, vertical: true)
    }
}


extension String {
    var nonEmpty: String? { isEmpty ? nil : self }
}

/// Centered, window-relative card for the checks UI. A macOS sheet hangs off the top edge and can't be
/// taller than its content allows, so this overlays the whole window instead.
struct ChecksOverlay: View {
    var model: ChecksModel
    var baseURL: String
    var close: () -> Void

    var body: some View {
        GeometryReader { g in
            ZStack {
                Color.black.opacity(0.35).ignoresSafeArea().onTapGesture(perform: close)
                ChecksSheet(model: model, baseURL: baseURL, dismiss: close)
                    .frame(width: min(g.size.width - 64, 1500), height: g.size.height - 64)
                    .background(.windowBackground, in: RoundedRectangle(cornerRadius: 14))
                    .clipShape(RoundedRectangle(cornerRadius: 14))
                    .overlay(RoundedRectangle(cornerRadius: 14).strokeBorder(.separator))
                    .shadow(color: .black.opacity(0.35), radius: 30, y: 10)
            }
            .frame(width: g.size.width, height: g.size.height)
        }
        .transition(.opacity)
    }
}
