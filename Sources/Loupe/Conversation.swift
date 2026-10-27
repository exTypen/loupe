import SwiftUI

/// A pull request's conversation as one timeline: the description, comments, reviews with their inline comments, and
/// pushes, oldest first. A comment straight after another by the same person joins its card; a push in between
/// starts a new one, so what was said before and after the change stays apart.
struct ConversationView: View {
    var pull: PullRequestModel
    var onOpen: (String, Int?) -> Void
    @State private var wide = false

    var body: some View {
        let (entries, newDay) = Timeline.entries(pull)
        ScrollViewReader { proxy in
            HStack(spacing: 0) {
                VStack(spacing: 0) {
                    ScrollView {
                        LazyVStack(alignment: .leading, spacing: 0) {
                            ForEach(Array(entries.enumerated()), id: \.element.id) { i, e in
                                if newDay[i] { DayMarker(date: e.date) }
                                row(e).id(e.id)
                            }
                        }
                        .background(alignment: .leading) {
                            // The rail the avatars sit on.
                            Rectangle().fill(Color.primary.opacity(0.1)).frame(width: 2).padding(.leading, Timeline.rail / 2 - 1)
                        }
                        .padding(.horizontal, 20).padding(.vertical, 16)
                        .frame(maxWidth: Timeline.maxWidth)
                        .frame(maxWidth: .infinity)
                    }
                    .defaultScrollAnchor(.bottom)   // the latest is what's new
                    Divider()
                    Composer(pull: pull).frame(maxWidth: Timeline.maxWidth).frame(maxWidth: .infinity)
                }
                // Past a readable line length the room goes to what a reviewer checks beside the thread.
                if wide {
                    Divider()
                    ConversationSidebar(pull: pull, entries: entries) { id in withAnimation(.easeOut(duration: 0.25)) { proxy.scrollTo(id, anchor: .top) } }
                        .frame(width: 290)
                }
            }
        }
        .onGeometryChange(for: Bool.self) { $0.size.width >= Timeline.maxWidth + 290 } action: { wide = $0 }
    }

    @ViewBuilder private func row(_ e: Timeline.Entry) -> some View {
        switch e.kind {
        case .opened:
            CardRow(user: pull.pr.user) {
                Card(header: Header(user: pull.pr.user, action: "opened this pull request", date: e.date, isAuthor: false)) {
                    if let b = pull.pr.body, !b.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { MarkdownText(text: b) }
                    else { Text("No description provided.").italic().foregroundStyle(.secondary) }
                }
            }
        case .comments(let cs):
            let user = cs[0].user ?? FUser(login: "?")
            CardRow(user: user) {
                Card(header: Header(user: user, action: "commented", date: e.date, isAuthor: user.login == pull.pr.user.login)) {
                    ForEach(Array(cs.enumerated()), id: \.element.id) { k, c in
                        if k > 0 {
                            HStack(spacing: 8) {
                                Rectangle().fill(Color.primary.opacity(0.08)).frame(height: 1)
                                Text(Timeline.date(c.created_at), format: .dateTime.hour().minute()).font(.caption2).foregroundStyle(.tertiary)
                            }
                        }
                        MarkdownText(text: c.body)
                    }
                }
            }
        case .review(let r, let inline):
            let user = r.user ?? FUser(login: "?")
            let style = ReviewStyle(r.state)
            CardRow(user: user, badge: style) {
                Card(header: Header(user: user, action: style.action, date: e.date, isAuthor: user.login == pull.pr.user.login,
                                    tint: style.color, outdated: r.commit_id.map { $0 != pull.pr.head.sha } ?? false),
                     tint: style.color) {
                    if !r.body.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { MarkdownText(text: r.body) }
                    ForEach(inline) { InlineCommentView(comment: $0, head: pull.pr.head.sha, onOpen: onOpen) }
                }
            }
        case .inline(let c):
            let user = c.user ?? FUser(login: "?")
            CardRow(user: user) {
                Card(header: Header(user: user, action: "commented on a line", date: e.date, isAuthor: user.login == pull.pr.user.login)) {
                    InlineCommentView(comment: c, head: pull.pr.head.sha, onOpen: onOpen)
                }
            }
        case .commits(let cs):
            CommitsRow(commits: cs, date: e.date)
        }
    }
}

