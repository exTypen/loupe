import SwiftUI
import AppKit
import Observation

/// One row in the checks list: a commit status, linked to its Forgejo Actions job when it has one.
struct Check: Identifiable, Hashable {
    var status: CommitStatus
    var job: ActionJob?
    var id: String { status.context }

    var state: CheckState {
        let s = CheckState(status: status.status, description: status.description)
        // A pending status says nothing of queued vs running (GitHub's in-progress check runs); the job does.
        if s == .pending, let job, CheckState(jobStatus: job.status) == .running { return .running }
        return s
    }
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
    var notice: String?                                 // why jobs and logs aren't shown, when they can't be
    var lastUpdated: Date?
    var features: CIFeatures?
    private var runs: [ActionRun] = []
    private var jobs: [Int: [ActionJob]] = [:]         // run id → jobs (latest attempt)
    private static var featureCache: [String: CIFeatures] = [:]   // per server URL: one entry per account

    init(client: ForgejoClient, repo: RemoteRepo, sha: String) {
        self.client = client; self.repo = repo; self.sha = sha
    }

    var summary: (failing: Int, running: Int, queued: Int, passed: Int, total: Int) {
        (checks.filter { $0.state == .failing }.count, checks.filter { $0.state == .running }.count,
         checks.filter { $0.state == .pending }.count, checks.filter { $0.state == .success }.count, checks.count)
    }
    var allDone: Bool { !checks.isEmpty && checks.allSatisfy { $0.state.isDone } }

    func run(id: Int) -> ActionRun? { runs.first { $0.id == id } }

    func refresh() async {
        loading = checks.isEmpty
        defer { loading = false }
        let f = await loadFeatures()
        do {
            async let statusesReq = client.combinedStatus(repo, sha: sha)
            async let runsReq = actionRuns(f)
            let statuses = try await statusesReq
            // Actions may be disabled on the server (404): commit statuses still stand on their own.
            do { runs = try await runsReq; notice = Self.notice(f, error: nil) }
            catch { runs = []; notice = Self.notice(f, error: error) }

            // Only re-fetch jobs for runs that can still change; finished runs are immutable (a rerun resets their status).
            let stale = runs.filter { jobs[$0.id] == nil || !["success", "failure", "cancelled", "skipped"].contains($0.status ?? "") }
            try await withThrowingTaskGroup(of: (Int, [ActionJob]).self) { g in
                for run in stale { g.addTask { (run.id, try await self.client.ciJobs(self.repo, runID: run.id, features: f)) } }
                for try await (id, js) in g { jobs[id] = js }
            }

            checks = statuses.map { Check(status: $0, job: Self.job(for: $0, runs: runs, jobs: jobs)) }
                .sorted { ($0.state, $0.status.context.lowercased()) < ($1.state, $1.status.context.lowercased()) }
            lastUpdated = Date()
            error = nil
        } catch { self.error = error.localizedDescription }
    }

    private func actionRuns(_ f: CIFeatures) async throws -> [ActionRun] {
        f.actions ? try await client.ciRuns(repo, headSHA: sha, features: f) : []
    }

    private func loadFeatures() async -> CIFeatures {
        if let features { return features }
        if let f = Self.featureCache[client.baseURL] { features = f; return f }
        let version = client.github ? nil : await client.serverVersion()
        let f = CIFeatures.detect(github: client.github, version: version)
        if client.github || version != nil { Self.featureCache[client.baseURL] = f }   // a failed lookup is retried next time
        features = f
        return f
    }

    nonisolated static func notice(_ f: CIFeatures, error: Error?) -> String? {
        if !f.actions { return "\(f.name) \(f.version ?? "") has no Actions API (it arrived in 1.25), so checks show their status only." }
        guard let error, error.localizedDescription.contains(" 403") else { return nil }
        return f.forge == .github
            ? "This token can't read GitHub Actions (403), so checks show their status only. Jobs and logs need “Actions: Read”."
            : "\(f.name) won't list Actions runs for this account (403), so checks show their status only."
    }

