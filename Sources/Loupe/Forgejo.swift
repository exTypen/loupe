import Foundation
import Security

// MARK: - Models (Forgejo / Gitea API v1; GitHub responses are mapped onto these)

struct FUser: Codable, Hashable { var login: String; var avatar_url: String? }
struct FBranchRef: Codable, Hashable { var ref: String; var sha: String; var label: String? }

struct PullRequest: Codable, Identifiable, Hashable {
    var id: Int
    var number: Int
    var title: String
    var body: String?
    var state: String
    var draft: Bool?
    var user: FUser
    var head: FBranchRef
    var base: FBranchRef
    var additions: Int?
    var deletions: Int?
    var changed_files: Int?
    var comments: Int?
    var mergeable: Bool?
    var merged: Bool?
    var merged_at: String?        // GitHub's list has no `merged`; this tells merged from closed
    var requested_reviewers: [FUser]?
    var html_url: String
    var updated_at: String
}

struct Review: Codable, Identifiable, Hashable {
    var id: Int
    var user: FUser?
    var state: String
    var body: String
    var commit_id: String?
    var stale: Bool?
    var dismissed: Bool?
    var submitted_at: String?
}

struct ReviewComment: Codable, Identifiable, Hashable {
    var id: Int
    var user: FUser?
    var path: String
    var body: String
    var position: Int          // line in the new file, 0 when commenting on the old side
    var original_position: Int // line in the old file
    var created_at: String?
    var commit_id: String?
}

struct IssueComment: Codable, Identifiable, Hashable {
    var id: Int
    var user: FUser?
    var body: String
    var created_at: String
}

struct CombinedStatus: Codable { var state: String?; var total_count: Int?; var statuses: [CommitStatus]? }

struct CommitStatus: Codable, Hashable, Identifiable {
    var id: Int
    var status: String          // pending | success | error | failure | warning | skipped
    var context: String
    var description: String?
    var target_url: String?
    var created_at: String?
    var updated_at: String?
}

struct ActionRunPage: Codable { var workflow_runs: [ActionRun]?; var total_count: Int? }

struct ActionRun: Codable, Hashable, Identifiable {
    var id: Int
    var index_in_repo: Int
    var title: String?
    var workflow_id: String?
    var status: String?
    var event: String?
    var html_url: String?
}

struct ActionStep: Codable, Hashable {
    var number: Int
    var name: String
    var status: String
    var started: String?
    var stopped: String?
}

struct ActionJob: Codable, Hashable, Identifiable {
    var id: Int
    var run_id: Int
    var attempt: Int?
    var name: String
    var html_url: String?
    var needs: [String]?
    var runs_on: [String]?
    var task_id: Int?
    var status: String          // success | failure | cancelled | skipped | waiting | running | blocked
    var steps: [ActionStep]?
}

struct DraftComment: Identifiable, Hashable, Codable {
    var id = UUID()
    var path: String
    var newLine: Int?
    var oldLine: Int?
    var body: String
}

// MARK: - Remote detection

struct RemoteRepo: Hashable {
    var host: String     // "git.example.com" or "localhost:3300"
    var owner: String
    var name: String

    /// Accepts https://host/o/r(.git), ssh://git@host:2222/o/r.git, git@host:o/r.git
    init?(remoteURL: String) {
        var s = remoteURL.trimmingCharacters(in: .whitespacesAndNewlines)
        if s.hasSuffix(".git") { s.removeLast(4) }
        var host: String, path: String
        if let u = URL(string: s), let h = u.host, u.scheme != nil {
            host = h
            if let p = u.port, u.scheme == "http" || u.scheme == "https" { host += ":\(p)" }
            path = u.path
        } else if let at = s.firstIndex(of: "@"), let colon = s[at...].firstIndex(of: ":") {
            host = String(s[s.index(after: at)..<colon]); path = String(s[s.index(after: colon)...])
        } else { return nil }
        let comps = path.split(separator: "/")
        guard comps.count >= 2 else { return nil }
        self.host = host; owner = String(comps[comps.count - 2]); name = String(comps[comps.count - 1])
    }
}

// MARK: - Account + Keychain

