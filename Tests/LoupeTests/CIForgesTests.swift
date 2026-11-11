import Testing
import Foundation
import AppKit
@testable import Loupe

@Test func detectsCIFeaturesFromVersion() {
    let gh = CIFeatures.detect(github: true, version: nil)
    #expect(gh.forge == .github && gh.githubShaped && gh.rerunRun && gh.rerunFailed && gh.rerunJob && gh.cancel)

    let fj17 = CIFeatures.detect(github: false, version: "17.0.0-100-2254cb0+gitea-1.22.0")
    #expect(fj17.forge == .forgejo && !fj17.githubShaped && fj17.actions)
    #expect(fj17.rerunRun && fj17.rerunJob && fj17.cancel && !fj17.rerunFailed)
    let fj16 = CIFeatures.detect(github: false, version: "16.0.2+gitea-1.22.0")
    #expect(fj16.cancel && !fj16.rerunRun && !fj16.rerunJob)
    let fj11 = CIFeatures.detect(github: false, version: "11.0.16+gitea-1.22.0")
    #expect(fj11.actions && !fj11.cancel && !fj11.rerunRun)

    let gt124 = CIFeatures.detect(github: false, version: "1.24.6")
    #expect(gt124.forge == .gitea && !gt124.actions && gt124.actions(runStatus: "failure", jobStatus: "failure").isEmpty)
    let gt125 = CIFeatures.detect(github: false, version: "1.25.1")
    #expect(gt125.actions && gt125.githubShaped && !gt125.rerunRun)
    let gt126 = CIFeatures.detect(github: false, version: "1.26.0")
    #expect(gt126.rerunRun && gt126.rerunFailed && gt126.rerunJob && !gt126.cancel)
    #expect(CIFeatures.detect(github: false, version: "1.27.0+dev-160-g55186ca39b").cancel)

    // Version unreadable: keep Actions (as before), offer nothing that writes.
    let unknown = CIFeatures.detect(github: false, version: nil)
    #expect(unknown.forge == .forgejo && unknown.actions && unknown.actions(runStatus: "failure", jobStatus: "failure").isEmpty)
}

@Test func offersRerunAndCancelByState() {
    let gh = CIFeatures.detect(github: true, version: nil)
    #expect(gh.actions(runStatus: "failure", jobStatus: "failure") == [.rerunJob, .rerunFailed, .rerunAll])
    #expect(gh.actions(runStatus: "success", jobStatus: "success") == [.rerunJob, .rerunAll])
    #expect(gh.actions(runStatus: "running", jobStatus: "success") == [.rerunJob, .cancel])
    #expect(gh.actions(runStatus: "running", jobStatus: "running") == [.cancel])
    let fj = CIFeatures.detect(github: false, version: "17.0.0+gitea-1.22.0")
    #expect(fj.actions(runStatus: "failure", jobStatus: "failure") == [.rerunJob, .rerunAll])

    let r = RemoteRepo(remoteURL: "https://git.example.com/acme/web.git")!
    #expect(fj.path(.rerunJob, r, runID: 7, jobID: 42) == "/repos/acme/web/actions/jobs/42/rerun")
    #expect(fj.path(.cancel, r, runID: 7, jobID: 42) == "/repos/acme/web/actions/runs/7/cancel")
    let gt = CIFeatures.detect(github: false, version: "1.26.0")
    #expect(gt.path(.rerunJob, r, runID: 7, jobID: 42) == "/repos/acme/web/actions/runs/7/jobs/42/rerun")
    #expect(gh.path(.rerunFailed, r, runID: 7, jobID: 42) == "/repos/acme/web/actions/runs/7/rerun-failed-jobs")
    #expect(gh.path(.rerunAll, r, runID: 7, jobID: 42) == "/repos/acme/web/actions/runs/7/rerun")

    #expect(gh.explain(GitError(message: "GitHub 403: Resource not accessible by integration")).contains("Actions: Read and write"))
    #expect(fj.explain(GitError(message: "Forgejo 403: user should have write permission")).contains("write access"))
}

