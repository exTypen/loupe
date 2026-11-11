import Foundation

// CI across forges. Forgejo's Actions API has its own shapes (ActionRun / ActionJob in Forgejo.swift); GitHub's and
// Gitea's (1.25+) are the same as each other, so they're decoded once here and mapped onto Forgejo's models.

/// What a server's Actions API can do, decided once per server from GET /version.
struct CIFeatures: Equatable, Sendable {
    enum Forge: Equatable, Sendable { case github, forgejo, gitea }
    var forge: Forge
    var version: String?
    var actions = true             // runs, jobs and logs
    var rerunRun = false
    var rerunFailed = false
    var rerunJob = false
    var cancel = false

    /// GitHub and Gitea answer with GitHub's run/job shapes; Forgejo with its own.
    var githubShaped: Bool { forge != .forgejo }
    var name: String { switch forge { case .github: "GitHub"; case .forgejo: "Forgejo"; case .gitea: "Gitea" } }

    /// Forgejo reports "17.0.0-100-2254cb0+gitea-1.22.0"; Gitea "1.26.0" (or "1.27.0+dev-…").
    /// Endpoints as each release's swagger lists them:
    /// Forgejo: cancel 16+, rerun run/job 17+, no rerun-failed. Gitea: Actions API 1.25+, rerun (all/failed/job) 1.26+,
    /// cancel after 1.26. An unreadable version keeps today's behaviour: try Actions, offer nothing that writes.
    static func detect(github: Bool, version: String?) -> CIFeatures {
        if github { return CIFeatures(forge: .github, version: nil, rerunRun: true, rerunFailed: true, rerunJob: true, cancel: true) }
        guard let version, let v = numbers(version) else { return CIFeatures(forge: .forgejo, version: version) }
        // Gitea has always been 1.x; Forgejo started at 7 and newer releases drop the "+gitea-" suffix, so a plain
        // "17.0.0" is Forgejo too (taking it for Gitea decoded Forgejo's runs as GitHub's and failed every refresh).
        if v.major >= 2 || version.contains("+gitea-") || version.localizedCaseInsensitiveContains("forgejo") {
            return CIFeatures(forge: .forgejo, version: version, rerunRun: v.major >= 17, rerunJob: v.major >= 17, cancel: v.major >= 16)
        }
        let at = { (major: Int, minor: Int) in v.major > major || v.major == major && v.minor >= minor }
        return CIFeatures(forge: .gitea, version: version, actions: at(1, 25),
                          rerunRun: at(1, 26), rerunFailed: at(1, 26), rerunJob: at(1, 26), cancel: at(1, 27))
    }

    private static func numbers(_ s: String) -> (major: Int, minor: Int)? {
        let parts = s.split(whereSeparator: { !$0.isNumber }).prefix(2).compactMap { Int($0) }
        guard let major = parts.first else { return nil }
        return (major, parts.count > 1 ? parts[1] : 0)
    }

    enum Action: String, CaseIterable, Identifiable {
        case rerunJob = "Re-run Job", rerunFailed = "Re-run Failed Jobs", rerunAll = "Re-run All Jobs", cancel = "Cancel Run"
        var id: String { rawValue }
        var icon: String { self == .cancel ? "stop.circle" : self == .rerunJob ? "arrow.clockwise" : "arrow.clockwise.circle" }
    }

    /// The buttons a job's detail header offers. Any rerun, a job's too, needs the whole run to have finished (GitHub,
    /// Forgejo and Gitea all refuse one while the run is moving); cancel needs it moving.
    func actions(runStatus: String?, jobStatus: String) -> [Action] {
        guard actions else { return [] }
        let job = CheckState(jobStatus: jobStatus), run = runStatus.map { CheckState(jobStatus: $0) } ?? job
        var out: [Action] = []
        if rerunJob, job.isDone, run.isDone { out.append(.rerunJob) }
        if rerunFailed, run == .failing || (runStatus ?? jobStatus) == "cancelled" { out.append(.rerunFailed) }   // GitHub reruns cancelled jobs too
        if rerunRun, run.isDone { out.append(.rerunAll) }
        if cancel, !run.isDone { out.append(.cancel) }
        return out
    }

    func path(_ a: Action, _ r: RemoteRepo, runID: Int, jobID: Int) -> String {
        let base = "/repos/\(r.owner)/\(r.name)/actions"
        switch a {
        case .rerunJob: return forge == .gitea ? "\(base)/runs/\(runID)/jobs/\(jobID)/rerun" : "\(base)/jobs/\(jobID)/rerun"
        case .rerunFailed: return "\(base)/runs/\(runID)/rerun-failed-jobs"
        case .rerunAll: return "\(base)/runs/\(runID)/rerun"
        case .cancel: return "\(base)/runs/\(runID)/cancel"
        }
    }

    /// A refused write explained in terms of what to change, rather than the bare status line.
    func explain(_ error: Error) -> String {
        let m = error.localizedDescription
        // GitHub answers 404 rather than 403 when a token can't see Actions at all.
        if m.hasPrefix("GitHub 403") || m.hasPrefix("GitHub 404") {
            return "GitHub refused this (\(m.dropFirst(7).prefix(3))): the token needs write access to Actions — “Actions: Read and write” for a fine-grained token, the repo and workflow scopes for a classic one."
        }
        if m.contains(" 403:") { return "\(name) refused this (403): your account needs write access to this repository." }
        return m
    }
}