    /// Runs a rerun or cancel; nil on success, otherwise what went wrong in words the user can act on.
    func perform(_ a: CIFeatures.Action, job: ActionJob) async -> String? {
        guard let f = features else { return nil }
        do {
            try await client.ciPerform(a, repo, job: job, features: f)
            jobs[job.run_id] = nil
            await refresh()
            return nil
        } catch { return f.explain(error) }
    }

    /// The job behind a status row.
    /// - GitHub: an Actions check run's id is its job's id.
    /// - Forgejo/Gitea: target_url "/acme/web/actions/runs/292970/jobs/4" → run index 292970 (or id, on servers that
    ///   link by id), job #4 of that run; failing that, the job whose name the context "workflow / job (event)" carries.
    nonisolated static func job(for s: CommitStatus, runs: [ActionRun], jobs: [Int: [ActionJob]]) -> ActionJob? {
        if let id = s.checkRun?.id { return jobs.values.lazy.flatMap { $0 }.first { $0.id == id } }
        guard let url = s.target_url, let r = url.range(of: "/actions/runs/") else { return nil }
        let parts = url[r.upperBound...].split(separator: "/")
        guard let n = Int(parts.first ?? ""),
              let run = runs.first(where: { $0.index_in_repo == n }) ?? runs.first(where: { $0.id == n }),
              let list = jobs[run.id] else { return nil }
        let jobNo = parts.count >= 3 && parts[1] == "jobs" ? Int(parts[2]) : nil
        // Job html_url carries ".../runs/<index>/jobs/<n>/attempt/<a>"; keep the newest attempt per n.
        var byNo: [Int: ActionJob] = [:]
        for j in list {
            guard let u = j.html_url, let jr = u.range(of: "/jobs/") else { continue }
            let n = Int(u[jr.upperBound...].split(separator: "/").first ?? "") ?? -1
            if (byNo[n]?.attempt ?? 0) <= (j.attempt ?? 0) { byNo[n] = j }
        }
        if let jobNo, let j = byNo[jobNo] { return j }
        if jobNo == nil, byNo.count == 1 { return byNo.values.first }
        let named = list.filter { s.context == $0.name || s.context.contains(" / \($0.name)") }
        return named.max { ($0.name.count, $0.attempt ?? 0) < ($1.name.count, $1.attempt ?? 0) }
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

struct LogProblem: Hashable { var line: String; var range: NSRange; var warning = false }

extension NSAttributedString.Key {
    static let logGroup = NSAttributedString.Key("loupe.logGroup")   // Int: the ##[group] a body line belongs to
}

@MainActor @Observable
final class JobLogModel {
    let client: ForgejoClient
    let repo: RemoteRepo
    let features: CIFeatures
    var job: ActionJob
    var problems: [LogProblem] = []
    var truncatedBytes = 0                // bytes before what we show (we start from the tail of huge logs)
    var error: String?
    var waitingForLog = false             // the server has no log for this job yet (GitHub serves it once the job ends)
    @ObservationIgnored let storage = NSTextStorage()
    @ObservationIgnored var expanded: Set<Int> = []    // ##[group]s the user opened; all start folded, as on the web
    @ObservationIgnored private var offset = 0
    @ObservationIgnored private var pending = Data()   // bytes after the last newline (a chunk can split a line or a UTF-8 char)
    @ObservationIgnored private var fold = Fold()      // a group can open in one chunk and close in the next
    var version = 0                                      // bumps on every append so the view can follow the tail

    static let initialTail = 16 << 20   // detail page fetches the whole log; only beyond 16 MB start from the tail

    init(client: ForgejoClient, repo: RemoteRepo, job: ActionJob, features: CIFeatures) {
        self.client = client; self.repo = repo; self.job = job; self.features = features
    }

    var isRunning: Bool { !CheckState(jobStatus: job.status).isDone }

    func poll() async {
        var loaded = await loadInitial()
        var graceAfterEnd = 6        // GitHub publishes a finished job's log a few seconds after the job ends
        while !Task.isCancelled {
            if let j = try? await client.ciJob(repo, id: job.id, features: features) { job = j }
            let running = isRunning
            if !loaded { loaded = await loadInitial() }
            else if let (data, start, total) = try? await client.actionLog(repo, jobID: job.id, range: "bytes=\(offset)-"), !data.isEmpty {
                let fresh = Self.newBytes(data, start: start, offset: offset)
                offset = total ?? offset + fresh.count
                await append(fresh)
            }
            if !running {
                if loaded { await flushPending(); return }      // finished: one last read above, then stop
                graceAfterEnd -= 1
                if graceAfterEnd < 0 { waitingForLog = false; error = error ?? "\(features.name) has no log for this job."; return }
            }
            try? await Task.sleep(for: .seconds(loaded ? 2 : 5))
        }
    }

    /// The whole log, or its last 16 MB on Forgejo (GitHub's blob store and Gitea don't promise suffix ranges).
    private func loadInitial() async -> Bool {
        do {
            let (data, start, total) = try await client.actionLog(repo, jobID: job.id, range: features.githubShaped ? nil : "bytes=-\(Self.initialTail)")
            truncatedBytes = start
            offset = total ?? data.count
            waitingForLog = false
            error = nil
            await append(data)
            return true
        } catch {
            if isRunning { waitingForLog = true } else { self.error = error.localizedDescription }
            return false
        }
    }

    /// A tail request ("bytes=<offset>-") answered by a server that ignores Range comes back whole, from byte 0:
    /// keep only what's past the offset.
    nonisolated static func newBytes(_ data: Data, start: Int, offset: Int) -> Data {
        guard start == 0, offset > 0 else { return data }
        return data.count > offset ? Data(data.dropFirst(offset)) : Data()
    }

    func loadEarlier() async {
        guard truncatedBytes > 0 else { return }
        do {
            let (data, _, _) = try await client.actionLog(repo, jobID: job.id, range: "bytes=0-\(truncatedBytes - 1)")
            var f = Fold(next: fold.next + 1_000_000)        // ids apart from the tail's
            let (text, found) = await Self.formatted(String(decoding: data, as: UTF8.self), fold: &f)
            storage.insert(text, at: 0)
            let shift = text.length
            problems = found + problems.map { LogProblem(line: $0.line, range: NSRange(location: $0.range.location + shift, length: $0.range.length), warning: $0.warning) }
            truncatedBytes = 0
            version += 1
        } catch { self.error = error.localizedDescription }
    }

    private func append(_ data: Data) async {
        pending.append(data)
        guard let nl = pending.lastIndex(of: UInt8(ascii: "\n")) else { return }
        let chunk = pending[...nl]
        pending = Data(pending[pending.index(after: nl)...])
        await emit(String(decoding: chunk, as: UTF8.self))
    }

    private func flushPending() async {
        guard !pending.isEmpty else { return }
        let s = String(decoding: pending, as: UTF8.self) + "\n"
        pending = Data()
        await emit(s)
    }

    private func emit(_ s: String) async {
        var f = fold
        let (text, found) = await Self.formatted(s, fold: &f)
        fold = f
        let base = storage.length   // read after the await: loadEarlier may have inserted meanwhile
        storage.append(text)
        problems += found.map { LogProblem(line: $0.line, range: NSRange(location: $0.range.location + base, length: $0.range.length), warning: $0.warning) }
        version += 1
    }

    /// Formatting a multi-MB log takes long enough to drop frames: off the main thread.
    private static func formatted(_ s: String, fold: inout Fold) async -> (NSAttributedString, [LogProblem]) {
        struct Handoff: @unchecked Sendable { var text: NSAttributedString; var problems: [LogProblem]; var fold: Fold }   // built there, only read here
        let start = fold
        let h = await Task.detached(priority: .userInitiated) {
            var f = start
            let text = format(s, fold: &f)
            return Handoff(text: text, problems: findProblems(text, base: 0), fold: f)
        }.value
        fold = h.fold
        return (h.text, h.problems)
    }

    nonisolated(unsafe) private static let mono = NSFont.monospacedSystemFont(ofSize: 11.5, weight: .regular)
    nonisolated(unsafe) private static let monoBold = NSFont.monospacedSystemFont(ofSize: 11.5, weight: .semibold)

    nonisolated static func isProblem(_ l: Substring) -> Bool {
        l.hasPrefix("##[error]") || l.contains("❌") || l.hasPrefix("error") || l.contains("error:") || l.contains("Error:")
            || l.contains("FAILED") || l.contains("panicked at") || l.hasPrefix("thread '") && l.contains("panicked")
    }

    /// The ##[group] open at the end of a chunk, and the next group id to hand out.
    struct Fold: Sendable { var open: Int?; var next = 0 }

    nonisolated static let foldScheme = "loupe-fold"
    nonisolated static func foldID(_ url: URL) -> Int? { url.scheme == foldScheme ? Int(url.absoluteString.dropFirst(foldScheme.count + 1)) : nil }

    nonisolated static func format(_ s: String) -> NSAttributedString { var f = Fold(); return format(s, fold: &f) }

    /// Strips runner timestamps and turns workflow commands into readable, coloured lines. A `##[group]` line becomes
    /// a clickable "▸ title" header (a `loupe-fold:<id>` link), and the lines up to `##[endgroup]` carry `.logGroup`
    /// so the view can fold them.
    nonisolated static func format(_ s: String, fold: inout Fold) -> NSAttributedString {
        let out = NSMutableAttributedString()
        // "\r\n" is one Character in Swift, so a split on "\n" would miss it.
        let s = s.contains("\r\n") ? s.replacingOccurrences(of: "\r\n", with: "\n") : s
        for raw in s.split(separator: "\n", omittingEmptySubsequences: false).dropLast(s.hasSuffix("\n") ? 1 : 0) {
            var l = raw
            if l.first == "\u{FEFF}" { l = l.dropFirst() }          // GitHub's logs start with a BOM
            // "2026-10-19T14:33:38.5145900Z " prefix
            if l.count > 20, l.dropFirst(4).first == "-", l.dropFirst(10).first == "T", let z = l.prefix(32).firstIndex(of: "Z") {
                l = l[l.index(after: z)...]
                if l.first == " " { l = l.dropFirst() }   // only the separator; runner indentation is meaningful
            }
            if l.hasPrefix("##[endgroup]") { fold.open = nil; continue }
            var color = NSColor.labelColor, font = mono
            var attrs: [NSAttributedString.Key: Any] = [:]
            if l.hasPrefix("##[group]") {
                let id = fold.next
                fold.next += 1
                fold.open = id
                l = "▸ " + l.dropFirst(9); color = .secondaryLabelColor; font = monoBold
                attrs[.link] = URL(string: "\(foldScheme):\(id)")
            } else {
                if let g = fold.open { attrs[.logGroup] = g }
                if l.hasPrefix("##[error]") { l = l.dropFirst(9); color = .systemRed; font = monoBold }
                else if l.hasPrefix("##[warning]") { l = l.dropFirst(11); color = .systemOrange }
                else if l.hasPrefix("##[notice]") { l = l.dropFirst(10); color = .controlAccentColor }
                else if l.hasPrefix("##[debug]") { l = l.dropFirst(9); color = .tertiaryLabelColor }
                else if l.hasPrefix("[command]") { l = l.dropFirst(9); color = .secondaryLabelColor }
                else if l.hasPrefix("⭐") { color = .controlAccentColor; font = monoBold }
                else if l.contains("✅") { color = .systemGreen }
                else if isProblem(l) { color = .systemRed }
            }
            attrs[.font] = font
            attrs[.foregroundColor] = color
            out.append(NSAttributedString(string: l + "\n", attributes: attrs))
        }
        return out
    }

    /// Error lines (red) and ##[warning] lines (orange), for the click-to-jump list. Group headers are skipped.
    nonisolated static func findProblems(_ text: NSAttributedString, base: Int) -> [LogProblem] {
        var found: [LogProblem] = []
        let ns = text.string as NSString
        ns.enumerateSubstrings(in: NSRange(location: 0, length: ns.length), options: .byLines) { sub, range, _, stop in
            guard let sub, text.attribute(.link, at: range.location, effectiveRange: nil) == nil else { return }
            let color = text.attribute(.foregroundColor, at: range.location, effectiveRange: nil) as? NSColor
            guard isProblem(Substring(sub)) || color == .systemRed || color == .systemOrange else { return }
            found.append(LogProblem(line: sub.trimmingCharacters(in: .whitespaces), range: NSRange(location: range.location + base, length: range.length),
                                    warning: color == .systemOrange))
            if found.count >= 200 { stop.pointee = true }
        }
        return found
    }
}

/// The job log. Folded ##[group] bodies get null glyphs and zero-width line breaks, so they take no space; a click on
/// a group's header link toggles it.
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
        context.coordinator.model = model
        tv.layoutManager?.delegate = context.coordinator      // before the text arrives, so folding applies from the start
        tv.layoutManager?.replaceTextStorage(model.storage)
        tv.isAutomaticLinkDetectionEnabled = false
        tv.linkTextAttributes = [.cursor: NSCursor.pointingHand]   // group headers keep their own colour
        tv.delegate = context.coordinator
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
            context.coordinator.unfold(at: target.location, in: tv)
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

    // @preconcurrency: NSLayoutManagerDelegate is nonisolated, but a text view lays out on the main thread (checked at runtime).
    @MainActor final class Coordinator: NSObject, NSTextViewDelegate, @preconcurrency NSLayoutManagerDelegate {
        var lastVersion = 0
        weak var model: JobLogModel?
        private var hiddenCache: (range: NSRange, hidden: Bool)?    // within one glyph pass, which asks char by char

        func textView(_ textView: NSTextView, clickedOnLink link: Any, at charIndex: Int) -> Bool {
            guard let url = link as? URL, let id = JobLogModel.foldID(url), let model else { return false }
            if model.expanded.contains(id) { model.expanded.remove(id) } else { model.expanded.insert(id) }
            refold(header: charIndex, in: textView)
            return true
        }

        /// Opens the group holding `location`, so a problem line inside a folded group can be shown.
        func unfold(at location: Int, in tv: NSTextView) {
            guard let model, location < model.storage.length,
                  let id = model.storage.attribute(.logGroup, at: location, effectiveRange: nil) as? Int, !model.expanded.contains(id) else { return }
            model.expanded.insert(id)
            var body = NSRange()
            _ = model.storage.attribute(.logGroup, at: location, longestEffectiveRange: &body, in: NSRange(location: 0, length: model.storage.length))
            guard body.location > 0 else { return }
            refold(header: body.location - 1, in: tv)
        }

        /// Flips the header's ▸/▾ and lays its body out again.
        private func refold(header charIndex: Int, in tv: NSTextView) {
            guard let model, let lm = tv.layoutManager else { return }
            let storage = model.storage
            let header = (storage.string as NSString).lineRange(for: NSRange(location: charIndex, length: 0))
            guard let url = storage.attribute(.link, at: header.location, effectiveRange: nil) as? URL, let id = JobLogModel.foldID(url) else { return }
            storage.replaceCharacters(in: NSRange(location: header.location, length: 1), with: model.expanded.contains(id) ? "▾" : "▸")
            let end = NSMaxRange(header)
            var body = NSRange(location: end, length: 0)
            if end < storage.length {
                var r = NSRange()
                if storage.attribute(.logGroup, at: end, longestEffectiveRange: &r, in: NSRange(location: end, length: storage.length - end)) as? Int == id { body = r }
            }
            let all = NSUnionRange(header, body)
            lm.invalidateGlyphs(forCharacterRange: all, changeInLength: 0, actualCharacterRange: nil)
            lm.invalidateLayout(forCharacterRange: all, actualCharacterRange: nil)
            tv.needsDisplay = true
        }

        private func isHidden(_ i: Int) -> Bool {
            if let c = hiddenCache, NSLocationInRange(i, c.range) { return c.hidden }
            guard let model, i < model.storage.length else { return false }
            var r = NSRange()
            let id = model.storage.attribute(.logGroup, at: i, effectiveRange: &r) as? Int
            let hidden = id.map { !model.expanded.contains($0) } ?? false
            hiddenCache = (r, hidden)
            return hidden
        }

        func layoutManager(_ lm: NSLayoutManager, shouldGenerateGlyphs glyphs: UnsafePointer<CGGlyph>, properties props: UnsafePointer<NSLayoutManager.GlyphProperty>,
                           characterIndexes charIndexes: UnsafePointer<Int>, font: NSFont, forGlyphRange range: NSRange) -> Int {
            hiddenCache = nil      // the storage may have grown or shifted since the last pass
            defer { hiddenCache = nil }
            var changed: [NSLayoutManager.GlyphProperty]?
            for i in 0..<range.length where isHidden(charIndexes[i]) {
                if changed == nil { changed = Array(UnsafeBufferPointer(start: props, count: range.length)) }
                changed![i] = .null
            }
            guard let changed else { return 0 }
            lm.setGlyphs(glyphs, properties: changed, characterIndexes: charIndexes, font: font, forGlyphRange: range)
            return range.length
        }

        func layoutManager(_ lm: NSLayoutManager, shouldUse action: NSLayoutManager.ControlCharacterAction,
                           forControlCharacterAt charIndex: Int) -> NSLayoutManager.ControlCharacterAction {
            hiddenCache = nil
            defer { hiddenCache = nil }
            return isHidden(charIndex) ? .zeroAdvancement : action
        }
    }
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
                    Text("checks").foregroundStyle(.secondary)
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
                    .id(c.id + "\(c.job?.id ?? 0).\(c.job?.attempt ?? 0)")   // a rerun is a new attempt (Forgejo keeps the job id)
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
            else if let n = model.notice { Label(n, systemImage: "info.circle").foregroundStyle(.secondary).lineLimit(2).font(.caption).help(n) }
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
    @State private var busy: CIFeatures.Action?
    @State private var actionError: String?
    @State private var annotations: [CheckAnnotation] = []

    private var forgeName: String { model.features?.name ?? "Forgejo" }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            VStack(alignment: .leading, spacing: 6) {
                HStack {
                    Button(action: back) { Label("Checks", systemImage: "chevron.left") }
                        .buttonStyle(.borderless).shortcut(.back)
                    StateIcon(state: log.map { CheckState(jobStatus: $0.job.status) } ?? check.state)
                    Text(check.status.context).font(.headline).lineLimit(2).textSelection(.enabled)
                    Spacer()
                    if let j = log?.job ?? check.job { actionButtons(j) }
                    if let log { Button("Copy Log") { NSPasteboard.general.clearContents(); NSPasteboard.general.setString(log.storage.string, forType: .string) } }
                    if let url = webURL { Button("Open in \(forgeName)") { NSWorkspace.shared.open(url) } }
                }
                if let d = check.status.description, !d.isEmpty { Text(d).font(.callout).foregroundStyle(.secondary).textSelection(.enabled) }
                if let e = actionError {
                    Label(e, systemImage: "exclamationmark.triangle.fill").font(.callout).foregroundStyle(.red).textSelection(.enabled)
                }
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
                if log.waitingForLog {
                    HStack(spacing: 8) {
                        ProgressView().controlSize(.small)
                        Text("\(forgeName) makes a job's log available once the job has finished; the steps above update meanwhile.")
                            .font(.callout).foregroundStyle(.secondary)
                    }
                    .padding(.horizontal, 12).padding(.vertical, 8)
                    Divider()
                }
                if !log.problems.isEmpty {
                    DisclosureGroup(isExpanded: $showProblems) {
                        ScrollView {
                            LazyVStack(alignment: .leading, spacing: 2) {
                                ForEach(Array(log.problems.prefix(200).enumerated()), id: \.offset) { _, p in
                                    Button { scrollTarget = p.range } label: {
                                        Text(p.line).font(.system(size: 11, design: .monospaced)).foregroundStyle(p.warning ? .orange : .red).lineLimit(2)
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
                if let run = check.status.checkRun, run.summary?.isEmpty == false || !annotations.isEmpty {
                    CheckRunOutput(summary: run.summary, annotations: annotations)
                } else {
                    ContentUnavailableView {
                        Label("External Status", systemImage: "arrow.up.right.square")
                    } description: {
                        Text("Reported by a service outside \(forgeName) Actions, so there's no log to show here.")
                    } actions: {
                        if let url = webURL { Button("Open Details") { NSWorkspace.shared.open(url) } }
                    }
                }
            }
        }
        .task(id: check.job?.id) {
            guard let j = check.job else { log = nil; return }
            let m = JobLogModel(client: model.client, repo: model.repo, job: j,
                                features: model.features ?? CIFeatures.detect(github: model.client.github, version: nil))
            log = m
            showProblems = CheckState(jobStatus: j.status) == .failing
            await m.poll()
        }
        .task(id: check.status.checkRun?.id) {
            // A third-party check run's annotations (an Actions job's are its log's ##[error] lines).
            guard check.job == nil, let run = check.status.checkRun, (run.annotations ?? 0) > 0 else { return }
            annotations = (try? await model.client.checkRunAnnotations(model.repo, id: run.id)) ?? []
        }
    }

    /// Re-run / cancel, as far as the server's API allows (see CIFeatures.detect).
    @ViewBuilder private func actionButtons(_ j: ActionJob) -> some View {
        let actions = model.features?.actions(runStatus: model.run(id: j.run_id)?.status, jobStatus: j.status) ?? []
        ForEach(actions) { a in
            Button {
                Task {
                    busy = a
                    actionError = await model.perform(a, job: j)
                    busy = nil
                }
            } label: {
                if busy == a { ProgressView().controlSize(.small) } else { Label(a.rawValue, systemImage: a.icon) }
            }
            .disabled(busy != nil)
            .help(a == .rerunJob ? "Run this job again (on Forgejo, with the jobs that depend on it)" : a.rawValue)
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

/// A third-party GitHub check run's own report: its output summary (Markdown) and annotations.
struct CheckRunOutput: View {
    var summary: String?
    var annotations: [CheckAnnotation]

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 12) {
                if let s = summary, !s.isEmpty { MarkdownText(text: s).textSelection(.enabled) }
                if !annotations.isEmpty {
                    Text("Annotations · \(String(annotations.count))").font(.headline)
                    ForEach(Array(annotations.enumerated()), id: \.offset) { _, a in
                        HStack(alignment: .firstTextBaseline, spacing: 8) {
                            Image(systemName: a.annotation_level == "failure" ? "xmark.circle.fill" : a.annotation_level == "warning" ? "exclamationmark.triangle.fill" : "info.circle")
                                .foregroundStyle(a.annotation_level == "failure" ? .red : a.annotation_level == "warning" ? .orange : .secondary)
                            VStack(alignment: .leading, spacing: 2) {
                                Text(a.path + (a.start_line.map { ":\($0)" } ?? "")).font(.system(size: 11, design: .monospaced)).foregroundStyle(.secondary)
                                if let t = a.title, !t.isEmpty { Text(t).fontWeight(.medium) }
                                Text(a.message).font(.callout)
                            }
                            .textSelection(.enabled)
                        }
                    }
                }
            }
            .padding(14)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
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