/// The reply box. Its own view, so typing re-renders only this and not the timeline.
private struct Composer: View {
    var pull: PullRequestModel
    @State private var reply = ""
    @State private var sending = false

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            TextField("Leave a comment", text: $reply, axis: .vertical)
                .textFieldStyle(.plain)
                .lineLimit(3...12)
                .padding(10)
                .background(Color(nsColor: .textBackgroundColor), in: RoundedRectangle(cornerRadius: 8))
                .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(Color.primary.opacity(0.12)))
            HStack {
                Text("Markdown · ⌘↩ to send").font(.caption).foregroundStyle(.tertiary)
                Spacer()
                if sending { ProgressView().controlSize(.small) }
                Button("Comment") {
                    let r = reply
                    sending = true
                    Task { await pull.postComment(r); reply = ""; sending = false }
                }
                .buttonStyle(.borderedProminent)
                .disabled(sending || reply.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                .keyboardShortcut(.return, modifiers: .command)
            }
        }
        .padding(.horizontal, 20).padding(.vertical, 12)
    }
}

// MARK: - Building the timeline

enum Timeline {
    static let rail: CGFloat = 36           // width of the avatar column
    static let maxWidth: CGFloat = 960      // a readable line length for the thread
    static let joinWindow: TimeInterval = 30 * 60

    struct Entry: Identifiable {
        enum Kind { case opened, comments([IssueComment]), review(Review, [ReviewComment]), inline(ReviewComment), commits([PRCommit]) }
        var id: String
        var date: Date
        var kind: Kind
    }

    private struct Inputs: Equatable {
        var pr: PullRequest, conversation: [IssueComment], reviews: [Review], comments: [ReviewComment], commits: [PRCommit]
    }
    @MainActor private static var memo: (inputs: Inputs, entries: [Entry], newDay: [Bool])?

    /// The timeline and where each day starts, rebuilt only when the pull request's data changed. Comparing the
    /// inputs is cheap: unchanged arrays share storage, and Array's == checks that first.
    @MainActor static func entries(_ pull: PullRequestModel) -> ([Entry], [Bool]) {
        let inputs = Inputs(pr: pull.pr, conversation: pull.conversation, reviews: pull.reviews, comments: pull.comments, commits: pull.commits)
        if let m = memo, m.inputs == inputs { return (m.entries, m.newDay) }
        let entries = build(pull)
        let newDay = entries.indices.map { $0 == 0 || !Calendar.current.isDate(entries[$0].date, inSameDayAs: entries[$0 - 1].date) }
        memo = (inputs, entries, newDay)
        return (entries, newDay)
    }