// MARK: - GitHub-shaped runs and jobs (GitHub, Gitea 1.25+)

struct GHRun: Decodable {
    var id: Int
    var run_number: Int?
    var run_attempt: Int?
    var name: String?
    var display_title: String?
    var path: String?
    var status: String?
    var conclusion: String?
    var event: String?
    var html_url: String?

    var forgejo: ActionRun {
        ActionRun(id: id, index_in_repo: run_number ?? id, title: display_title ?? name, workflow_id: path,
                  status: GHRun.status(status, conclusion), event: event, html_url: html_url)
    }

    /// GitHub's status + conclusion → Forgejo's single status (success | failure | cancelled | skipped | waiting | running).
    static func status(_ status: String?, _ conclusion: String?) -> String {
        switch status ?? "" {
        case "completed":
            switch conclusion ?? "" {
            case "success", "neutral": return "success"
            case "skipped", "stale": return "skipped"
            case "cancelled": return "cancelled"
            default: return "failure"      // failure, timed_out, action_required, startup_failure
            }
        case "in_progress": return "running"
        case "success", "failure", "cancelled", "skipped", "running", "waiting", "blocked": return status!   // already Forgejo-style
        default: return "waiting"          // queued, waiting, requested, pending
        }
    }
}

struct GHStep: Decodable {
    var number: Int
    var name: String
    var status: String?
    var conclusion: String?
    var started_at: String?
    var completed_at: String?
}

struct GHJob: Decodable {
    var id: Int
    var run_id: Int
    var run_attempt: Int?
    var name: String
    var html_url: String?
    var labels: [String]?
    var status: String?
    var conclusion: String?
    var started_at: String?
    var completed_at: String?
    var steps: [GHStep]?

    var forgejo: ActionJob {
        ActionJob(id: id, run_id: run_id, attempt: run_attempt, name: name, html_url: html_url, needs: nil,
                  runs_on: labels, task_id: nil, status: GHRun.status(status, conclusion),
                  steps: steps?.map { ActionStep(number: $0.number, name: $0.name, status: GHRun.status($0.status, $0.conclusion),
                                                 started: $0.started_at, stopped: $0.completed_at) })
    }
}

/// What a GitHub check run carries beyond its status row. `id` equals the Actions job id for check runs made by
/// GitHub Actions, which is how a row finds its job.
struct CheckRunRef: Codable, Hashable {
    var id: Int
    var app: String?          // "github-actions" for Actions jobs
    var summary: String?      // output.summary, Markdown
    var annotations: Int?     // output.annotations_count
}

struct CheckAnnotation: Decodable, Hashable {
    var path: String
    var start_line: Int?
    var annotation_level: String?   // notice | warning | failure
    var title: String?
    var message: String
}

/// URLSession forwards every header on a redirect. GitHub answers a log request with a 302 to a signed blob URL,
/// which must not receive the account token.
final class RedirectWithoutAuth: NSObject, URLSessionTaskDelegate, Sendable {
    static let shared = RedirectWithoutAuth()
    func urlSession(_ session: URLSession, task: URLSessionTask, willPerformHTTPRedirection response: HTTPURLResponse, newRequest request: URLRequest) async -> URLRequest? {
        var r = request
        if r.url?.host != task.originalRequest?.url?.host { r.setValue(nil, forHTTPHeaderField: "Authorization") }
        return r
    }
}

extension ForgejoClient {
    func serverVersion() async -> String? {
        struct V: Decodable { var version: String }
        return (try? await get("/version") as V)?.version
    }

    private var pageSize: [String: String] { [github ? "per_page" : "limit": "100"] }

    func ciRuns(_ r: RemoteRepo, headSHA: String, features f: CIFeatures) async throws -> [ActionRun] {
        guard f.githubShaped else { return try await actionRuns(r, headSHA: headSHA) }
        struct Page: Decodable { var workflow_runs: [GHRun]? }
        let p: Page = try await get("/repos/\(r.owner)/\(r.name)/actions/runs", query: pageSize.merging(["head_sha": headSHA]) { a, _ in a })
        return (p.workflow_runs ?? []).map(\.forgejo)
    }

    func ciJobs(_ r: RemoteRepo, runID: Int, features f: CIFeatures) async throws -> [ActionJob] {
        guard f.githubShaped else { return try await actionJobs(r, runID: runID) }
        struct Page: Decodable { var jobs: [GHJob]? }
        let p: Page = try await get("/repos/\(r.owner)/\(r.name)/actions/runs/\(runID)/jobs", query: pageSize)
        return (p.jobs ?? []).map(\.forgejo)
    }

    func ciJob(_ r: RemoteRepo, id: Int, features f: CIFeatures) async throws -> ActionJob {
        guard f.githubShaped else { return try await actionJob(r, id: id) }
        let j: GHJob = try await get("/repos/\(r.owner)/\(r.name)/actions/jobs/\(id)")
        return j.forgejo
    }

    func ciPerform(_ a: CIFeatures.Action, _ r: RemoteRepo, job: ActionJob, features f: CIFeatures) async throws {
        _ = try await send(try request("POST", f.path(a, r, runID: job.run_id, jobID: job.id)))
    }

    func checkRunAnnotations(_ r: RemoteRepo, id: Int) async throws -> [CheckAnnotation] {
        try await get("/repos/\(r.owner)/\(r.name)/check-runs/\(id)/annotations", query: ["per_page": "100"])
    }
}