struct ForgejoAccount: Codable, Hashable, Identifiable {
    var baseURL: String   // "https://git.example.com", "https://github.com"
    var username: String
    var github: Bool? = nil   // nil in accounts saved before GitHub support → Forgejo
    var space: String? = nil  // nil → the default space (accounts saved before spaces)
    var isGitHub: Bool { github == true }
    var spaceID: String { space ?? Space.defaultID }
    var id: String { space.map { "\($0) \(baseURL)" } ?? baseURL }   // also the Keychain account name
    var hostKey: String { URL(string: baseURL).map { u in u.host.map { $0 + (u.port.map { ":\($0)" } ?? "") } ?? "" } ?? "" }

    // Keychain reads can block for seconds (securityd decrypt, ACL checks); never call these from a view body.
    // AppModel loads tokens once, off the main thread, and serves them from memory.
    nonisolated func readToken() -> String? { Keychain.get(account: id) }
    func saveToken(_ v: String?) { if let v { Keychain.set(v, account: id) } else { Keychain.delete(account: id) } }
}

enum Keychain {
    static let service = "dev.extypen.loupe.forgejo"
    static func set(_ value: String, account: String) {
        delete(account: account)
        let q: [String: Any] = [kSecClass as String: kSecClassGenericPassword, kSecAttrService as String: service,
                                kSecAttrAccount as String: account, kSecValueData as String: Data(value.utf8)]
        SecItemAdd(q as CFDictionary, nil)
    }
    static func get(account: String) -> String? {
        let q: [String: Any] = [kSecClass as String: kSecClassGenericPassword, kSecAttrService as String: service,
                                kSecAttrAccount as String: account, kSecReturnData as String: true]
        var out: AnyObject?
        guard SecItemCopyMatching(q as CFDictionary, &out) == errSecSuccess, let d = out as? Data else { return nil }
        return String(data: d, encoding: .utf8)
    }
    static func delete(account: String) {
        let q: [String: Any] = [kSecClass as String: kSecClassGenericPassword, kSecAttrService as String: service, kSecAttrAccount as String: account]
        SecItemDelete(q as CFDictionary)
    }
}

// MARK: - Client

/// Forgejo/Gitea API v1, or GitHub's REST API when `github` is set. GitHub's API is close enough to share the
/// models: the methods below convert where the two differ, so the views never know which server they talk to.
struct ForgejoClient {
    var baseURL: String
    var token: String
    var github = false

    init(baseURL: String, token: String, github: Bool = false) { self.baseURL = baseURL; self.token = token; self.github = github }
    init(_ a: ForgejoAccount, token: String) { self.init(baseURL: a.baseURL, token: token, github: a.isGitHub) }

    private var apiRoot: String {
        let base = baseURL.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        guard github else { return base + "/api/v1" }
        return base == "https://github.com" ? "https://api.github.com" : base + "/api/v3"   // GitHub Enterprise Server
    }
    private var serverName: String { github ? "GitHub" : "Forgejo" }
    /// Page-size parameter: Forgejo `limit`, GitHub `per_page`.
    private func paged(_ page: Int, _ size: Int) -> [String: String] { ["page": "\(page)", github ? "per_page" : "limit": "\(size)"] }

    private func request(_ method: String, _ path: String, query: [String: String] = [:], body: Encodable? = nil, accept: String = "application/json") throws -> URLRequest {
        var c = URLComponents(string: apiRoot + path)!
        if !query.isEmpty { c.queryItems = query.map { URLQueryItem(name: $0.key, value: $0.value) } }
        var r = URLRequest(url: c.url!)
        r.httpMethod = method
        r.setValue("token \(token)", forHTTPHeaderField: "Authorization")
        r.setValue(github && accept == "application/json" ? "application/vnd.github+json" : accept, forHTTPHeaderField: "Accept")
        if let body {
            r.setValue("application/json", forHTTPHeaderField: "Content-Type")
            r.httpBody = try JSONEncoder().encode(body)
        }
        return r
    }