    @MainActor static func build(_ pull: PullRequestModel) -> [Entry] {
        var raw: [Entry] = []
        if let d = pull.pr.created_at { raw.append(Entry(id: "opened", date: date(d), kind: .opened)) }
        for c in pull.conversation { raw.append(Entry(id: "c\(c.id)", date: date(c.created_at), kind: .comments([c]))) }
        let byReview = Dictionary(grouping: pull.comments.filter { $0.pull_request_review_id != nil }) { $0.pull_request_review_id! }
        for r in pull.reviews {
            let inline = byReview[r.id] ?? []
            // A bare "commented" review with nothing in it says nothing.
            guard !r.body.isEmpty || r.state != "COMMENT" || !inline.isEmpty else { continue }
            raw.append(Entry(id: "r\(r.id)", date: date(r.submitted_at), kind: .review(r, inline)))
        }
        let reviewIDs = Set(pull.reviews.map(\.id))
        for c in pull.comments where c.pull_request_review_id.map({ !reviewIDs.contains($0) }) ?? true {
            raw.append(Entry(id: "i\(c.id)", date: date(c.created_at), kind: .inline(c)))
        }
        for c in pull.commits {
            raw.append(Entry(id: "k\(c.sha)", date: date(c.commit.committer?.date ?? c.commit.author?.date), kind: .commits([c])))
        }
        // Stable: the opening post stays first even when a commit is older than the pull request.
        let sorted = raw.enumerated().sorted { a, b in
            if case .opened = a.element.kind { return true }
            if case .opened = b.element.kind { return false }
            return a.element.date != b.element.date ? a.element.date < b.element.date : a.offset < b.offset
        }.map(\.element)

        var out: [Entry] = []
        for e in sorted {
            if let last = out.last {
                switch (last.kind, e.kind) {
                case (.commits(let a), .commits(let b)):
                    out[out.count - 1].kind = .commits(a + b); continue
                case (.comments(let a), .comments(let b))
                    where a.last?.user?.login == b[0].user?.login && e.date.timeIntervalSince(date(a.last!.created_at)) < joinWindow:
                    out[out.count - 1].kind = .comments(a + b); continue
                default: break
                }
            }
            out.append(e)
        }
        return out
    }

    private static let iso: ISO8601DateFormatter = { let f = ISO8601DateFormatter(); f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]; return f }()
    private static let isoPlain = ISO8601DateFormatter()
    static func date(_ s: String?) -> Date {
        guard let s else { return .distantPast }
        return iso.date(from: s) ?? isoPlain.date(from: s) ?? .distantPast
    }
}

// MARK: - Sidebar

/// Beside the thread: who reviewed and how (a click goes to their review), and everyone who took part. Checks and
/// the change in numbers are in the header already.
private struct ConversationSidebar: View {
    var pull: PullRequestModel
    var entries: [Timeline.Entry]
    var jump: (String) -> Void

    /// Everyone the pull request knows, by login, for avatars.
    private var users: [String: FUser] {
        var u: [String: FUser] = [:]
        let all = [pull.pr.user] + (pull.pr.requested_reviewers ?? []) + pull.reviews.compactMap(\.user) + pull.conversation.compactMap(\.user)
            + pull.comments.compactMap(\.user) + pull.commits.compactMap(\.author)
        for x in all where u[x.login]?.avatar_url == nil { u[x.login] = x }
        return u
    }

    private func latestReview(by login: String) -> String? {
        entries.last { e in if case .review(let r, _) = e.kind { return r.user?.login == login }; return false }?.id
    }

