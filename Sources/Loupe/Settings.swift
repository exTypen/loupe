import SwiftUI
import AppKit

// MARK: - Appearance

enum Appearance: String, CaseIterable {
    case system, light, dark
    var title: String { rawValue.capitalized }
    /// NSApp.appearance rather than `.preferredColorScheme`: the latter can't hand a window back to the system setting.
    func apply() {
        NSApp.appearance = switch self {
        case .system: nil
        case .light: NSAppearance(named: .aqua)
        case .dark: NSAppearance(named: .darkAqua)
        }
    }
    static var saved: Appearance { Appearance(rawValue: UserDefaults.standard.string(forKey: "appearance") ?? "") ?? .system }
}

// MARK: - Settings window

struct SettingsView: View {
    @AppStorage("settingsTab") private var tab = "spaces"

    var body: some View {
        TabView(selection: $tab) {
            GeneralSettings()
                .tabItem { Label("General", systemImage: "gearshape") }.tag("general")
            SpacesSettings()
                .tabItem { Label("Spaces", systemImage: "square.stack.3d.up") }.tag("spaces")
        }
        .frame(width: 720, height: 500)
    }
}

struct GeneralSettings: View {
    @AppStorage("appearance") private var appearance = Appearance.system.rawValue

    var body: some View {
        Form {
            Picker("Appearance", selection: $appearance) {
                ForEach(Appearance.allCases, id: \.rawValue) { Text($0.title).tag($0.rawValue) }
            }
            .pickerStyle(.segmented)
            .onChange(of: appearance) { (Appearance(rawValue: appearance) ?? .system).apply() }
        }
        .formStyle(.grouped)
    }
}

// MARK: - Spaces

struct SpacesSettings: View {
    @Environment(AppModel.self) private var app
    @State private var selected: String?
    @State private var deleting: Space?