    private func send(_ r: URLRequest) async throws -> Data {
        let (data, resp) = try await URLSession.shared.data(for: r)
        guard let http = resp as? HTTPURLResponse else { throw GitError(message: "No HTTP response") }
        guard (200..<300).contains(http.statusCode) else {
            let msg = (try? JSONSerialization.jsonObject(with: data) as? [String: Any])?["message"] as? String
            throw GitError(message: "\(serverName) \(http.statusCode): \(msg ?? String(decoding: data.prefix(300), as: UTF8.self))")
        }
        return data
    }

    private func get<T: Decodable>(_ path: String, query: [String: String] = [:]) async throws -> T {
        try JSONDecoder().decode(T.self, from: try await send(try request("GET", path, query: query)))
    }

    /// Follows page=N until a short page comes back.
    private func getAll<T: Decodable>(_ path: String, query: [String: String] = [:]) async throws -> [T] {
        var all: [T] = [], page = 1
        while true {
            let q = query.merging(paged(page, 50)) { a, _ in a }
            let batch: [T] = try await get(path, query: q)
            all += batch
            if batch.count < 50 { return all }
            page += 1
        }
    }

    func me() async throws -> FUser { try await get("/user") }

    /// One page, server default order (= the web UI's, newest first). Closed lists run to thousands; never fetch them all.
    func pulls(_ r: RemoteRepo, state: String, page: Int) async throws -> [PullRequest] {
        let list: [PullRequest] = try await get("/repos/\(r.owner)/\(r.name)/pulls", query: paged(page, Self.pullPage).merging(["state": state]) { a, _ in a })
        return list.map(normalized)
    }
    static let pullPage = 50

    func pull(_ r: RemoteRepo, _ n: Int) async throws -> PullRequest { normalized(try await get("/repos/\(r.owner)/\(r.name)/pulls/\(n)")) }

    private func normalized(_ p: PullRequest) -> PullRequest {
        var p = p
        if p.merged == nil, p.merged_at != nil { p.merged = true }
        return p
    }

    struct DiffTooLarge: LocalizedError { var errorDescription: String? { "The server won't serve a diff this large." } }

    /// Throws DiffTooLarge when GitHub refuses (over 300 files or 20k lines); the caller diffs locally instead.
    func diff(_ r: RemoteRepo, _ n: Int) async throws -> Data {
        guard github else { return try await send(try request("GET", "/repos/\(r.owner)/\(r.name)/pulls/\(n).diff", accept: "text/plain")) }
        do { return try await send(try request("GET", "/repos/\(r.owner)/\(r.name)/pulls/\(n)", accept: "application/vnd.github.diff")) }
        catch let e as GitError where e.message.hasPrefix("GitHub 406") || e.message.hasPrefix("GitHub 422") { throw DiffTooLarge() }
    }

    /// GitHub states → Forgejo's: CHANGES_REQUESTED → REQUEST_CHANGES, COMMENTED → COMMENT, DISMISSED → dismissed.
    /// The viewer's own unsubmitted (PENDING) review is left out.
    func reviews(_ r: RemoteRepo, _ n: Int) async throws -> [Review] {
        let list: [Review] = try await getAll("/repos/\(r.owner)/\(r.name)/pulls/\(n)/reviews")
        guard github else { return list }
        return list.filter { $0.state != "PENDING" }.map { v in
            var v = v
            switch v.state {
            case "CHANGES_REQUESTED": v.state = "REQUEST_CHANGES"
            case "COMMENTED": v.state = "COMMENT"
            case "DISMISSED": v.dismissed = true
            default: break
            }
            return v
        }
    }

    /// Inline comments of all reviews. Forgejo lists them per review; GitHub lists the whole PR at once and gives
    /// line + side instead of Forgejo's position (new-file line) / original_position (old-file line).
    func reviewComments(_ r: RemoteRepo, _ n: Int, reviews: [Review]) async throws -> [ReviewComment] {
        if github {
            struct C: Decodable {
                var id: Int; var user: FUser?; var path: String; var body: String
                var line: Int?; var side: String?; var created_at: String?; var commit_id: String?
            }
            let list: [C] = try await getAll("/repos/\(r.owner)/\(r.name)/pulls/\(n)/comments")
            return list.map { c in
                let left = c.side == "LEFT"
                return ReviewComment(id: c.id, user: c.user, path: c.path, body: c.body,
                                     position: left ? 0 : c.line ?? 0, original_position: left ? c.line ?? 0 : 0,
                                     created_at: c.created_at, commit_id: c.commit_id)
            }
        }
        return try await withThrowingTaskGroup(of: [ReviewComment].self) { g in
            for v in reviews { g.addTask { try await get("/repos/\(r.owner)/\(r.name)/pulls/\(n)/reviews/\(v.id)/comments") } }
            var all: [ReviewComment] = []
            for try await c in g { all += c }
            return all.sorted { $0.id < $1.id }
        }
    }