@Test func mapsGitHubRunsAndJobs() throws {
    let json = """
    {"total_count":1,"jobs":[{"id":555,"run_id":99,"run_attempt":2,"name":"build (macos)","html_url":"https://github.com/acme/web/actions/runs/99/job/555",
      "labels":["macos-15"],"status":"in_progress","conclusion":null,"started_at":"2026-11-01T10:00:00Z","completed_at":null,
      "steps":[{"number":1,"name":"Set up job","status":"completed","conclusion":"success","started_at":"2026-11-01T10:00:00Z","completed_at":"2026-11-01T10:00:02Z"},
               {"number":2,"name":"Test","status":"in_progress","conclusion":null,"started_at":"2026-11-01T10:00:02Z","completed_at":null},
               {"number":3,"name":"Upload","status":"queued","conclusion":null}]}]}
    """
    struct Page: Decodable { var jobs: [GHJob] }
    let job = try JSONDecoder().decode(Page.self, from: Data(json.utf8)).jobs[0].forgejo
    #expect(job.id == 555 && job.run_id == 99 && job.attempt == 2 && job.runs_on == ["macos-15"] && job.status == "running")
    #expect(job.steps?.map(\.status) == ["success", "running", "waiting"])
    #expect(job.steps?.first?.stopped == "2026-11-01T10:00:02Z")

    #expect(GHRun.status("completed", "timed_out") == "failure")
    #expect(GHRun.status("completed", "neutral") == "success")
    #expect(GHRun.status("completed", "cancelled") == "cancelled")
    #expect(GHRun.status("queued", nil) == "waiting")
    #expect(GHRun.status("success", nil) == "success")   // a Gitea build that already speaks Forgejo's statuses

    let run = GHRun(id: 99, run_number: 12, display_title: "Fix login", status: "completed", conclusion: "failure").forgejo
    #expect(run.id == 99 && run.index_in_repo == 12 && run.title == "Fix login" && run.status == "failure")
}

@Test func matchesChecksToJobs() {
    let job = ActionJob(id: 555, run_id: 99, attempt: 1, name: "build", status: "failure")
    let other = ActionJob(id: 556, run_id: 99, attempt: 1, name: "lint", status: "success")
    let runs = [ActionRun(id: 99, index_in_repo: 12)]
    let jobs = [99: [job, other]]

    // GitHub: an Actions check run's id is its job's id; a third-party check run matches nothing.
    let actions = CommitStatus(id: 555, status: "failure", context: "build", checkRun: CheckRunRef(id: 555, app: "github-actions"))
    #expect(ChecksModel.job(for: actions, runs: runs, jobs: jobs)?.id == 555)
    let thirdParty = CommitStatus(id: 9001, status: "success", context: "codecov/patch", target_url: "https://codecov.io/x", checkRun: CheckRunRef(id: 9001, app: "codecov"))
    #expect(ChecksModel.job(for: thirdParty, runs: runs, jobs: jobs) == nil)

    // Forgejo: run index and job number from the status's target_url, the job number from the job's html_url.
    var a = job; a.html_url = "https://git.example.com/acme/web/actions/runs/12/jobs/0/attempt/1"
    var b = other; b.html_url = "https://git.example.com/acme/web/actions/runs/12/jobs/1/attempt/1"
    let byURL = CommitStatus(id: 1, status: "success", context: "CI / lint (push)", target_url: "/acme/web/actions/runs/12/jobs/1")
    #expect(ChecksModel.job(for: byURL, runs: runs, jobs: [99: [a, b]])?.id == 556)

    // Gitea linking the run by id, with job URLs that carry no usable number: the job name in the context decides.
    let byName = CommitStatus(id: 2, status: "failure", context: "CI / build (pull_request)", target_url: "https://gitea.example.com/acme/web/actions/runs/99/jobs/5")
    #expect(ChecksModel.job(for: byName, runs: runs, jobs: jobs)?.id == 555)
    let unrelated = CommitStatus(id: 3, status: "success", context: "deploy", target_url: "https://ci.example.com/build/3")
    #expect(ChecksModel.job(for: unrelated, runs: runs, jobs: jobs) == nil)
}

@MainActor @Test func formatsGitHubLogsWithFoldableGroups() {
    let raw = "\u{FEFF}2026-11-01T10:00:00.1234567Z ##[group]Run actions/checkout@v4\n"
        + "2026-11-01T10:00:00.2Z with:\n"
        + "2026-11-01T10:00:00.3Z   repository: acme/web\n"
        + "2026-11-01T10:00:00.4Z ##[endgroup]\n"
        + "2026-11-01T10:00:01.0Z Syncing repository\n"
        + "2026-11-01T10:00:02.0Z ##[warning]Node.js 16 actions are deprecated.\n"
        + "2026-11-01T10:00:03.0Z ##[error]Process completed with exit code 1.\r\n"
    let out = JobLogModel.format(raw)
    #expect(out.string == "▸ Run actions/checkout@v4\nwith:\n  repository: acme/web\nSyncing repository\nNode.js 16 actions are deprecated.\nProcess completed with exit code 1.\n")
    // The header links to its group, the body carries it, what follows the group doesn't.
    let ns = out.string as NSString
    #expect((out.attribute(.link, at: 0, effectiveRange: nil) as? URL).flatMap(JobLogModel.foldID) == 0)
    #expect(out.attribute(.logGroup, at: ns.range(of: "with:").location, effectiveRange: nil) as? Int == 0)
    #expect(out.attribute(.logGroup, at: ns.range(of: "  repository").location, effectiveRange: nil) as? Int == 0)
    #expect(out.attribute(.logGroup, at: ns.range(of: "Syncing").location, effectiveRange: nil) == nil)

    let problems = JobLogModel.findProblems(out, base: 0)
    #expect(problems.map(\.line) == ["Node.js 16 actions are deprecated.", "Process completed with exit code 1."])
    #expect(problems.map(\.warning) == [true, false])

    // A group opened in one chunk carries on into the next; the next group gets a new id.
    var fold = JobLogModel.Fold()
    _ = JobLogModel.format("2026-11-01T10:00:00Z ##[group]Install\n", fold: &fold)
    #expect(fold.open == 0 && fold.next == 1)
    let second = JobLogModel.format("2026-11-01T10:00:01Z added 12 packages\n2026-11-01T10:00:02Z ##[endgroup]\n2026-11-01T10:00:03Z ##[group]Test\n", fold: &fold)
    #expect(second.attribute(.logGroup, at: 0, effectiveRange: nil) as? Int == 0)
    #expect(fold.open == 1 && fold.next == 2)
}