    var body: some View {
        let users = self.users
        ScrollView {
            VStack(alignment: .leading, spacing: 22) {
                section("Reviewers") {
                    if pull.reviewerStates.isEmpty { Text("No reviews yet").foregroundStyle(.secondary) }
                    ForEach(pull.reviewerStates, id: \.login) { r in
                        let target = latestReview(by: r.login)
                        Button { if let target { jump(target) } } label: {
                            HStack(spacing: 8) {
                                Avatar(user: users[r.login] ?? FUser(login: r.login), size: 20)
                                Text(r.login).lineLimit(1)
                                Spacer(minLength: 4)
                                switch r.state {
                                case .approved: Label("Approved", systemImage: "checkmark.circle.fill").foregroundStyle(.green)
                                case .changesRequested: Label("Changes", systemImage: "exclamationmark.circle.fill").foregroundStyle(.orange)
                                case .pending: Label("Pending", systemImage: "circle.dashed").foregroundStyle(.secondary)
                                }
                            }
                            .labelStyle(.titleAndIcon).font(.callout)
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .disabled(target == nil)
                        .help(target == nil ? "" : "Go to \(r.login)'s review")
                    }
                }
                section("Participants") {
                    let logins = Array(Set(entries.flatMap { e -> [String] in
                        switch e.kind {
                        case .opened: [pull.pr.user.login]
                        case .comments(let cs): cs.compactMap { $0.user?.login }
                        case .review(let r, let i): [r.user?.login].compactMap { $0 } + i.compactMap { $0.user?.login }
                        case .inline(let c): [c.user?.login].compactMap { $0 }
                        case .commits(let cs): cs.compactMap { $0.author?.login }
                        }
                    })).sorted()
                    LazyVGrid(columns: [GridItem(.adaptive(minimum: 28), spacing: 6)], alignment: .leading, spacing: 6) {
                        ForEach(logins, id: \.self) { l in Avatar(user: users[l] ?? FUser(login: l), size: 28).help(l) }
                    }
                }
            }
            .font(.callout)
            .padding(16)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    private func section<C: View>(_ title: String, @ViewBuilder _ content: () -> C) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(title).font(.caption.weight(.semibold)).foregroundStyle(.secondary)
            content()
        }
    }
}

// MARK: - Pieces

private struct ReviewStyle {
    var action: String, color: Color, icon: String
    init(_ state: String) {
        switch state {
        case "APPROVED": (action, color, icon) = ("approved these changes", .green, "checkmark")
        case "REQUEST_CHANGES": (action, color, icon) = ("requested changes", .orange, "exclamationmark")
        case "REQUEST_REVIEW": (action, color, icon) = ("was asked to review", .secondary, "eye")
        default: (action, color, icon) = ("reviewed", .secondary, "text.bubble")
        }
    }
}

/// An avatar on the rail and a card beside it.
private struct CardRow<Content: View>: View {
    var user: FUser
    var badge: ReviewStyle? = nil
    @ViewBuilder var content: Content
    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            Avatar(user: user, size: 32)
                .overlay(alignment: .bottomTrailing) {
                    if let badge, badge.color != .secondary {
                        Image(systemName: badge.icon).font(.system(size: 8, weight: .bold)).foregroundStyle(.white)
                            .frame(width: 15, height: 15).background(badge.color, in: Circle())
                            .overlay(Circle().strokeBorder(Color(nsColor: .windowBackgroundColor), lineWidth: 2))
                            .offset(x: 4, y: 4)
                    }
                }
                .frame(width: Timeline.rail - 4)
                .padding(.top, 2)
            content
        }
        .padding(.vertical, 8)
    }
}

private struct Header: View {
    var user: FUser
    var action: String
    var date: Date
    var isAuthor: Bool
    var tint: Color = .secondary
    var outdated = false
    var body: some View {
        HStack(spacing: 6) {
            Text(user.login).fontWeight(.semibold)
            if Self.isBot(user.login) { Tag(text: "bot") }
            if isAuthor { Tag(text: "Author") }
            Text(action).foregroundStyle(tint == .secondary ? .secondary : tint)
            if outdated { Tag(text: "earlier commit").help("Made on a commit that's no longer the head of this pull request") }
            Spacer(minLength: 8)
            if date != .distantPast {
                Text(date.formatted(.relative(presentation: .named))).font(.caption).foregroundStyle(.secondary)
                    .help(date.formatted(date: .complete, time: .shortened))
            }
        }
        .font(.callout)
    }
    static func isBot(_ login: String) -> Bool { login.hasSuffix("[bot]") || login.lowercased().hasSuffix("bot") }
}

private struct Tag: View {
    var text: String
    var body: some View {
        Text(text).font(.caption2.weight(.medium)).foregroundStyle(.secondary)
            .padding(.horizontal, 5).padding(.vertical, 1)
            .overlay(Capsule().strokeBorder(Color.primary.opacity(0.18)))
    }
}