    var body: some View {
        @Bindable var app = app
        HStack(spacing: 0) {
            VStack(spacing: 0) {
                List(selection: $selected) {
                    ForEach(app.spaces) { sp in
                        HStack {
                            Text(sp.name).lineLimit(1)
                            Spacer()
                            if sp.id == app.currentSpace { Image(systemName: "checkmark").foregroundStyle(.secondary).help("Current space") }
                        }
                        .tag(sp.id)
                    }
                    .onMove { app.spaces.move(fromOffsets: $0, toOffset: $1) }
                }
                Divider()
                HStack(spacing: 0) {
                    Button { app.addSpace(); selected = app.currentSpace } label: { Image(systemName: "plus").frame(width: 24, height: 20) }
                        .help("New Space")
                    Divider().frame(height: 16)
                    Button { deleting = app.spaces.first { $0.id == selected } } label: { Image(systemName: "minus").frame(width: 24, height: 20) }
                        .disabled(selected == nil || app.spaces.count < 2)
                        .help("Delete Space…")
                    Spacer()
                }
                .buttonStyle(.borderless)
                .padding(4)
            }
            .frame(width: 200)
            Divider()
            if let i = app.spaces.firstIndex(where: { $0.id == selected }) {
                SpaceDetail(space: $app.spaces[i]).id(app.spaces[i].id)
            } else {
                ContentUnavailableView("No Space Selected", systemImage: "square.stack.3d.up")
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .onAppear { if selected == nil { selected = app.currentSpace } }
        .confirmationDialog("Delete space “\(deleting?.name ?? "")”?", isPresented: Binding(get: { deleting != nil }, set: { if !$0 { deleting = nil } })) {
            Button("Delete", role: .destructive) {
                if let d = deleting { app.deleteSpace(d.id); selected = app.currentSpace }
            }
        } message: { Text("Its repositories move to the neighbouring space. Its accounts are signed out and their tokens removed from the Keychain.") }
    }
}

struct SpaceDetail: View {
    @Environment(AppModel.self) private var app
    @Binding var space: Space
    @State private var adding = false

    var body: some View {
        Form {
            Section {
                TextField("Name", text: $space.name)
                if space.id != app.currentSpace {
                    LabeledContent("") { Button("Switch to This Space") { app.switchSpace(to: space.id) } }
                }
            }
            Section {
                let accounts = app.accounts(in: space.id)
                if accounts.isEmpty {
                    Text("No accounts. Pull requests, checks and HTTPS push and fetch need one for the repo's server.")
                        .foregroundStyle(.secondary)
                }
                ForEach(accounts) { a in
                    HStack {
                        Image(systemName: a.isGitHub ? "cat" : "server.rack").frame(width: 20).foregroundStyle(.secondary)
                        VStack(alignment: .leading, spacing: 1) {
                            Text(a.username).fontWeight(.medium)
                            Text(a.baseURL).font(.caption).foregroundStyle(.secondary)
                        }
                        Spacer()
                        if app.tokensLoaded && app.tokens[a.id] == nil {
                            Label("No token", systemImage: "exclamationmark.triangle.fill").font(.caption).foregroundStyle(.orange)
                        }
                        Button("Remove", role: .destructive) { app.setToken(nil, for: a); app.accounts.removeAll { $0.id == a.id } }
                    }
                }
            } header: {
                HStack {
                    Text("Accounts")
                    Spacer()
                    Button("Add Account…") { adding = true }
                }
            } footer: {
                Text("Used for every repository in this space. Adding an account for a server that already has one replaces it.")
                    .font(.caption).foregroundStyle(.secondary)
            }
            Section("Repositories") {
                if space.repos.isEmpty { Text("No repositories. Open one from the sidebar while this space is shown.").foregroundStyle(.secondary) }
                ForEach(space.repos, id: \.self) { repo in
                    HStack {
                        Image(systemName: "folder").frame(width: 20).foregroundStyle(.secondary)
                        VStack(alignment: .leading, spacing: 1) {
                            Text((repo as NSString).lastPathComponent).fontWeight(.medium)
                            Text((repo as NSString).abbreviatingWithTildeInPath).font(.caption).foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle)
                        }
                        Spacer()
                        if app.spaces.count > 1 {
                            Menu("Move to") {
                                ForEach(app.spaces.filter { $0.id != space.id }) { other in
                                    Button(other.name) { app.moveRepos([repo], to: other.id) }
                                }
                            }
                            .menuStyle(.button)
                            .fixedSize()
                        }
                        Button("Remove") { app.removeRepo(repo) }
                            .help("Remove from Loupe. Nothing on disk changes.")
                    }
                }
            }
        }
        .formStyle(.grouped)
        .sheet(isPresented: $adding) { AddAccountSheet(space: space.id) }
    }
}

// MARK: - Add account

struct AddAccountSheet: View {
    @Environment(AppModel.self) private var app
    @Environment(\.dismiss) private var dismiss
    var space: String
    @State private var github = true
    @State private var url = "https://github.com"
    @State private var token = ""
    @State private var status: String?
    @State private var checking = false

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Form { Section {
                Picker("Server", selection: $github) {
                    Text("GitHub").tag(true)
                    Text("Forgejo / Gitea").tag(false)
                }
                .pickerStyle(.segmented)
                .onChange(of: github) { url = github ? "https://github.com" : ""; token = ""; status = nil }
                TextField("Server URL", text: $url, prompt: Text(github ? "https://github.com" : "https://git.example.com"))
                SecureField("Access Token", text: $token,
                            prompt: Text(github ? "Settings → Developer settings → Personal access tokens" : "Settings → Applications → Generate token"))
                if github {
                    LabeledContent("") {
                        Button("Use GitHub CLI Token") { useGHToken() }.help("Reads the token `gh auth login` stored for this host")
                    }
                }
            } footer: {
                Text("Give the token repository read and write access: Loupe uses it for pushes and fetches over HTTPS too.")
                    .font(.caption).foregroundStyle(.secondary)
            } }
            .formStyle(.grouped)
            HStack {
                if let status { Text(status).font(.caption).foregroundStyle(.secondary) }
                Spacer()
                Button("Cancel") { dismiss() }.keyboardShortcut(.cancelAction)
                Button("Add") { add() }.keyboardShortcut(.defaultAction).disabled(url.isEmpty || token.isEmpty || checking)
            }
            .padding([.horizontal, .bottom], 20)
        }
        .frame(width: 460)
    }

    private func add() {
        var base = url.trimmingCharacters(in: .whitespacesAndNewlines)
        if !base.contains("://") { base = "https://" + base }
        while base.hasSuffix("/") { base.removeLast() }
        checking = true
        status = "Checking…"
        Task {
            defer { checking = false }
            do {
                let me = try await ForgejoClient(baseURL: base, token: token, github: github).me()
                let acct = ForgejoAccount(baseURL: base, username: me.login, github: github ? true : nil,
                                          space: space == Space.defaultID ? nil : space)
                app.setToken(token, for: acct)
                app.accounts.removeAll { $0.id == acct.id }
                app.accounts.append(acct)
                dismiss()
            } catch { status = error.localizedDescription }
        }
    }

    private func useGHToken() {
        let host = URL(string: url.contains("://") ? url : "https://" + url)?.host ?? "github.com"
        Task {
            if let t = await Self.ghToken(host: host) { token = t; status = "Token read from gh." }
            else { status = "No gh login for \(host). Run `gh auth login` first." }
        }
    }

    /// `gh auth token`; a GUI app's PATH has no Homebrew, so look in the usual places.
    static func ghToken(host: String) async -> String? {
        await Task.detached {
            guard let gh = ["/opt/homebrew/bin/gh", "/usr/local/bin/gh", "/usr/bin/gh"].first(where: FileManager.default.isExecutableFile) else { return nil }
            let p = Process(), out = Pipe()
            p.executableURL = URL(fileURLWithPath: gh)
            p.arguments = ["auth", "token", "--hostname", host]
            p.standardOutput = out
            p.standardError = FileHandle.nullDevice
            guard (try? p.run()) != nil else { return nil }
            let data = out.fileHandleForReading.readDataToEndOfFile()
            p.waitUntilExit()
            let t = String(decoding: data, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)
            return p.terminationStatus == 0 && !t.isEmpty ? t : nil
        }.value
    }
}