    func issueComments(_ r: RemoteRepo, _ n: Int) async throws -> [IssueComment] {
        // Forgejo returns them all and ignores paging; GitHub pages at 30.
        github ? try await getAll("/repos/\(r.owner)/\(r.name)/issues/\(n)/comments")
               : try await get("/repos/\(r.owner)/\(r.name)/issues/\(n)/comments")
    }

    func addIssueComment(_ r: RemoteRepo, _ n: Int, body: String) async throws {
        _ = try await send(try request("POST", "/repos/\(r.owner)/\(r.name)/issues/\(n)/comments", body: ["body": body]))
    }

    enum ReviewEvent: String, CaseIterable { case comment = "COMMENT", approve = "APPROVED", requestChanges = "REQUEST_CHANGES" }

    func submitReview(_ r: RemoteRepo, _ n: Int, event: ReviewEvent, body: String, commit: String, comments: [DraftComment]) async throws {
        let path = "/repos/\(r.owner)/\(r.name)/pulls/\(n)/reviews"
        if github {
            struct C: Encodable { var path: String; var body: String; var line: Int; var side: String }
            struct B: Encodable { var body: String; var event: String; var commit_id: String; var comments: [C] }
            let b = B(body: body, event: event == .approve ? "APPROVE" : event.rawValue, commit_id: commit,
                      comments: comments.compactMap { d in
                          d.newLine.map { C(path: d.path, body: d.body, line: $0, side: "RIGHT") }
                              ?? d.oldLine.map { C(path: d.path, body: d.body, line: $0, side: "LEFT") }
                      })
            _ = try await send(try request("POST", path, body: b))
            return
        }
        struct C: Encodable { var path: String; var body: String; var new_position: Int?; var old_position: Int? }
        struct B: Encodable { var body: String; var event: String; var commit_id: String; var comments: [C] }
        let b = B(body: body, event: event.rawValue, commit_id: commit,
                  comments: comments.map { C(path: $0.path, body: $0.body, new_position: $0.newLine, old_position: $0.newLine == nil ? $0.oldLine : nil) })
        _ = try await send(try request("POST", path, body: b))
    }

    // MARK: CI (commit statuses + Forgejo Actions)

    func combinedStatus(_ r: RemoteRepo, sha: String) async throws -> [CommitStatus] {
        if github { return try await gitHubChecks(r, sha: sha) }
        var all: [CommitStatus] = [], page = 1
        while true {
            let c: CombinedStatus = try await get("/repos/\(r.owner)/\(r.name)/commits/\(sha)/status", query: paged(page, 50))
            all += c.statuses ?? []
            if (c.statuses?.count ?? 0) < 50 || all.count >= (c.total_count ?? 0) { return all }
            page += 1
        }
    }

    /// GitHub reports Actions (and most apps) as check runs, older integrations as commit statuses; both become
    /// CommitStatus rows. Logs aren't tailed for GitHub: a row's details open in the browser.
    private func gitHubChecks(_ r: RemoteRepo, sha: String) async throws -> [CommitStatus] {
        struct S: Decodable { var id: Int; var state: String; var context: String; var description: String?; var target_url: String?; var created_at: String?; var updated_at: String? }
        struct Combined: Decodable { var statuses: [S] }
        struct Run: Decodable {
            struct Output: Decodable { var title: String? }
            var id: Int; var name: String; var status: String; var conclusion: String?; var html_url: String?
            var started_at: String?; var completed_at: String?; var output: Output?
        }
        struct Runs: Decodable { var check_runs: [Run] }
        async let combinedReq: Combined = get("/repos/\(r.owner)/\(r.name)/commits/\(sha)/status", query: ["per_page": "100"])
        async let runsReq: Runs = get("/repos/\(r.owner)/\(r.name)/commits/\(sha)/check-runs", query: ["per_page": "100"])
        let (combined, runs) = try await (combinedReq, runsReq)
        return combined.statuses.map {
            CommitStatus(id: $0.id, status: $0.state, context: $0.context, description: $0.description,
                         target_url: $0.target_url, created_at: $0.created_at, updated_at: $0.updated_at)
        } + runs.check_runs.map { c in
            // conclusion: success | failure | neutral | cancelled | skipped | timed_out | action_required | stale
            let status = switch c.status == "completed" ? c.conclusion ?? "" : "pending" {
            case "success", "neutral": "success"
            case "skipped", "stale": "skipped"
            case "pending": "pending"
            default: "failure"
            }
            return CommitStatus(id: c.id, status: status, context: c.name, description: c.output?.title,
                                target_url: c.html_url, created_at: c.started_at, updated_at: c.completed_at)
        }
    }