private struct Card<Content: View>: View {
    var header: Header
    var tint: Color = .secondary
    @ViewBuilder var content: Content
    var body: some View {
        let accent = tint == .secondary ? Color.primary : tint
        VStack(alignment: .leading, spacing: 0) {
            header.padding(.horizontal, 12).padding(.vertical, 8)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(accent.opacity(tint == .secondary ? 0.04 : 0.09), in: UnevenRoundedRectangle(topLeadingRadius: 8, topTrailingRadius: 8))
            Divider().opacity(0.6)
            VStack(alignment: .leading, spacing: 10) { content }
                .padding(12)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        // Shaped backgrounds rather than a clip: clipping makes a mask layer for every card.
        .background(Color.primary.opacity(0.02), in: RoundedRectangle(cornerRadius: 8))
        .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(accent.opacity(tint == .secondary ? 0.1 : 0.35)))
    }
}

/// A run of pushed commits: one line each, under who pushed them.
private struct CommitsRow: View {
    var commits: [PRCommit]
    var date: Date
    var body: some View {
        let who = commits.last?.author ?? FUser(login: commits.last?.commit.author?.name ?? "?")
        HStack(alignment: .top, spacing: 12) {
            Image(systemName: "point.3.connected.trianglepath.dotted").font(.system(size: 11, weight: .medium)).foregroundStyle(.secondary)
                .frame(width: 22, height: 22)
                .background(Color(nsColor: .windowBackgroundColor), in: Circle())
                .overlay(Circle().strokeBorder(Color.primary.opacity(0.15)))
                .frame(width: Timeline.rail - 4)
            VStack(alignment: .leading, spacing: 5) {
                HStack(spacing: 6) {
                    Avatar(user: who, size: 16)
                    Text(who.login).fontWeight(.semibold)
                    Text(commits.count == 1 ? "added a commit" : "added \(commits.count) commits").foregroundStyle(.secondary)
                    Spacer()
                    Text(date.formatted(.relative(presentation: .named))).font(.caption).foregroundStyle(.secondary)
                        .help(date.formatted(date: .complete, time: .shortened))
                }
                .font(.callout)
                ForEach(commits) { c in
                    HStack(spacing: 8) {
                        Text(String(c.sha.prefix(7))).font(.caption.monospaced()).foregroundStyle(.secondary)
                        Text(c.commit.message.split(separator: "\n").first.map(String.init) ?? "").lineLimit(1).truncationMode(.tail)
                            .help(c.commit.message)
                    }
                    .font(.callout)
                    .textSelection(.enabled)
                }
            }
            .padding(.top, 2)
        }
        .padding(.vertical, 8)
    }
}

private struct DayMarker: View {
    var date: Date
    var body: some View {
        let cal = Calendar.current
        let text = date == .distantPast ? "Earlier" : cal.isDateInToday(date) ? "Today" : cal.isDateInYesterday(date) ? "Yesterday"
            : date.formatted(.dateTime.weekday(.wide).day().month(.wide))
        Text(text).font(.caption.weight(.semibold)).foregroundStyle(.secondary)
            .padding(.horizontal, 8).padding(.vertical, 2)
            .background(Color(nsColor: .windowBackgroundColor), in: Capsule())
            .overlay(Capsule().strokeBorder(Color.primary.opacity(0.1)))
            .padding(.leading, Timeline.rail / 2 - 20)
            .padding(.vertical, 10)
    }
}

/// An inline review comment: where it is, the code it's on, and what it says.
private struct InlineCommentView: View {
    var comment: ReviewComment
    var head: String
    var onOpen: (String, Int?) -> Void
    var body: some View {
        let line = comment.position > 0 ? comment.position : comment.original_position
        VStack(alignment: .leading, spacing: 0) {
            Button { onOpen(comment.path, comment.position > 0 ? comment.position : nil) } label: {
                HStack(spacing: 6) {
                    FileIcon(path: comment.path)
                    Text(comment.path).font(.caption.monospaced()).lineLimit(1).truncationMode(.head)
                    if line > 0 { Text("line \(line)").font(.caption).foregroundStyle(.secondary) }
                    if let c = comment.commit_id, c != head { Tag(text: "outdated") }
                    Spacer()
                    Image(systemName: "arrow.up.right").font(.caption2).foregroundStyle(.tertiary)
                }
                .padding(.horizontal, 10).padding(.vertical, 6)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .help("Open in Files")
            if let h = comment.diff_hunk, !h.isEmpty {
                Divider().opacity(0.6)
                HunkTail(hunk: h, path: comment.path)
            }
            Divider().opacity(0.6)
            HStack(alignment: .top, spacing: 8) {
                Avatar(user: comment.user ?? FUser(login: "?"), size: 18)
                VStack(alignment: .leading, spacing: 3) {
                    Text(comment.user?.login ?? "?").font(.callout.weight(.semibold))
                    MarkdownText(text: comment.body)
                }
            }
            .padding(10)
        }
        .background(Color.primary.opacity(0.025), in: RoundedRectangle(cornerRadius: 6))
        .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(Color.primary.opacity(0.1)))
    }
}