@Test func keepsOnlyNewBytesWhenRangeIsIgnored() {
    let full = Data("line 1\nline 2\nline 3\n".utf8)
    #expect(JobLogModel.newBytes(full, start: 0, offset: 7) == Data("line 2\nline 3\n".utf8))     // 200 with the whole log
    #expect(JobLogModel.newBytes(Data("line 3\n".utf8), start: 14, offset: 14) == Data("line 3\n".utf8))   // 206
    #expect(JobLogModel.newBytes(full, start: 0, offset: 0) == full)
    #expect(JobLogModel.newBytes(full, start: 0, offset: full.count).isEmpty)
}

/// Folded group bodies take no height; clicking the header opens them, and a jump to a line inside opens its group.
@MainActor @Test func foldsLogGroupsInTheTextView() {
    let model = JobLogModel(client: ForgejoClient(baseURL: "https://git.example.com", token: ""), repo: RemoteRepo(remoteURL: "https://git.example.com/a/b")!,
                            job: ActionJob(id: 1, run_id: 1, name: "j", status: "success"), features: CIFeatures.detect(github: true, version: nil))
    let body = (1...40).map { "input \($0)\n" }.joined()
    model.storage.append(JobLogModel.format("##[group]Run step\n" + body + "##[endgroup]\nafter\n"))
    let coordinator = LogTextView.Coordinator()
    coordinator.model = model
    let tv = NSTextView(frame: NSRect(x: 0, y: 0, width: 600, height: 400))
    tv.layoutManager!.delegate = coordinator
    tv.layoutManager!.replaceTextStorage(model.storage)
    func height() -> CGFloat {
        tv.layoutManager!.ensureLayout(for: tv.textContainer!)
        return tv.layoutManager!.usedRect(for: tv.textContainer!).height
    }
    let folded = height()
    #expect(coordinator.textView(tv, clickedOnLink: URL(string: "loupe-fold:0")!, at: 0))
    #expect(model.storage.string.hasPrefix("▾ Run step"))
    let open = height()
    #expect(open > folded * 10)
    _ = coordinator.textView(tv, clickedOnLink: URL(string: "loupe-fold:0")!, at: 0)
    #expect(model.storage.string.hasPrefix("▸ Run step") && abs(height() - folded) < 1)

    coordinator.unfold(at: (model.storage.string as NSString).range(of: "input 30").location, in: tv)
    #expect(model.expanded == [0] && abs(height() - open) < 1)
}

// Read-only against api.github.com with the `gh` CLI's token. Opt-in: LOUPE_GH_LIVE=1 swift test --filter GitHubActionsLive
@Suite(.enabled(if: ProcessInfo.processInfo.environment["LOUPE_GH_LIVE"] != nil))
struct GitHubActionsLive {
    @Test func checkRunsJobsAndLog() async throws {
        let c = ForgejoClient(baseURL: "https://github.com", token: try #require(await AddAccountSheet.ghToken(host: "github.com")), github: true)
        let repo = RemoteRepo(remoteURL: "https://github.com/actions/checkout")!
        struct Runs: Decodable { struct R: Decodable { var head_sha: String }; var workflow_runs: [R] }
        let latest: Runs = try await c.get("/repos/actions/checkout/actions/runs", query: ["per_page": "1", "status": "completed"])
        let sha = try #require(latest.workflow_runs.first?.head_sha)
        let f = CIFeatures.detect(github: true, version: nil)

        let statuses = try await c.combinedStatus(repo, sha: sha)
        let runs = try await c.ciRuns(repo, headSHA: sha, features: f)
        var jobs: [Int: [ActionJob]] = [:]
        for r in runs { jobs[r.id] = try await c.ciJobs(repo, runID: r.id, features: f) }
        let matched = statuses.compactMap { ChecksModel.job(for: $0, runs: runs, jobs: jobs) }
        #expect(!matched.isEmpty)
        let job = try #require(matched.first { $0.status == "success" || $0.status == "failure" })
        #expect(try await c.ciJob(repo, id: job.id, features: f).steps?.isEmpty == false)

        // 302 to a blob host, fetched without the token.
        let (data, _, _) = try await c.actionLog(repo, jobID: job.id, range: nil)
        let text = JobLogModel.format(String(decoding: data, as: UTF8.self))
        #expect(text.string.contains("▸ "))
    }
}