    func actionRuns(_ r: RemoteRepo, headSHA: String) async throws -> [ActionRun] {
        if github { return [] }   // Forgejo Actions API; GitHub's runs arrive as check runs above
        let page: ActionRunPage = try await get("/repos/\(r.owner)/\(r.name)/actions/runs", query: ["head_sha": headSHA, "limit": "100"])
        return page.workflow_runs ?? []
    }

    func actionJobs(_ r: RemoteRepo, runID: Int) async throws -> [ActionJob] {
        try await get("/repos/\(r.owner)/\(r.name)/actions/runs/\(runID)/jobs")
    }

    func actionJob(_ r: RemoteRepo, id: Int) async throws -> ActionJob {
        try await get("/repos/\(r.owner)/\(r.name)/actions/jobs/\(id)")
    }

    /// Log bytes from `range` ("bytes=N-" to tail, "bytes=-N" for the last N bytes). The server honours Range,
    /// so polling a running job only transfers what's new. Returns (data, firstByte, totalSize); empty data when nothing new.
    func actionLog(_ r: RemoteRepo, jobID: Int, range: String?) async throws -> (Data, Int, Int?) {
        var req = try request("GET", "/repos/\(r.owner)/\(r.name)/actions/jobs/\(jobID)/logs", accept: "text/plain")
        if let range { req.setValue(range, forHTTPHeaderField: "Range") }
        let (data, resp) = try await URLSession.shared.data(for: req)
        guard let http = resp as? HTTPURLResponse else { throw GitError(message: "No HTTP response") }
        if http.statusCode == 416 { return (Data(), 0, nil) }   // offset == size: no new output yet
        guard (200..<300).contains(http.statusCode) else { throw GitError(message: "Log \(http.statusCode): \(String(decoding: data.prefix(200), as: UTF8.self))") }
        // "Content-Range: bytes 1824000-1887148/1887149"
        if let cr = http.value(forHTTPHeaderField: "Content-Range"), let sp = cr.split(separator: " ").last {
            let parts = sp.split(separator: "/")
            let start = Int(parts.first?.split(separator: "-").first ?? "") ?? 0
            return (data, start, parts.count > 1 ? Int(parts[1]) : nil)
        }
        return (data, 0, data.count)
    }

    /// Forgejo user behind a commit's author email (nil when the email maps to no account).
    func commitAuthor(_ r: RemoteRepo, sha: String) async throws -> FUser? {
        struct C: Decodable { var author: FUser? }
        let list: [C] = try await get("/repos/\(r.owner)/\(r.name)/commits",
                                      query: ["sha": sha, github ? "per_page" : "limit": "1", "stat": "false", "verification": "false", "files": "false"])
        return list.first?.author
    }

    enum MergeStyle: String, CaseIterable { case merge, rebase, squash }

    func merge(_ r: RemoteRepo, _ n: Int, style: MergeStyle) async throws {
        if github {
            _ = try await send(try request("PUT", "/repos/\(r.owner)/\(r.name)/pulls/\(n)/merge", body: ["merge_method": style.rawValue]))
            return
        }
        _ = try await send(try request("POST", "/repos/\(r.owner)/\(r.name)/pulls/\(n)/merge", body: ["Do": style.rawValue]))
    }
}
