import SwiftUI

struct SwitcherItem: Equatable { var repo: String; var wt: Worktree }

/// Every query token must appear in "repo branch dir"; matches ordered most-recently-used first, then sidebar order.
func switcherMatches(_ all: [SwitcherItem], query: String, recent: [String]) -> [SwitcherItem] {
    let rank = Dictionary(recent.enumerated().map { ($1, $0) }, uniquingKeysWith: min)
    let tokens = query.lowercased().split(separator: " ")
    return all.enumerated()
        .filter { _, i in
            let hay = "\((i.repo as NSString).lastPathComponent) \(i.wt.branch ?? "") \(i.wt.name)".lowercased()
            return tokens.allSatisfy { hay.contains($0) }
        }
        .sorted { (rank[$0.element.wt.path] ?? .max, $0.offset) < (rank[$1.element.wt.path] ?? .max, $1.offset) }
        .map(\.element)
}

struct QuickSwitcher: View {
    @Environment(AppModel.self) private var app
    @Environment(\.dismiss) private var dismiss
    @State private var query = ""
    @State private var index = 0

    private var items: [SwitcherItem] {
        let all = app.repoPaths.flatMap { r in (app.worktrees[r] ?? []).map { SwitcherItem(repo: r, wt: $0) } }
        return switcherMatches(all, query: query, recent: app.recent)
    }

    var body: some View {
        let items = items
        VStack(spacing: 0) {
            TextField("Go to worktree…", text: $query)
                .textFieldStyle(.plain).font(.title3).padding(12)
                .onKeyPress(.downArrow) { move(1, items.count) }
                .onKeyPress(.upArrow) { move(-1, items.count) }
                .onSubmit { if items.indices.contains(index) { open(items[index]) } }
            Divider()
            ScrollViewReader { proxy in
                List {
                    ForEach(Array(items.enumerated()), id: \.element.wt.path) { i, item in
                        HStack(spacing: 10) {
                            Text((item.repo as NSString).lastPathComponent).foregroundStyle(.secondary)
                                .lineLimit(1).frame(width: 130, alignment: .leading)
                            WorktreeRow(wt: item.wt, isMain: item.wt.path == item.repo, model: app.model(for: item.wt.path))
                        }
                        .contentShape(Rectangle())
                        .listRowBackground(i == index ? Color.accentColor.opacity(0.25) : Color.clear)
                        .onTapGesture { open(item) }
                    }
                }
                .listStyle(.plain)
                .onChange(of: index) { if items.indices.contains(index) { proxy.scrollTo(items[index].wt.path) } }
            }
        }
        .frame(width: 600, height: 380)
        .onExitCommand { dismiss() }
        // Empty query lists the current worktree first; preselect the previous one so ⌘K ↩ toggles back.
        .onAppear { index = items.count > 1 && items[0].wt.path == app.selection ? 1 : 0 }
        .onChange(of: query) { index = 0 }
    }

    private func move(_ d: Int, _ count: Int) -> KeyPress.Result {
        index = max(0, min(count - 1, index + d))
        return .handled
    }

    private func open(_ item: SwitcherItem) {
        app.collapsed.remove(item.repo)
        app.selection = item.wt.path
        dismiss()
    }
}