/// The last lines of a review comment's hunk: the code it points at.
private struct HunkTail: View {
    var hunk: String
    var path: String
    private let ls: [String]
    @State private var colored: [AttributedString]

    init(hunk: String, path: String) {
        self.hunk = hunk; self.path = path
        ls = Array(hunk.split(separator: "\n", omittingEmptySubsequences: false).map(String.init).filter { !$0.hasPrefix("@@") }.suffix(4))
        _colored = State(initialValue: HighlightCache.cached(ls.map { String($0.dropFirst()) }, path: path) ?? [])
    }

    var body: some View {
        let ls = self.ls
        VStack(alignment: .leading, spacing: 0) {
            ForEach(Array(ls.enumerated()), id: \.offset) { i, l in
                let mark = l.first.map(String.init) ?? " "
                HStack(spacing: 6) {
                    Text(mark == " " ? " " : mark == "+" ? "+" : "−").foregroundStyle(mark == "+" ? .green : mark == "-" ? .red : .secondary).frame(width: 8)
                    Text(i < colored.count ? colored[i] : AttributedString(String(l.dropFirst()))).lineLimit(1)
                }
                .padding(.horizontal, 10).padding(.vertical, 1)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(mark == "+" ? Color(nsColor: Theme.addBG) : mark == "-" ? Color(nsColor: Theme.delBG) : .clear)
            }
        }
        .font(.editor(11))
        .foregroundStyle(Color(nsColor: Theme.foreground))
        .padding(.vertical, 4)
        .background(Color(nsColor: Theme.background))
        .task(id: hunk) {
            guard colored.isEmpty else { return }
            colored = await HighlightCache.highlight(ls.map { String($0.dropFirst()) }, path: path)
        }
    }
}

// MARK: - Markdown

/// Block-level Markdown, enough for review comments and bot reports: headings, paragraphs, lists, quotes, fenced
/// code, tables and rules. Inline styling comes from AttributedString; HTML tags are dropped,
/// keeping their text (a `<summary>` reads as a bold line).
struct MarkdownText: View {
    var text: String

    enum Block { case paragraph(String), heading(Int, String), list([(indent: Int, marker: String, text: String)]), quote(String), code(String, String), table([[String]]), rule }

    /// A block with its inline Markdown already turned into AttributedStrings.
    enum Rendered {
        case paragraph(AttributedString), heading(Int, AttributedString), list([(indent: Int, marker: String, text: AttributedString)])
        case quote(AttributedString), code(String, String), table([[AttributedString]]), rule   // a table's first row is its header
    }
    private final class Blocks { let value: [Rendered]; init(_ v: [Rendered]) { value = v } }
    /// Parsing and AttributedString(markdown:) are slow, and a lazy stack recreates a card each time it scrolls back
    /// in: render each text once. Bounded by count and by roughly the text's size.
    private static let cache: NSCache<NSString, Blocks> = {
        let c = NSCache<NSString, Blocks>(); c.countLimit = 500; c.totalCostLimit = 8 << 20; return c
    }()

    static func rendered(_ text: String) -> [Rendered] {
        if let b = cache.object(forKey: text as NSString) { return b.value }
        let out: [Rendered] = parse(text).map { b in
            switch b {
            case .paragraph(let s): .paragraph(inline(s))
            case .heading(let l, let s): .heading(l, inline(s))
            case .list(let items): .list(items.map { ($0.indent, $0.marker, inline($0.text)) })
            case .quote(let s): .quote(inline(s))
            case .code(let lang, let s): .code(lang, s)
            case .table(let rows): .table(rows.map { $0.map(inline) })
            case .rule: .rule
            }
        }
        cache.setObject(Blocks(out), forKey: text as NSString, cost: text.utf16.count * 6)
        return out
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            ForEach(Array(Self.rendered(text).enumerated()), id: \.offset) { _, b in block(b) }
        }
        .textSelection(.enabled)
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    @ViewBuilder private func block(_ b: Rendered) -> some View {
        switch b {
        case .paragraph(let s): Text(s).fixedSize(horizontal: false, vertical: true)
        case .heading(let level, let s):
            Text(s).font(level == 1 ? .title3.weight(.semibold) : level == 2 ? .headline : .subheadline.weight(.semibold))
                .padding(.top, 2)
        case .list(let items):
            VStack(alignment: .leading, spacing: 3) {
                ForEach(Array(items.enumerated()), id: \.offset) { _, it in
                    HStack(alignment: .firstTextBaseline, spacing: 6) {
                        Text(it.marker).foregroundStyle(.secondary).frame(minWidth: 12, alignment: .trailing)
                        Text(it.text).fixedSize(horizontal: false, vertical: true)
                    }
                    .padding(.leading, CGFloat(it.indent) * 14)
                }
            }
        case .quote(let s):
            Text(s).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                .padding(.leading, 10)
                .overlay(alignment: .leading) { Rectangle().fill(Color.primary.opacity(0.2)).frame(width: 3) }
        case .code(let lang, let s): CodeBlock(text: s, path: "x." + (lang.isEmpty ? "txt" : lang), maxLines: 30)
        case .table(let rows):
            let width = rows.map(\.count).max() ?? 0
            Grid(alignment: .leading, horizontalSpacing: 0, verticalSpacing: 0) {
                ForEach(Array(rows.enumerated()), id: \.offset) { i, row in
                    GridRow {
                        ForEach(0..<width, id: \.self) { k in
                            Text(k < row.count ? row[k] : "")
                                .fontWeight(i == 0 ? .semibold : nil)
                                .fixedSize(horizontal: false, vertical: true)
                                .padding(.horizontal, 8).padding(.vertical, 4)
                                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .leading)
                                .background(i == 0 ? Color.primary.opacity(0.07) : i % 2 == 0 ? Color.primary.opacity(0.03) : .clear)
                                .overlay(alignment: .leading) { if k > 0 { Rectangle().fill(Color.primary.opacity(0.1)).frame(width: 1) } }
                        }
                    }
                    if i == 0 { Rectangle().fill(Color.primary.opacity(0.12)).frame(height: 1).gridCellUnsizedAxes(.horizontal) }
                }
            }
            .clipShape(RoundedRectangle(cornerRadius: 4))
            .overlay(RoundedRectangle(cornerRadius: 4).strokeBorder(Color.primary.opacity(0.12)))
        case .rule: Divider()
        }
    }

    static func inline(_ s: String) -> AttributedString {
        (try? AttributedString(markdown: s, options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace))) ?? AttributedString(s)
    }

    /// A table's lines as cells, without the |---|:--:| line under the header.
    static func rows(_ lines: [String]) -> [[String]] {
        lines.map { line in
            var l = Substring(line.trimmingCharacters(in: .whitespaces))
            if l.hasPrefix("|") { l = l.dropFirst() }
            if l.hasSuffix("|") && !l.hasSuffix("\\|") { l = l.dropLast() }
            // Split on | but not on \| (a pipe inside a cell).
            var cells: [String] = [], cur = "", escaped = false
            for ch in l {
                if escaped { cur.append(ch == "|" ? "|" : "\\" + String(ch)); escaped = false }
                else if ch == "\\" { escaped = true }
                else if ch == "|" { cells.append(cur.trimmingCharacters(in: .whitespaces)); cur = "" }
                else { cur.append(ch) }
            }
            cells.append(cur.trimmingCharacters(in: .whitespaces))
            return cells
        }
        .filter { row in !row.allSatisfy { $0.range(of: #"^:?-+:?$"#, options: .regularExpression) != nil } }
    }

    static func parse(_ text: String) -> [Block] {
        var out: [Block] = [], para: [String] = [], list: [(indent: Int, marker: String, text: String)] = [], quote: [String] = [], table: [String] = []
        func flush() {
            if !para.isEmpty { out.append(.paragraph(para.joined(separator: "\n"))); para = [] }
            if !list.isEmpty { out.append(.list(list)); list = [] }
            if !quote.isEmpty { out.append(.quote(quote.joined(separator: "\n"))); quote = [] }
            if !table.isEmpty { out.append(.table(Self.rows(table))); table = [] }
        }
        let lines = text.replacingOccurrences(of: "\r\n", with: "\n").split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
        var i = 0
        while i < lines.count {
            let raw = lines[i]; i += 1
            let t = raw.trimmingCharacters(in: .whitespaces)
            if t.hasPrefix("```") || t.hasPrefix("~~~") {
                flush()
                let fence = String(t.prefix(3)), lang = String(t.dropFirst(3)).trimmingCharacters(in: .whitespaces)
                var code: [String] = []
                while i < lines.count, !lines[i].trimmingCharacters(in: .whitespaces).hasPrefix(fence) { code.append(lines[i]); i += 1 }
                i += 1
                out.append(.code(lang, code.joined(separator: "\n")))
                continue
            }
            // HTML: keep the words, lose the tags. A <summary> line is a heading of sorts.
            var line = raw
            if t.hasPrefix("<") {
                let stripped = t.replacingOccurrences(of: "<[^>]+>", with: "", options: .regularExpression).trimmingCharacters(in: .whitespaces)
                if stripped.isEmpty { continue }
                if t.lowercased().hasPrefix("<summary") { flush(); out.append(.heading(4, stripped)); continue }
                line = stripped
            }
            let s = line.trimmingCharacters(in: .whitespaces)
            if s.isEmpty { flush(); continue }
            if s.hasPrefix("|") { if table.isEmpty { flush() }; table.append(s); continue }
            if s == "---" || s == "***" || s == "___" { flush(); out.append(.rule); continue }
            if let h = s.firstIndex(where: { $0 != "#" }), s.hasPrefix("#"), s[h] == " ", s.distance(from: s.startIndex, to: h) <= 6 {
                flush(); out.append(.heading(s.distance(from: s.startIndex, to: h), String(s[h...]).trimmingCharacters(in: .whitespaces))); continue
            }
            if s.hasPrefix(">") { if quote.isEmpty { flush() }; quote.append(String(s.dropFirst()).trimmingCharacters(in: .whitespaces)); continue }
            let indent = (line.prefix { $0 == " " }.count) / 2
            if let m = s.firstMatch(of: #/^([-*+]|\d+[.)])\s+(.*)$/#) {
                if list.isEmpty { flush() }
                let marker = String(m.1)
                var body = String(m.2)
                if body.hasPrefix("[ ] ") { body = "☐ " + body.dropFirst(4) } else if body.lowercased().hasPrefix("[x] ") { body = "☑ " + body.dropFirst(4) }
                list.append((indent, "-*+".contains(marker) ? "•" : marker, body)); continue
            }
            if !list.isEmpty, raw.hasPrefix("  ") { list[list.count - 1].text += " " + s; continue }   // a list item's wrapped line
            if !list.isEmpty || !quote.isEmpty || !table.isEmpty { flush() }
            para.append(line)
        }
        flush()
        return out
    }
}
