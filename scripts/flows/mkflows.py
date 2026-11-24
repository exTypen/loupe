#!/usr/bin/env python3
"""Builds the human-scale "flows" fixture: two repos, worktrees in every state, and the data the mock forge serves.

    scripts/flows/mkflows.py <out> [--port 8790]

Deterministic: every commit, tag, stash and rebase step has a fixed author and date, so every run makes the same
SHAs. Re-running wipes and rebuilds <out>/home, <out>/repos, <out>/forge and <out>/scenario.json (that is the reset).

    <out>/home/.gitconfig      HOME for generation and for the apps (no ~/.gitconfig of the owner)
    <out>/forge/acme/*.git     bare repos the mock forge serves (scripts/flows/mockforge-flows.py)
    <out>/forge/lfs/<oid>      LFS objects the forge serves
    <out>/forge/state.json     pull requests, reviews, comments, CI profiles, users
    <out>/repos/...            the local clones and worktrees the apps open; every remote is http://127.0.0.1:<port>
    <out>/scenario.json        LOUPE_STRESS scenario (StressDriver.swift) with no steps, plus a "fixture" map
"""
import re as _re
import argparse, hashlib, json, os, random, shutil, struct, subprocess, sys, time, zlib
from string import Template

GIT = "/opt/homebrew/bin/git"   # a shell hook rewrites plain `git` output
ap = argparse.ArgumentParser()
ap.add_argument("out")
ap.add_argument("--port", type=int, default=8790)
args = ap.parse_args()

OUT = os.path.realpath(args.out)   # /tmp is /private/tmp: git reports real paths
HOME, REPOS, FORGE = (os.path.join(OUT, d) for d in ("home", "repos", "forge"))
URL = f"http://127.0.0.1:{args.port}"
for d in (HOME, REPOS, FORGE):
    shutil.rmtree(d, ignore_errors=True)
    os.makedirs(d)
with open(os.path.join(HOME, ".gitconfig"), "w") as f:
    f.write("[user]\n\tname = Alice Mock\n\temail = alice@example.com\n[init]\n\tdefaultBranch = main\n"
            "[protocol \"file\"]\n\tallow = always\n[advice]\n\tdetachedHead = false\n[gc]\n\tauto = 0\n"
            "[maintenance]\n\tauto = false\n[core]\n\tfsmonitor = false\n[pull]\n\trebase = false\n")
ENV = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
ENV.update(HOME=HOME, GIT_CONFIG_NOSYSTEM="1", GIT_EDITOR="true", GIT_TERMINAL_PROMPT="0", GIT_LFS_SKIP_PUSH="1")
subprocess.run([GIT, "lfs", "install", "--skip-repo"], env=ENV, check=True, stdout=subprocess.DEVNULL)   # filters in HOME/.gitconfig

PEOPLE = {   # login: (name, email)
    "alice": ("Alice Mock", "alice@example.com"), "bob": ("Bob Builder", "bob@example.com"),
    "carol": ("Carol Chen", "carol@example.com"), "dave": ("Dave Diaz", "dave@example.com"),
    "erin": ("Erin Okafor", "erin@example.com"), "frank": ("Frank Meyer", "frank@example.com"),
    "grace": ("Grace Hopper", "grace@example.com"), "renovate[bot]": ("Renovate Bot", "bot@renovate.example"),
    "xavier": ("Xavier Contractor", "x@contractor.invalid"),   # no forge account: initials avatar
}
CLOCK = 1_768_208_400   # 2026-01-12 09:00 UTC
rnd = random.Random(7)


TICK = (6, 38)   # hours between commits; shorter once the open-PR phase starts


def tick(lo=None, hi=None):
    global CLOCK
    lo, hi = lo or TICK[0], hi or TICK[1]
    CLOCK += rnd.randint(lo, hi) * 3600 + rnd.randint(0, 59) * 60


def clock_at(iso):
    global CLOCK
    CLOCK = max(CLOCK, int(time.mktime(time.strptime(iso, "%Y-%m-%dT%H:%M")) - time.timezone))


def iso(t=None):
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(CLOCK if t is None else t))


def git(*a, cwd, who="alice", check=True, env=None):
    """Every call carries a fixed author, committer and date: nothing git writes depends on the wall clock."""
    name, email = PEOPLE[who]
    d = f"@{CLOCK} +0000"
    e = dict(ENV, GIT_AUTHOR_NAME=name, GIT_AUTHOR_EMAIL=email, GIT_AUTHOR_DATE=d,
             GIT_COMMITTER_NAME=name, GIT_COMMITTER_EMAIL=email, GIT_COMMITTER_DATE=d, **(env or {}))
    r = subprocess.run([GIT, *a], cwd=cwd, env=e, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if check and r.returncode != 0:
        sys.exit(f"git {' '.join(a)} in {cwd} failed: {r.stderr.decode()[:2000]}")
    return r.stdout.decode().strip()


def W(wt, path, data):
    p = os.path.join(wt, path)
    os.makedirs(os.path.dirname(p), exist_ok=True)
    with open(p, "wb" if isinstance(data, bytes) else "w") as f:
        f.write(data)


def R(wt, path):
    with open(os.path.join(wt, path)) as f:
        return f.read()


def commit(wt, msg, who="alice", paths=None):
    tick()
    git("add", "-A", *(["--", *paths] if paths else []), cwd=wt)
    git("commit", "-q", "-m", msg, cwd=wt, who=who)
    return git("rev-parse", "HEAD", cwd=wt)


def log(msg):
    print(f"[{time.strftime('%H:%M:%S')}] {msg}", flush=True)


# ---------------------------------------------------------------------------------------------------------------------
# Content: code in nine languages (each file has a template for the functions history appends to it; the new functions
# call ones defined in other files, for go-to-definition), SQL, YAML, Markdown, images, a binary, LFS, generated code.
# ---------------------------------------------------------------------------------------------------------------------
VERBS = ["load", "save", "validate", "render", "parse", "sync", "fetch", "build", "merge", "index", "export", "resolve",
         "refresh", "encode", "decode", "schedule", "archive", "verify", "compute", "notify"]
NOUNS = ["invoice", "session", "user", "order", "token", "report", "ledger", "cache", "webhook", "profile", "account",
         "payment", "receipt", "quota", "audit", "search"]
NAMES = [(v, n) for v in VERBS for n in NOUNS]
rnd.shuffle(NAMES)


def next_name():
    v, n = NAMES.pop()
    return {"snake": f"{v}_{n}", "camel": v + n.title(), "pascal": v.title() + n.title(), "verb": v, "Verb": v.title(), "noun": n}


CODE = {
    "src/server/db.rs": ("""//! Database access for the API server.
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct User {
    pub id: u64,
    pub name: String,
    pub email: String,
}

pub struct Pool {
    users: HashMap<u64, User>,
}

impl Pool {
    pub fn new() -> Self {
        Pool { users: HashMap::new() }
    }
}

pub const MAX_CONNECTIONS: usize = 16;

/// Looks a user up by id.
pub fn fetch_user(pool: &Pool, id: u64) -> Option<User> {
    pool.users.get(&id).cloned()
}
""", """
/// ${Verb}s the ${noun} of a user.
pub fn ${snake}(pool: &Pool, id: u64) -> Option<String> {
    let user = fetch_user(pool, id)?;
    Some(format!("${noun}:{}:$k", user.email))
}
"""),
    "src/server/router.rs": ("""//! HTTP routing for the API server.
use crate::db::{fetch_user, Pool, MAX_CONNECTIONS};

pub struct Request {
    pub path: String,
    pub user_id: u64,
}

pub fn route(pool: &Pool, req: &Request) -> Result<String, String> {
    match req.path.as_str() {
        "/health" => Ok("ok".into()),
        "/me" => fetch_user(pool, req.user_id).map(|u| u.name).ok_or_else(|| "not found".into()),
        _ => Err(format!("no route for {}", req.path)),
    }
}
""", """
/// ${Verb}s the ${noun} for a request.
pub fn ${snake}(pool: &Pool, req: &Request) -> Result<String, String> {
    let user = fetch_user(pool, req.user_id).ok_or("unknown user")?;
    let limit = MAX_CONNECTIONS * $k;
    Ok(format!("${noun} {} for {}", limit, user.name))
}
"""),
    "ios/App/SessionStore.swift": ("""import Foundation

struct Session: Codable, Equatable {
    var id: UUID
    var userID: Int
    var token: String
}

final class SessionStore {
    private(set) var sessions: [Session] = []
    static let maxSessions = 8

    func current(for userID: Int) -> Session? {
        sessions.last { $0.userID == userID }
    }
}
""", """
extension SessionStore {
    /// ${Verb}s the ${noun} of the signed-in user.
    func ${camel}(userID: Int) -> String? {
        guard let s = current(for: userID) else { return nil }
        return "${noun}-\\(s.token.prefix($k))"
    }
}
"""),
    "ios/App/APIClient.swift": ("""import Foundation

struct APIClient {
    var baseURL: URL
    var store: SessionStore

    func request(_ path: String) -> URLRequest {
        var r = URLRequest(url: baseURL.appendingPathComponent(path))
        r.timeoutInterval = 30
        return r
    }
}
""", """
extension APIClient {
    func ${camel}() -> URLRequest {
        var r = request("/api/${noun}s")
        r.timeoutInterval = $k
        return r
    }
}
"""),
    "services/billing/invoice.go": ("""package billing

import (
	"errors"
	"fmt"
)

// Invoice is a bill sent to a customer.
type Invoice struct {
	ID       int64
	Customer string
	Cents    int64
}

var ErrEmpty = errors.New("billing: empty invoice")

// Total returns the invoice total in cents, refusing empty invoices.
func Total(inv Invoice) (int64, error) {
	if inv.Cents == 0 {
		return 0, ErrEmpty
	}
	return inv.Cents, nil
}

func describe(inv Invoice) string {
	return fmt.Sprintf("#%d %s", inv.ID, inv.Customer)
}
""", """
// ${pascal} ${verb}s the ${noun} for an invoice.
func ${pascal}(inv Invoice) (string, error) {
	t, err := Total(inv)
	if err != nil {
		return "", err
	}
	return fmt.Sprintf("${noun} %s %d", describe(inv), t*$k), nil
}
"""),
    "services/billing/main.go": ("""package billing

import "log"

// Run starts the billing worker.
func Run(invoices []Invoice) {
	for _, inv := range invoices {
		if _, err := Total(inv); err != nil {
			log.Printf("skip %s: %v", describe(inv), err)
		}
	}
}
""", """
func ${camel}Worker(invoices []Invoice) int {
	n := 0
	for _, inv := range invoices {
		if t, err := Total(inv); err == nil && t > $k {
			n++
		}
	}
	return n
}
"""),
    "web/src/api.ts": ("""export interface User {
  id: number;
  name: string;
  email: string;
}

export const API_BASE = "/api/v1";

export async function getJSON<T>(path: string): Promise<T> {
  const res = await fetch(API_BASE + path, { headers: { Accept: "application/json" } });
  if (!res.ok) throw new Error(`HTTP ${res.status} for ${path}`);
  return (await res.json()) as T;
}
""", """
/** ${Verb}s the ${noun}. */
export async function ${camel}(id: number): Promise<User> {
  const user = await getJSON<User>("/${noun}s/" + id);
  return { ...user, id: user.id + $k };
}
"""),
    "web/src/store.ts": ("""import { getJSON, User } from "./api";

export class Store {
  private users = new Map<number, User>();

  async user(id: number): Promise<User> {
    const cached = this.users.get(id);
    if (cached) return cached;
    const u = await getJSON<User>("/users/" + id);
    this.users.set(id, u);
    return u;
  }
}
""", """
export function ${camel}(store: Store, ids: number[]): Promise<User[]> {
  return Promise.all(ids.slice(0, $k).map((id) => store.user(id)));
}
"""),
    "web/src/components/Button.tsx": ("""import React from "react";

export type ButtonProps = {
  label: string;
  onClick: () => void;
  variant?: "primary" | "secondary";
};

export function Button({ label, onClick, variant = "primary" }: ButtonProps) {
  return (
    <button className={"btn btn-" + variant} onClick={onClick}>
      {label}
    </button>
  );
}
""", """
export function ${pascal}Button(props: { onClick: () => void }) {
  return <Button label="${Verb} ${noun}" variant="secondary" onClick={props.onClick} />;
}
"""),
    "web/src/util.js": ("""// Small helpers shared by the web client.
export function formatCents(cents) {
  return (cents / 100).toFixed(2);
}

export function debounce(fn, ms) {
  let t;
  return (...args) => {
    clearTimeout(t);
    t = setTimeout(() => fn(...args), ms);
  };
}
""", """
export function ${camel}(items) {
  return items.filter((x) => x.cents > $k).map((x) => formatCents(x.cents));
}
"""),
    "tools/migrate.py": ('''#!/usr/bin/env python3
"""Applies SQL migrations in db/migrations in order."""
import pathlib
import sqlite3

MIGRATIONS = pathlib.Path(__file__).resolve().parent.parent / "db" / "migrations"


def connect(path="app.db"):
    return sqlite3.connect(path)


def pending(conn):
    done = {r[0] for r in conn.execute("SELECT name FROM schema_migrations")}
    return [p for p in sorted(MIGRATIONS.glob("*.sql")) if p.name not in done]
''', '''

def ${snake}(conn, limit=$k):
    """${Verb} the ${noun} rows."""
    rows = conn.execute("SELECT id FROM ${noun}s LIMIT ?", (limit,)).fetchall()
    return [r[0] for r in rows] + [p.name for p in pending(conn)]
'''),
    "tools/report.py": ('''"""Monthly billing report."""
from migrate import connect


def totals(conn):
    return dict(conn.execute("SELECT customer, SUM(cents) FROM invoices GROUP BY customer"))
''', '''

def ${snake}_report(conn):
    data = totals(conn)
    return {k: v // $k for k, v in data.items()}
'''),
    "native/hash.c": ("""#include "hash.h"

/* FNV-1a, 64 bit. */
uint64_t acme_hash(const void *data, size_t len) {
    const unsigned char *p = data;
    uint64_t h = 0xcbf29ce484222325ULL;
    for (size_t i = 0; i < len; i++) {
        h ^= p[i];
        h *= 0x100000001b3ULL;
    }
    return h;
}
""", """
uint64_t acme_${snake}(const char *key, size_t len) {
    return acme_hash(key, len) % $k;
}
"""),
    "native/vec.cpp": ("""#include "vec.hpp"

namespace acme {

double mean(const Vec<double>& v) {
    double sum = 0;
    for (std::size_t i = 0; i < v.size(); ++i) sum += v.at(i);
    return v.size() ? sum / v.size() : 0;
}

}  // namespace acme
""", """
double ${camel}(const acme::Vec<double>& v) {
    return acme::mean(v) * $k;
}
"""),
    "android/app/src/main/java/dev/acme/app/Session.java": ("""package dev.acme.app;

import java.util.Optional;

public final class Session {
    private final long userId;
    private final String token;

    public Session(long userId, String token) {
        this.userId = userId;
        this.token = token;
    }

    public long userId() { return userId; }

    public Optional<String> token() {
        return token.isEmpty() ? Optional.empty() : Optional.of(token);
    }
}
""", """
    public String ${camel}() {
        return "${noun}:" + userId + ":" + token().orElse("anonymous").length() * $k;
    }
"""),
    "android/app/src/main/java/dev/acme/app/MainActivity.java": ("""package dev.acme.app;

import android.app.Activity;
import android.os.Bundle;

public class MainActivity extends Activity {
    private Session session;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        session = new Session(1L, "dev-token");
    }
}
""", """
    void ${camel}() {
        if (session.token().isPresent()) {
            setTitle("${Verb} ${noun} " + $k);
        }
    }
"""),
}
AREA = {"src/server": "server", "ios": "ios", "services/billing": "billing", "web": "web", "tools": "tools", "native": "native", "android": "android"}
STATIC = {
    "src/server/main.rs": """mod db;
mod router;

use router::{route, Request};

fn main() {
    let pool = db::Pool::new();
    let req = Request { path: "/health".into(), user_id: 1 };
    println!("{:?}", route(&pool, &req));
}
""",
    "Cargo.toml": '[package]\nname = "acme-server"\nversion = "0.1.0"\nedition = "2021"\n\n[[bin]]\nname = "acme-server"\npath = "src/server/main.rs"\n',
    "native/hash.h": "#ifndef ACME_HASH_H\n#define ACME_HASH_H\n\n#include <stddef.h>\n#include <stdint.h>\n\nuint64_t acme_hash(const void *data, size_t len);\n\n#endif\n",
    "native/vec.hpp": """#pragma once
#include <cstddef>
#include <vector>

namespace acme {

template <typename T>
class Vec {
public:
    void push(const T& v) { items_.push_back(v); }
    std::size_t size() const { return items_.size(); }
    const T& at(std::size_t i) const { return items_.at(i); }

private:
    std::vector<T> items_;
};

double mean(const Vec<double>& v);

}  // namespace acme
""",
    "tools/legacy_export.py": '"""Exports invoices as CSV for the old accounting system. Deprecated."""\nimport csv\nimport sys\n\n\ndef export(rows, out=sys.stdout):\n    w = csv.writer(out)\n    for r in rows:\n        w.writerow(r)\n',
    "go.mod": "module example.com/acme/app\n\ngo 1.23\n",
    "web/package.json": '{\n  "name": "acme-web",\n  "private": true,\n  "scripts": { "build": "vite build", "test": "vitest run" },\n  "dependencies": { "react": "18.3.1" }\n}\n',
    "README.md": """# acme/app

The acme billing platform: an API server (Rust), a billing worker (Go), a web client (TypeScript/React), iOS and
Android apps, and native helpers in C/C++.

## Layout

| Path | What |
| --- | --- |
| `src/server` | API server |
| `services/billing` | billing worker |
| `web` | web client |
| `ios`, `android` | mobile apps |
| `db` | schema and migrations |

## Running

```sh
cargo run --bin acme-server
go run ./services/billing
npm --prefix web run dev
```
""",
    "CHANGELOG.md": "# Changelog\n\n## Unreleased\n",
    "docs/architecture.md": """# Architecture

The API server owns the database. The billing worker reads invoices through the API, never directly.

```mermaid
graph LR
  web --> server
  ios --> server
  billing --> server
  server --> db[(Postgres)]
```

- Requests are routed in `src/server/router.rs`.
- Sessions live in `SessionStore` on iOS and `Session` on Android.
""",
    "docs/billing.md": "# Billing\n\nInvoices are created as drafts and finalized by the billing worker.\n\n1. Draft\n2. Finalized\n3. Paid\n\n"
                       "## Totals\n\nTotals are in cents. `Total` refuses empty invoices.\n\n## Currencies\n\nEUR and USD. JPY has no minor unit.\n\n"
                       "## Reminders\n\nThe worker sends a reminder 7 days before the due date and on the due date.\n",
    ".forgejo/pull_request_template.md": "## What\n\n\n## Why\n\n\n## Checklist\n\n- [ ] Tests added\n- [ ] Migration reviewed\n- [ ] Changelog updated\n",
    ".gitattributes": "*.psd filter=lfs diff=lfs merge=lfs -text\n*.woff2 binary\nweb/src/generated/** linguist-generated\n",
    ".gitignore": "/target\nnode_modules/\n*.db\n.DS_Store\n",
    ".forgejo/workflows/ci.yml": """name: CI
on:
  push:
    branches: [main]
  pull_request:

jobs:
  lint:
    runs-on: docker
    steps:
      - uses: actions/checkout@v4
      - run: cargo clippy -- -D warnings
  build:
    runs-on: docker
    steps:
      - uses: actions/checkout@v4
      - run: cargo build --release
  test:
    runs-on: docker
    needs: [build]
    steps:
      - uses: actions/checkout@v4
      - run: cargo test
      - run: go test ./services/...
  web:
    runs-on: docker
    steps:
      - uses: actions/checkout@v4
      - run: npm --prefix web ci && npm --prefix web test
""",
    "config/app.yaml": "server:\n  port: 8080\n  timeout_secs: 30\ndatabase:\n  url: postgres://acme@localhost/acme\n  pool: 16\nfeatures:\n  dark_mode: false\n  csv_export: false\n",
    "docker-compose.yml": "services:\n  db:\n    image: postgres:16\n    ports: [\"5432:5432\"]\n  server:\n    build: .\n    depends_on: [db]\n",
    "web/public/vendor.min.js": "".join(f"function v{k}(a,b){{return a*{k}+b-{k % 13}}};" for k in range(2500)) + "\n",
}


STATIC.update({
    # Manifests and lock files (dependency lens), configs in every format (config lens), both PR templates.
    "Cargo.toml": '[package]\nname = "acme-server"\nversion = "0.1.0"\nedition = "2021"\n\n[[bin]]\nname = "acme-server"\npath = "src/server/main.rs"\n\n'
                  '[dependencies]\nserde = { version = "1.0.200", features = ["derive"] }\ntokio = { version = "1.38", features = ["full"] }\naxum = "0.7.5"\n'
                  'sqlx = { version = "0.7.4", default-features = false }\nanyhow = "1.0.86"\nacme-lib = { path = "vendor/acme-lib" }\n\n'
                  '[dev-dependencies]\ninsta = "1.39"\n\n[target.\'cfg(unix)\'.dependencies]\nlibc = "0.2.155"\n',
    "Cargo.lock": "version = 3\n\n" + "".join(f'[[package]]\nname = "{n}"\nversion = "{v}"\nsource = "registry+https://github.com/rust-lang/crates.io-index"\n\n'
                                              for n, v in [("anyhow", "1.0.86"), ("axum", "0.7.5"), ("libc", "0.2.155"), ("serde", "1.0.200"),
                                                           ("syn", "1.0.109"), ("syn", "2.0.66"), ("tokio", "1.38.0")]),
    "web/package.json": '{\n  "name": "acme-web",\n  "private": true,\n  "scripts": { "build": "vite build", "test": "vitest run" },\n'
                        '  "dependencies": {\n    "react": "18.3.1",\n    "react-dom": "18.3.1",\n    "zod": "3.23.8"\n  },\n'
                        '  "devDependencies": {\n    "typescript": "5.4.5",\n    "vite": "5.2.11",\n    "vitest": "1.6.0"\n  },\n'
                        '  "peerDependencies": {\n    "react": ">=18"\n  },\n  "optionalDependencies": {\n    "fsevents": "2.3.3"\n  }\n}\n',
    "web/package-lock.json": '{\n  "name": "acme-web",\n  "lockfileVersion": 3,\n  "packages": {\n    "node_modules/react": { "version": "18.3.1" },\n'
                             '    "node_modules/zod": { "version": "3.23.8" },\n    "node_modules/vite/node_modules/esbuild": { "version": "0.20.2" }\n  }\n}\n',
    "go.mod": "module example.com/acme/app\n\ngo 1.23\n\nrequire (\n\tgithub.com/google/uuid v1.6.0\n\tgithub.com/stretchr/testify v1.9.0\n\tgolang.org/x/sync v0.7.0\n)\n",
    "requirements-dev.txt": "pytest==8.2.1\nruff==0.4.8\nmypy==1.10.0\n",
    "pyproject.toml": '[project]\nname = "acme-tools"\nversion = "0.4.0"\ndependencies = ["psycopg[binary]>=3.1", "click==8.1.7"]\n\n'
                      '[tool.poetry.dependencies]\npython = "^3.11"\nrich = "13.7.1"\n',
    "config/app.toml": '[server]\nport = 8080\nworkers = 4\n\n[database]\nurl = "postgres://acme@localhost/acme"\npool = 16\n\n[billing]\ncurrency = "EUR"\nretries = 3\n',
    "config/app.json": '{\n  "server": { "port": 8080, "cors": ["https://acme.example"] },\n  "features": { "darkMode": false, "csvExport": false }\n}\n',
    "config/.env.example": "# Copy to .env\nexport DATABASE_URL=postgres://acme@localhost/acme\nLOG_LEVEL=info\napi_token=changeme\n",
    "android/gradle.properties": "org.gradle.jvmargs=-Xmx2g\nandroid.useAndroidX=true\nacme.apiBase=https://api.acme.example\n",
    ".github/PULL_REQUEST_TEMPLATE.md": "## Summary\n\n(GitHub template: Loupe on Forgejo prefers .forgejo/pull_request_template.md)\n",
    # File tree: natural sort, mixed case, a deep single-child chain, two index.ts, an empty file.
    "docs/steps/step1.md": "# Step 1\n", "docs/steps/step2.md": "# Step 2\n", "docs/steps/step10.md": "# Step 10\n", "docs/steps/Step3.md": "# Step 3\n",
    "web/src/deep/a/b/c/d/index.ts": 'export const depth = 4;\n',
    "web/src/payments-legacy/index.ts": 'export const legacy = true;\n',
    "web/src/generated/.keep": "",
    "dev/test_data/seed.sql": "INSERT INTO users VALUES (1, 'alice@example.com', 'Alice');\nINSERT INTO users VALUES (2, 'bob@example.com', 'Bob');\nINSERT INTO users VALUES (3, 'carol@example.com', 'Carol');\n",
    "docs/overview.md": "# Overview\n\nacme bills customers.\n",
    "Makefile": "build:\n\tcargo build\n\ntest:\n\tcargo test\n",
    "Dockerfile": "FROM rust:1.80 AS build\nWORKDIR /src\nCOPY . .\nRUN cargo build --release\n",
    ".gitattributes": "*.psd filter=lfs diff=lfs merge=lfs -text\nassets/lfs/** filter=lfs diff=lfs merge=lfs -text\n*.woff2 binary\nweb/src/generated/** linguist-generated\n",
    ".gitignore": "/target\nnode_modules/\n*.db\n.DS_Store\n.direnv/\n.worktree/\n",
})


def png(w, h, seed):
    r = random.Random(seed)
    c1, c2 = [r.randrange(40, 230) for _ in range(3)], [r.randrange(40, 230) for _ in range(3)]
    rows = bytearray()
    for y in range(h):
        rows.append(0)
        for x in range(w):
            t = (x + y) / (w + h)
            rows += bytes(int(a + (b - a) * t) for a, b in zip(c1, c2))
    chunk = lambda t, d: struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d) & 0xffffffff)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(bytes(rows))) + chunk(b"IEND", b"")


def svg(color, label):
    return (f'<svg xmlns="http://www.w3.org/2000/svg" width="128" height="128" viewBox="0 0 128 128">\n'
            f'  <rect x="8" y="8" width="112" height="112" rx="24" fill="{color}"/>\n'
            f'  <text x="64" y="76" font-family="Helvetica" font-size="36" text-anchor="middle" fill="#fff">{label}</text>\n</svg>\n')


def blob(seed, n, magic):
    return magic + random.Random(seed).randbytes(n - len(magic))


def generated_ts(variant):
    out = ["// Code generated by acme-codegen. DO NOT EDIT.\n"]
    for k in range(2500):
        fields = [f"  id: number;", f"  name{k}: string;", f"  createdAt: string;", f"  ownerId{k % 97}: number;", f"  tags: string[];"]
        if variant and k % 37 == 0:
            fields.insert(2, f"  archived{k}?: boolean;")
        if variant and k % 53 == 0:
            fields.remove("  tags: string[];")
        out.append(f"export interface Model{k} {{\n" + "\n".join(fields) + "\n}\n\n")
    return "".join(out)


# Schema: tables → column lines; migrations add to it and re-render db/schema.sql.
SCHEMA = {"users": ["id          INTEGER PRIMARY KEY", "email       TEXT NOT NULL UNIQUE", "name        TEXT NOT NULL",
                    "created_at  TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP"],
          "invoices": ["id          INTEGER PRIMARY KEY", "customer_id INTEGER NOT NULL REFERENCES users(id)",
                       "cents       INTEGER NOT NULL", "status      TEXT NOT NULL DEFAULT 'draft'"]}
INDEXES = ["CREATE INDEX invoices_customer ON invoices(customer_id);"]


def schema_sql(schema, indexes):
    body = "\n\n".join(f"CREATE TABLE {t} (\n    " + ",\n    ".join(cols) + "\n);" for t, cols in schema.items())
    return "-- Schema for the acme app. db/migrations builds it; keep the two in step.\n\n" + body + "\n\n" + "\n".join(indexes) + "\n"


MIGRATIONS = [
    ("0002_sessions.sql", "sessions", ["id          INTEGER PRIMARY KEY", "user_id     INTEGER NOT NULL REFERENCES users(id)", "token       TEXT NOT NULL", "expires_at  TIMESTAMP NOT NULL"], None, None),
    ("0003_invoice_due_date.sql", None, None, ("invoices", "due_at      TIMESTAMP"), None),
    ("0004_audit_log.sql", "audit_log", ["id          INTEGER PRIMARY KEY", "actor_id    INTEGER REFERENCES users(id)", "action      TEXT NOT NULL", "at          TIMESTAMP NOT NULL"], None, None),
    ("0005_index_sessions.sql", None, None, None, "CREATE INDEX sessions_user ON sessions(user_id);"),
    ("0006_orders.sql", "orders", ["id          INTEGER PRIMARY KEY", "user_id     INTEGER NOT NULL REFERENCES users(id)", "total_cents INTEGER NOT NULL", "placed_at   TIMESTAMP NOT NULL"], None, None),
]


def apply_migration(wt, m, schema, indexes):
    name, table, cols, addcol, index = m
    if table:
        schema[table] = cols
        sql = f"CREATE TABLE {table} (\n    " + ",\n    ".join(cols) + "\n);\n"
    elif addcol:
        schema[addcol[0]].append(addcol[1])
        sql = f"ALTER TABLE {addcol[0]} ADD COLUMN {addcol[1].split()[0]} {' '.join(addcol[1].split()[1:])};\n"
    else:
        indexes.append(index)
        sql = index + "\n"
    W(wt, f"db/migrations/{name}", f"-- {name}\nBEGIN;\n{sql}COMMIT;\n")
    W(wt, "db/schema.sql", schema_sql(schema, indexes))


def area(path):
    return next((a for p, a in AREA.items() if path.startswith(p)), path.split("/")[0])


def add_fn(wt, path, name=None):
    """Appends a function from the file's template; returns its name (as the language spells it)."""
    nm = name or next_name()
    snippet = Template(CODE[path][1]).substitute(nm, k=rnd.randint(2, 64))
    text = R(wt, path)
    if path.endswith(".java"):
        i = text.rstrip().rfind("}")
        text = text[:i].rstrip("\n") + "\n" + snippet + "}\n"
    else:
        text = text.rstrip("\n") + "\n" + snippet
    W(wt, path, text)
    lang = path.rsplit(".", 1)[1]
    return nm["pascal"] if path.endswith("invoice.go") or lang == "tsx" else nm["camel"] + "Worker" if path.endswith("main.go") \
        else nm["snake"] if lang in ("rs", "py", "c") else nm["camel"]


def tweak(wt, path):
    """Changes one small integer literal (a limit, a timeout) somewhere in the file."""
    import re
    text = R(wt, path)
    hits = [m for m in re.finditer(r"(?<![\w.#x])(\d{1,3})(?![\w.])", text)]
    if not hits:
        return False
    m = hits[rnd.randrange(len(hits))]
    W(wt, path, text[:m.start()] + str(int(m.group(1)) + rnd.randint(1, 9)) + text[m.end():])
    return True


def changelog(wt, line):
    t = R(wt, "CHANGELOG.md").replace("## Unreleased\n", f"## Unreleased\n\n- {line}", 1)
    W(wt, "CHANGELOG.md", t.replace(f"- {line}\n\n", f"- {line}\n", 1) if t.count("\n- ") > 1 else t)


# ---------------------------------------------------------------------------------------------------------------------
# acme/lib: a small Rust crate, the app's submodule. One linked worktree stopped in an interactive rebase.
# ---------------------------------------------------------------------------------------------------------------------
log("acme/lib")
LIB = os.path.join(REPOS, "lib")
LIB_BARE = os.path.join(FORGE, "acme", "lib.git")
git("init", "-q", LIB, cwd=REPOS)
W(LIB, "Cargo.toml", '[package]\nname = "acme-lib"\nversion = "0.3.0"\nedition = "2021"\n')
W(LIB, "README.md", "# acme-lib\n\nSmall helpers shared by acme services: hex, constant-time compare, base64.\n")
W(LIB, "src/lib.rs", "//! acme-lib\npub mod hex;\n")
W(LIB, "src/hex.rs", "/// Lower-case hex of `bytes`.\npub fn encode(bytes: &[u8]) -> String {\n    bytes.iter().map(|b| format!(\"{:02x}\", b)).collect()\n}\n")
lib_commits = [commit(LIB, "Initial crate: hex encoding")]
for k, (msg, who) in enumerate([("hex: decode", "bob"), ("docs: usage example", "alice"), ("hex: reject odd lengths", "carol"),
                                ("Bump to 0.3.1", "alice"), ("hex: faster encode with a lookup table", "bob"),
                                ("ci: run clippy", "dave"), ("Bump to 0.4.0", "alice")]):
    W(LIB, "src/hex.rs", R(LIB, "src/hex.rs") + f"\n/// helper {k}\npub fn helper_{k}(x: u8) -> u8 {{\n    x.wrapping_mul({k + 3})\n}}\n")
    lib_commits.append(commit(LIB, msg, who))
    if k == 3:   # the branch the lib-rebase worktree rebases, from here
        git("checkout", "-q", "-b", "feature/ct-compare", cwd=LIB)
        W(LIB, "src/ct.rs", "/// Constant-time equality.\npub fn eq(a: &[u8], b: &[u8]) -> bool {\n    a.len() == b.len() && a.iter().zip(b).fold(0, |acc, (x, y)| acc | (x ^ y)) == 0\n}\n")
        W(LIB, "src/lib.rs", R(LIB, "src/lib.rs") + "pub mod ct;\n")
        commit(LIB, "ct: constant-time equality")
        W(LIB, "src/ct.rs", R(LIB, "src/ct.rs") + "\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn equal() {\n        assert!(super::eq(b\"ab\", b\"ab\"));\n    }\n}\n")
        commit(LIB, "ct: tests")
        W(LIB, "README.md", R(LIB, "README.md") + "\n## ct\n\n`ct::eq` compares secrets without leaking timing.\n")
        commit(LIB, "docs: ct module")
        W(LIB, "src/ct.rs", R(LIB, "src/ct.rs").replace("Constant-time equality.", "Constant-time equality of two byte strings."))
        commit(LIB, "ct: clearer doc comment")
        git("checkout", "-q", "main", cwd=LIB)
    if k == 1:
        git("checkout", "-q", "-b", "feat/base64", cwd=LIB)
        W(LIB, "src/b64.rs", "/// Standard base64, padded.\npub fn encode(_bytes: &[u8]) -> String {\n    todo!()\n}\n")
        commit(LIB, "b64: stub encoder", "bob")
        git("checkout", "-q", "main", cwd=LIB)
lib_pr_ct_head = git("rev-parse", "feature/ct-compare", cwd=LIB)
lib_b64 = git("rev-parse", "feat/base64", cwd=LIB)
git("init", "-q", "--bare", LIB_BARE, cwd=REPOS)
git("push", "-q", "--no-verify", LIB_BARE, "refs/heads/*:refs/heads/*", cwd=LIB)
git("remote", "add", "origin", f"{URL}/acme/lib.git", cwd=LIB)
git("fetch", "-q", LIB_BARE, "+refs/heads/*:refs/remotes/origin/*", cwd=LIB)
git("symbolic-ref", "refs/remotes/origin/HEAD", "refs/remotes/origin/main", cwd=LIB)
git("branch", "-q", "--set-upstream-to=origin/main", "main", cwd=LIB)
git("branch", "-q", "-D", "feat/base64", cwd=LIB)

# ---------------------------------------------------------------------------------------------------------------------
# acme/app: ~300 commits, merges, tags, a submodule, LFS.
# ---------------------------------------------------------------------------------------------------------------------
log("acme/app: history")
APP = os.path.join(REPOS, "app")
APP_BARE = os.path.join(FORGE, "acme", "app.git")
git("init", "-q", APP, cwd=REPOS)
subprocess.run([GIT, "lfs", "install", "--local"], cwd=APP, env=ENV, check=True, stdout=subprocess.DEVNULL)
for p, (header, _) in CODE.items():
    W(APP, p, header)
for p, t in STATIC.items():
    W(APP, p, t)
W(APP, "db/migrations/0001_init.sql", "-- 0001_init.sql\nBEGIN;\n" + schema_sql(SCHEMA, INDEXES).split("\n\n", 1)[1] + "COMMIT;\n")
W(APP, "db/schema.sql", schema_sql(SCHEMA, INDEXES))
W(APP, "assets/logo.svg", svg("#3b6fd8", "acme"))
W(APP, "assets/icon.png", png(64, 64, "icon-1"))
W(APP, "assets/fonts/inter.woff2", blob("font-1", 8192, b"wOF2"))
W(APP, "assets/design/hero.psd", blob("hero-1", 65536, b"8BPS"))
git("-c", "protocol.file.allow=always", "submodule", "add", "-q", LIB_BARE, "vendor/acme-lib", cwd=APP)
SUB = os.path.join(APP, "vendor/acme-lib")
git("checkout", "-q", lib_commits[4], cwd=SUB)
git("config", "-f", ".gitmodules", "submodule.vendor/acme-lib.url", f"{URL}/acme/lib.git", cwd=APP)
git("submodule", "sync", "-q", cwd=APP)
git("remote", "set-url", "origin", f"{URL}/acme/lib.git", cwd=SUB)
commit(APP, "Initial import")

weights = ["alice"] * 5 + ["bob"] * 4 + ["carol"] * 3 + ["dave"] * 2 + ["erin", "frank", "xavier"]
code_paths = list(CODE)
pulls = []   # PR records for state.json, numbered in the order they were opened
MERGED = [("feat/csv-invoices", "Export invoices as CSV", "carol", ["tools/report.py", "web/src/util.js"]),
          ("feat/session-refresh", "Refresh sessions before they expire", "bob", ["ios/App/SessionStore.swift", "android/app/src/main/java/dev/acme/app/Session.java"]),
          ("fix/hash-seed", "Seed the native hash per process", "dave", ["native/hash.c"]),
          ("feat/api-pagination", "Paginate list endpoints", "alice", ["src/server/router.rs", "web/src/api.ts"]),
          ("chore/clippy-fixes", "Fix clippy warnings", "erin", ["src/server/db.rs"]),
          ("feat/invoice-reminders", "Send invoice reminders", "bob", ["services/billing/invoice.go", "services/billing/main.go"]),
          ("feat/web-store", "Client-side user store", "carol", ["web/src/store.ts"]),
          ("fix/android-crash", "Fix crash on rotate", "frank", ["android/app/src/main/java/dev/acme/app/MainActivity.java"]),
          ("feat/vec-stats", "Stats on native vectors", "dave", ["native/vec.cpp"]),
          ("feat/buttons", "Secondary button variants", "carol", ["web/src/components/Button.tsx"]),
          ("feat/migrate-dry-run", "Dry run for migrations", "alice", ["tools/migrate.py"]),
          ("fix/ios-timeouts", "Longer request timeouts on iOS", "bob", ["ios/App/APIClient.swift"]),
          ("feat/audit-trail", "Audit trail for admin actions", "erin", ["src/server/db.rs", "src/server/router.rs"]),
          ("perf/billing-batch", "Batch invoice totals", "dave", ["services/billing/main.go"])]
CLOSED = [("spike/graphql", "Spike: GraphQL endpoint", "frank", ["src/server/router.rs"]),
          ("feat/old-dashboard", "Old dashboard (superseded)", "carol", ["web/src/components/Button.tsx"]),
          ("fix/flaky-test", "Retry the flaky billing test", "erin", ["services/billing/invoice.go"]),
          ("feat/sms-login", "Log in with SMS", "bob", ["ios/App/SessionStore.swift"]),
          ("chore/drop-java8", "Drop Java 8 support", "frank", ["android/app/src/main/java/dev/acme/app/Session.java"]),
          ("feat/xml-export", "XML export", "dave", ["tools/report.py"])]
merged_q, closed_q, mig_q = list(MERGED), list(CLOSED), list(MIGRATIONS)
tag_no = 1
for i in range(220):
    who = rnd.choice(weights)
    r = rnd.random()
    if i % 37 == 18 and mig_q:
        m = mig_q.pop(0)
        apply_migration(APP, m, SCHEMA, INDEXES)
        commit(APP, f"db: migration {m[0][:-4]}\n\nAdds `{m[0]}` and updates `db/schema.sql`.\n\n- reviewed by **dave**\n- run `tools/migrate.py` after pulling", who)
    elif i in (30, 70, 90, 170):   # a file with 5+ commits and a rename in its history (file history, blame)
        p = "docs/intro.md" if i > 120 else "docs/overview.md"
        W(APP, p, R(APP, p) + f"\nUpdate {i}: {rnd.choice(VERBS)} {rnd.choice(NOUNS)}s.\n")
        commit(APP, f"docs: overview, part {i}", who)
    elif i == 120:
        git("mv", "docs/overview.md", "docs/intro.md", cwd=APP)
        commit(APP, "docs: rename overview.md to intro.md", "erin")
    elif i in (110, 180):   # LFS-tracked PNGs and a .bin; 180 changes banner.png (its old object is dropped locally)
        W(APP, "assets/lfs/banner.png", png(200, 80, f"banner-{i}"))
        if i == 110:
            W(APP, "assets/lfs/old.png", png(48, 48, "old"))
            W(APP, "assets/lfs/model.bin", blob("model", 32768, b"ACME"))
        commit(APP, "assets: LFS banner" + (", old icon and model" if i == 110 else " refresh"), "carol")
    elif i == 60:
        W(APP, "web/src/generated/schema.gen.ts", generated_ts(False))
        commit(APP, "codegen: generate API schema types", "renovate[bot]")
    elif i in (45, 150):
        W(APP, "assets/icon.png", png(64, 64, f"icon-{i}"))
        if i == 150:
            W(APP, "assets/logo.svg", svg("#d8573b", "acme"))
        commit(APP, "assets: refresh the app icon" + (" and logo" if i == 150 else ""), "carol")
    elif i == 100:
        W(APP, "assets/design/hero.psd", blob("hero-2", 65536, b"8BPS"))
        W(APP, "assets/fonts/inter.woff2", blob("font-2", 8192, b"wOF2"))
        commit(APP, "assets: new hero artwork and font subset", "carol")
    elif i == 130:
        git("checkout", "-q", lib_commits[5], cwd=SUB)
        commit(APP, "vendor: bump acme-lib to 0.3.1", "alice")
    elif r < 0.55:
        p = rnd.choice(code_paths)
        commit(APP, f"{area(p)}: add {add_fn(APP, p)}", who)
    elif r < 0.75:
        p = rnd.choice(code_paths)
        if not tweak(APP, p):
            add_fn(APP, p)
        commit(APP, f"{area(p)}: tune limits in {os.path.basename(p)}", who)
    elif r < 0.88:
        changelog(APP, f"{rnd.choice(VERBS).title()} {rnd.choice(NOUNS)} handling")
        commit(APP, "changelog", who)
    else:
        tweak(APP, "config/app.yaml")
        commit(APP, "config: adjust defaults", who)
    if i % 15 == 7 and merged_q:   # a feature branch, merged as a pull request
        branch, title, author, files = merged_q.pop(0)
        git("checkout", "-q", "-b", branch, cwd=APP)
        opened = CLOCK
        for f in files + files[:1]:
            commit(APP, f"{area(f)}: add {add_fn(APP, f)}", author)
        head = git("rev-parse", "HEAD", cwd=APP)
        git("checkout", "-q", "main", cwd=APP)
        changelog(APP, title)
        commit(APP, "changelog", rnd.choice(weights))
        base = git("rev-parse", "HEAD", cwd=APP)
        n = len(pulls) + 1
        tick(1, 5)
        git("merge", "-q", "--no-ff", "-m", f"Merge pull request '{title}' (#{n}) from {branch} into main", branch, cwd=APP, who="alice")
        pulls.append(dict(repo="acme/app", number=n, title=title, user=author, head=branch, head_sha=head, base="main", base_sha=base,
                          state="closed", merged=True, merge_commit_sha=git("rev-parse", "HEAD", cwd=APP), created_at=iso(opened),
                          merged_at=iso(), closed_at=iso(), ci="success", keep_branch=n % 4 == 0,
                          body=f"{title}.\n\n- touches `{'`, `'.join(files)}`\n", reviews=[dict(user="bob" if author != "bob" else "carol", state="APPROVED", body="")]))
    if i % 30 == 22 and closed_q:   # a branch whose pull request was closed unmerged
        branch, title, author, files = closed_q.pop(0)
        git("checkout", "-q", "-b", branch, cwd=APP)
        opened = CLOCK
        for f in files:
            commit(APP, f"{area(f)}: add {add_fn(APP, f)}", author)
        git("checkout", "-q", "main", cwd=APP)
        base = git("rev-parse", "main", cwd=APP)
        tick()
        pulls.append(dict(repo="acme/app", number=len(pulls) + 1, title=title, user=author, head=branch, head_sha=git("rev-parse", branch, cwd=APP),
                          base="main", base_sha=base, state="closed", merged=False, created_at=iso(opened), closed_at=iso(), ci="failure",
                          keep_branch=len(pulls) % 2 == 0, body=f"{title}.", issue_comments=[dict(user="alice", body="Closing: we went another way.")]))
    if i % 36 == 35:
        tick(1, 2)
        git("tag", "-a", f"v0.{tag_no}.0", "-m", f"Release v0.{tag_no}.0", cwd=APP, who="alice")
        tag_no += 1
git("tag", "v1.0.0-rc.1", cwd=APP)   # lightweight
tick()
git("tag", "-a", "v1.0.0", "-m", "Release v1.0.0\n\nFirst stable release.", cwd=APP, who="alice")
M = git("rev-parse", "main", cwd=APP)
log(f"  main has {git('rev-list', '--count', 'main', cwd=APP)} commits, {len(pulls)} closed pull requests")

# ---------------------------------------------------------------------------------------------------------------------
# Open pull requests: branches off main (four of them become linked worktrees).
# ---------------------------------------------------------------------------------------------------------------------
log("acme/app: open pull request branches")
clock_at("2026-10-20T09:00")
TICK = (1, 6)


def branch(name, base, author, steps):
    """steps: (message, [ops]); an op is a path (add a function), ("tweak", p), ("write", p, text), ("rm", p), ("mv", a, b)."""
    git("checkout", "-q", "-B", name, base, cwd=APP)
    shas = []
    for msg, ops in steps:
        for op in ops:
            if isinstance(op, str):
                add_fn(APP, op)
            elif op[0] == "tweak":
                tweak(APP, op[1]) or add_fn(APP, op[1])
            elif op[0] == "write":
                W(APP, op[1], op[2])
            elif op[0] == "rm":
                git("rm", "-q", op[1], cwd=APP)
            elif op[0] == "sub":
                W(APP, op[1], R(APP, op[1]).replace(op[2], op[3], 1))
            elif op[0] == "mv":
                os.makedirs(os.path.dirname(os.path.join(APP, op[2])), exist_ok=True)
                git("mv", op[1], op[2], cwd=APP)
        shas.append(commit(APP, msg, author))
    git("checkout", "-q", "main", cwd=APP)
    return shas


# The medium pull request: ~40 files across every language, written out so review comments can point at lines.
def go_file(name, typ):
    return f"""package payments

import (
	"errors"
	"fmt"
)

// {typ} is part of the payments API.
type {typ} struct {{
	ID       string
	Amount   int64
	Currency string
}}

var Err{typ}Invalid = errors.New("payments: invalid {name}")

// New{typ} validates and builds a {name}.
func New{typ}(id string, amount int64, currency string) (*{typ}, error) {{
	if amount <= 0 || len(currency) != 3 {{
		return nil, Err{typ}Invalid
	}}
	return &{typ}{{ID: id, Amount: amount, Currency: currency}}, nil
}}

func (x *{typ}) String() string {{
	return fmt.Sprintf("{name} %s %d %s", x.ID, x.Amount, x.Currency)
}}
"""


def ts_file(name):
    return f"""import {{ getJSON }} from "../api";

export interface {name.title()} {{
  id: string;
  amount: number;
  currency: string;
}}

export async function fetch{name.title()}(id: string): Promise<{name.title()}> {{
  return getJSON<{name.title()}>("/payments/{name}s/" + id);
}}
"""


SEED = STATIC["dev/test_data/seed.sql"]
PAY_SQL = """-- 0008_payments.sql: payments, their states, and the bookkeeping around them.
BEGIN;

CREATE TYPE payment_status AS ENUM ('pending', 'paid', 'failed');
ALTER TYPE payment_status ADD VALUE 'refunded';

CREATE TABLE payments (
    id            TEXT PRIMARY KEY,
    invoice_id    INTEGER NOT NULL REFERENCES invoices(id) ON DELETE CASCADE,
    customer_id   INTEGER NOT NULL REFERENCES users(id),
    cents         INTEGER NOT NULL CHECK (cents > 0 AND cents < 100000000 AND (currency <> 'JPY' OR cents % 100 = 0)),
    currency      TEXT NOT NULL,
    status        payment_status NOT NULL DEFAULT 'pending',
    provider      TEXT NOT NULL DEFAULT 'stripe',
    provider_ref  TEXT,
    attempts      INTEGER NOT NULL DEFAULT 0,
    last_error    TEXT,
    created_at    TIMESTAMP NOT NULL DEFAULT now(),
    updated_at    TIMESTAMP NOT NULL DEFAULT now(),
    paid_at       TIMESTAMP,
    UNIQUE (provider, provider_ref)
);

ALTER TABLE invoices ADD COLUMN paid_at TIMESTAMP;
ALTER TABLE invoices ADD COLUMN IF NOT EXISTS payment_id TEXT;
ALTER TABLE invoices ALTER COLUMN cents TYPE BIGINT USING cents::bigint;
ALTER TABLE invoices ALTER COLUMN status SET DEFAULT 'open';
ALTER TABLE invoices RENAME COLUMN customer_id TO user_id;
ALTER TABLE users ADD COLUMN country TEXT NOT NULL;
ALTER TABLE audit_log DROP COLUMN actor_id;
DROP TABLE orders;

CREATE INDEX payments_invoice ON payments(invoice_id);
CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS payments_ref ON payments(provider_ref);

CREATE OR REPLACE FUNCTION touch_updated_at() RETURNS trigger AS $$
BEGIN
    NEW.updated_at := now();
    IF NEW.cents <= 0 THEN
        RAISE EXCEPTION 'cents must be positive' USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER payments_touch BEFORE UPDATE OF cents, status ON payments
    FOR EACH ROW EXECUTE FUNCTION touch_updated_at();

CREATE MATERIALIZED VIEW payments_daily AS
    SELECT date_trunc('day', paid_at) AS day, currency, SUM(cents) AS cents FROM payments GROUP BY 1, 2;

UPDATE invoices SET status = 'open';
COMMIT;
"""
PAY = []
PAY.append(("payments: charge and refund types", [("write", f"services/payments/{n}.go", go_file(n, n.title())) for n in ("charge", "refund", "payout", "dispute")]))
PAY.append(("payments: ledger, currency and webhook handling", [("write", f"services/payments/{n}.go", go_file(n, n.title())) for n in ("ledger", "currency", "webhook", "client")]
            + [("write", "services/payments/charge_test.go", 'package payments\n\nimport "testing"\n\nfunc TestChargeRounding(t *testing.T) {\n\tc, err := NewCharge("ch_1", 1999, "EUR")\n\tif err != nil || c.Amount != 2000 {\n\t\tt.Fatalf("got %v, want 2000", c)\n\t}\n}\n')]))
PAY.append(("server: /payments routes", [
    ("write", "src/server/payments.rs", "//! Payment endpoints.\nuse crate::db::{fetch_user, Pool};\nuse crate::router::Request;\n\n/// Routes /payments/*.\npub fn handle(pool: &Pool, req: &Request) -> Result<String, String> {\n    let user = fetch_user(pool, req.user_id).ok_or(\"unknown user\")?;\n    let amount: i64 = req.path.trim_start_matches(\"/payments/\").parse().map_err(|_| \"bad amount\")?;\n    Ok(format!(\"charged {} cents to {}\", amount, user.email))\n}\n"),
    ("write", "src/server/main.rs", STATIC["src/server/main.rs"].replace("mod db;\n", "mod db;\nmod payments;\n")),
    "src/server/db.rs"]))
PAY.append(("web: payment form", [("write", f"web/src/payments/{n}.ts", ts_file(n)) for n in ("charge", "refund", "receipt")]
            + [("write", "web/src/payments/PaymentForm.tsx", 'import React, { useState } from "react";\nimport { Button } from "../components/Button";\nimport { fetchCharge } from "./charge";\n\nexport function PaymentForm({ id }: { id: string }) {\n  const [amount, setAmount] = useState(0);\n  return (\n    <form>\n      <input type="number" value={amount} onChange={(e) => setAmount(Number(e.target.value))} />\n      <Button label="Pay" onClick={() => fetchCharge(id)} />\n    </form>\n  );\n}\n'),
               ("write", "web/src/payments/format.ts", 'export function formatAmount(cents: number, currency: string): string {\n  // TODO: use Intl.NumberFormat\n  // FIXME: negative amounts\n  const value: string = cents / 100;\n  return value + " " + currency;\n}\n'),
               ("write", "web/src/payments/legacyFormat.ts", '// formatAmount is defined twice on purpose: go-to-definition offers a picker.\nexport function formatAmount(cents: number): string {\n  // @ts-ignore legacy\n  return String(cents);\n}\n'
                + "".join(f"// TODO({k}): drop with the legacy client\n" for k in range(8)) + 'it.only("legacy", () => {});\nconst note = "TODO inside a string";\n'),
               ("write", "web/src/payments/format.test.ts", 'import { formatAmount } from "./format";\n\ntest("formats euros", () => {\n  expect(formatAmount(1999, "EUR")).toBe("19.99 EUR");\n});\n'),
               "web/src/store.ts", ("mv", "web/src/util.js", "web/src/lib/util.js")]))
PAY.append(("db: payments tables", [
    ("write", "db/migrations/0008_payments.sql", PAY_SQL),
    ("write", "db/migrations/0003_payments_backfill.sql", "-- Reuses version 0003 by mistake (the lens flags it).\nUPDATE invoices SET status = 'paid';\nDELETE FROM sessions WHERE expires_at < now();\n"),
    ("write", "dev/test_data/seed.sql", SEED.replace("INSERT INTO users VALUES (2, 'bob@example.com', 'Bob');\n", "")
                                         .replace("'Carol'", "'Carol Chen'") + "INSERT INTO payments VALUES ('pay_1', 1, 1999, 'EUR', 'paid');\n"),
    ("write", "db/schema.sql", None)]))   # filled in from SCHEMA
PAY.append(("deps and config for payments", [
    ("write", "Cargo.toml", STATIC["Cargo.toml"].replace('tokio = { version = "1.38"', 'tokio = { version = "2.0"').replace('axum = "0.7.5"', 'axum = "0.7.4"')
                                               .replace('anyhow = "1.0.86"', 'anyhow = "1.0.86"\nstripe-rs = "0.31.0"\nrust_decimal = "1.35"')),
    ("write", "web/package.json", STATIC["web/package.json"].replace('"zod": "3.23.8"', '"zod": "3.23.8",\n    "@stripe/stripe-js": "4.1.0",\n    "dinero.js": "2.0.0",\n    "currency.js": "2.0.4"')
                                  .replace('"typescript": "5.4.5"', '"typescript": "5.5.2"').replace('"vite": "5.2.11"', '"vite": "6.0.0"').replace('"vitest": "1.6.0"', '"vitest": "1.5.0",\n    "msw": "2.3.1",\n    "@types/react": "18.3.3",\n    "eslint": "9.5.0",\n    "prettier": "3.3.2"')
                                  .replace('"react-dom": "18.3.1"', '"react-dom": "18.3.2"')),
    ("write", "go.mod", STATIC["go.mod"].replace("\tgolang.org/x/sync v0.7.0\n", "\tgolang.org/x/sync v0.7.0\n\tgithub.com/stripe/stripe-go/v79 v79.4.0\n")),
    ("write", "requirements-dev.txt", STATIC["requirements-dev.txt"].replace("pytest==8.2.1", "pytest==8.2.2") + "responses==0.25.3\n"),
    ("write", "config/app.toml", STATIC["config/app.toml"] + '\n[payments]\nprovider = "stripe"\ncurrency = "EUR"\n\n[payments.webhooks]\npath = "/payments/webhook"\nsecret_env = "STRIPE_WEBHOOK_SECRET"\n'),
    ("write", "config/.env.example", STATIC["config/.env.example"].replace("api_token=changeme", "api_token=sk_test_replace_me\nSTRIPE_WEBHOOK_SECRET=whsec_replace_me")),
    ("write", "config/app.json", '{\n  "server": { "port": 8443, "cors": ["https://acme.example", "https://pay.acme.example"] },\n  "features": { "darkMode": false, "csvExport": true },\n'
                                  '  "payments": { "provider": "stripe", "currency": "EUR", "minCents": 50, "maxCents": 1000000, "retries": 3 }\n}\n'),
]))
PAY.append(("mobile and native: payment sheet, checksum", [
    ("write", "ios/App/PaymentSheet.swift", 'import SwiftUI\n\nstruct PaymentSheet: View {\n    var amount: Int\n    var onPay: () -> Void\n\n    var body: some View {\n        VStack(spacing: 12) {\n            Text("Pay \\(Double(amount) / 100, format: .currency(code: "EUR"))")\n            Button("Pay", action: onPay).buttonStyle(.borderedProminent)\n        }\n        .padding()\n    }\n}\n'),
    "ios/App/APIClient.swift", "ios/App/SessionStore.swift",
    ("write", "android/app/src/main/java/dev/acme/app/Payment.java", 'package dev.acme.app;\n\npublic record Payment(String id, long cents, String currency) {\n    public Payment {\n        if (cents <= 0) throw new IllegalArgumentException("cents must be positive");\n    }\n}\n'),
    "android/app/src/main/java/dev/acme/app/Session.java",
    ("write", "native/checksum.c", '#include "hash.h"\n\n/* Luhn check for card numbers. */\nint acme_luhn(const char *digits, size_t len) {\n    int sum = 0;\n    for (size_t i = 0; i < len; i++) {\n        int d = digits[len - 1 - i] - \'0\';\n        if (i % 2) { d *= 2; if (d > 9) d -= 9; }\n        sum += d;\n    }\n    return sum % 10 == 0;\n}\n'),
    ("write", "native/hash.h", STATIC["native/hash.h"].replace("#endif", "int acme_luhn(const char *digits, size_t len);\n\n#endif")),
    "native/vec.cpp"]))
PAY.append(("tools, docs, config and CI for payments", [
    ("write", "tools/payments_report.py", '"""Payments per currency."""\nfrom migrate import connect\n\n\ndef by_currency(conn):\n    return dict(conn.execute("SELECT currency, SUM(cents) FROM payments GROUP BY currency"))\n'),
    ("write", "tests/payments_test.py", 'from tools.payments_report import by_currency\n\n\ndef test_empty(conn):\n    assert by_currency(conn) == {}\n'),
    "tools/migrate.py", ("rm", "tools/legacy_export.py"),
    ("write", "docs/payments.md", "# Payments\n\nCharges go through `services/payments`. A charge is **pending** until the provider confirms it.\n\n```sh\ncurl -X POST localhost:8080/payments/1999\n```\n"),
    ("tweak", "config/app.yaml"), ("tweak", "docker-compose.yml"),
    ("write", "assets/payments/card.svg", svg("#2f9e44", "pay")), ("write", "assets/payments/card.png", png(96, 64, "card"))]))
PAY.append(("Address review: route /payments, healthz alias", []))   # alice; router changes written below

# Router edits for the medium PR need the current router text, so they're applied as a write at branch time.
def pay_branch():
    git("checkout", "-q", "-B", "feat/payments-api", M, cwd=APP)
    sch, idx = {k: list(v) for k, v in SCHEMA.items()}, list(INDEXES)
    sch["payments"] = ["id          TEXT PRIMARY KEY", "invoice_id  INTEGER NOT NULL REFERENCES invoices(id)", "cents       INTEGER NOT NULL",
                       "currency    TEXT NOT NULL", "status      TEXT NOT NULL DEFAULT 'pending'"]
    sch["invoices"].append("paid_at     TIMESTAMP")
    idx.append("CREATE INDEX payments_invoice ON payments(invoice_id);")
    shas = []
    for k, (msg, ops) in enumerate(PAY):
        for op in ops:
            if isinstance(op, str):
                add_fn(APP, op)
            elif op[0] == "write":
                W(APP, op[1], op[2] if op[2] is not None else schema_sql(sch, idx))
            elif op[0] == "tweak":
                tweak(APP, op[1])
            elif op[0] == "rm":
                git("rm", "-q", op[1], cwd=APP)
            elif op[0] == "mv":
                os.makedirs(os.path.dirname(os.path.join(APP, op[2])), exist_ok=True)
                git("mv", op[1], op[2], cwd=APP)
        if k == len(PAY) - 1:
            t = R(APP, "src/server/router.rs").replace('"/health" => Ok("ok".into()),', '"/health" | "/healthz" => Ok("ok".into()),\n        p if p.starts_with("/payments/") => crate::payments::handle(pool, req),')
            W(APP, "src/server/router.rs", t)
            changelog(APP, "Payments API")
            W(APP, "README.md", R(APP, "README.md").replace("| `db` |", "| `services/payments` | payments API |\n| `db` |"))
        shas.append(commit(APP, msg, "alice" if k == len(PAY) - 1 else "bob"))
    git("checkout", "-q", "main", cwd=APP)
    return shas


# Worktree branches.
search = branch("feature/search-index", M, "alice", [
    ("tools: build a search index of invoices", [("write", "tools/search_index.py", '"""Builds the invoice search index."""\nfrom migrate import connect\n\n\ndef build(conn):\n    rows = conn.execute("SELECT id, customer_id FROM invoices").fetchall()\n    return {cid: [i for i, c in rows if c == cid] for _, cid in rows}\n')]),
    ("web: search box", [("write", "web/src/search.ts", 'import { getJSON } from "./api";\n\nexport async function search(q: string): Promise<number[]> {\n  return getJSON<number[]>("/search?q=" + encodeURIComponent(q));\n}\n')]),
    ("docs: search", [("write", "docs/search.md", "# Search\n\n`tools/search_index.py` builds the index nightly.\n")])])
billing = branch("feature/billing-v2", M, "alice", [
    ("billing: v2 invoice totals", ["services/billing/invoice.go"]),
    ("billing: worker reads v2 totals", ["services/billing/main.go"])])
# feature/sessions rewrites db.rs lines that main also rewrites (by hand, for an exact conflict), adds the file main
# also adds, and deletes the one main modifies.
git("checkout", "-q", "-B", "feature/sessions", M, cwd=APP)
db = R(APP, "src/server/db.rs")
W(APP, "src/server/db.rs", _re.sub(r"pub const MAX_CONNECTIONS: usize = \d+;", "pub const MAX_CONNECTIONS: usize = 32;\npub const SESSION_TTL_SECS: u64 = 3600;", db)
  .replace("    pool.users.get(&id).cloned()\n", "    let user = pool.users.get(&id)?;\n    Some(user.clone())\n"))
sessions = [commit(APP, "sessions: keep the session TTL next to the pool")]
W(APP, "config/feature-flags.yaml", "flags:\n  sessions_v2: true\n  sessions_ttl_secs: 3600\n")
sessions.append(commit(APP, "sessions: feature flags for the rollout"))
git("rm", "-q", "tools/report.py", cwd=APP)
sessions.append(commit(APP, "sessions: drop report.py, the dashboard replaces it"))
sessions.append(commit(APP, f"ios: add {add_fn(APP, 'ios/App/SessionStore.swift')}"))
W(APP, "assets/icon.png", png(64, 64, "icon-sessions"))
sessions.append(commit(APP, "assets: session-themed icon"))
git("checkout", "-q", "main", cwd=APP)
router = branch("feature/router-cleanup", M, "alice", [
    ("router: split the health check into its own handler", ["src/server/router.rs"]),
    ("router: return typed errors\n\nErrors were strings everywhere. Now:\n\n- `NotFound` for unknown routes\n- `Unauthorized` for a missing user\n\nNo behaviour change for clients.", [("sub", "src/server/router.rs", '.ok_or_else(|| "not found".into())', '.ok_or_else(|| "user not found".into())')]),
    ("fixup! router: split the health check into its own handler", ["src/server/router.rs"]),
    ("router: log every request", ["src/server/router.rs"]),
    ("WIP", [("tweak", "src/server/main.rs")]),
    ("router: table-driven routes", ["src/server/router.rs"]),
    ("squash! router: table-driven routes", [("sub", "src/server/router.rs", '.ok_or_else(|| "user not found".into())', '.ok_or_else(|| "no such user".into())')]),
    ("router: remove the dead /legacy route", ["src/server/router.rs"]),
    ("tests: cover the route table", [("write", "tests/router_test.rs", "#[test]\nfn health() {\n    assert_eq!(2 + 2, 4);\n}\n")]),
    ("router: tidy imports", ["src/server/db.rs"])])
pay = pay_branch()
others = {   # branch: (author, steps, base)
    "fix/login-timeout": ("carol", [("ios: retry login once on timeout", ["ios/App/APIClient.swift"]), ("server: shorter login timeout", [("tweak", "config/app.yaml")])], M),
    "feat/dark-mode": ("dave", [("web: dark mode tokens", ["web/src/components/Button.tsx"]), ("ios: dark mode", ["ios/App/SessionStore.swift"])], M),
    "chore/deps-bump": ("renovate[bot]", [("chore(deps): update react to 18.3.2", [("write", "web/package.json", STATIC["web/package.json"].replace("18.3.1", "18.3.2"))])], f"{M}~30"),
    "docs/api-reference": ("erin", [("docs: API reference", [("write", "docs/api.md", "# API reference\n\n## GET /me\n\nThe signed-in user.\n\n## GET /health\n\n`ok`.\n")])], M),
    "perf/query-cache": ("bob", [("server: cache user lookups", ["src/server/db.rs"]), ("server: cache invalidation", ["src/server/db.rs"])], M),
    "feat/csv-export": ("carol", [("tools: CSV export of payments", ["tools/report.py"])], M),
    "fix/null-invoice-total": ("dave", [("billing: guard against nil totals", ["services/billing/invoice.go"])], M),
    "feat/webhooks": ("erin", [("server: webhook delivery", ["src/server/router.rs"]), ("billing: webhook on paid", ["services/billing/main.go"])], M),
    "ci/matrix-builds": ("frank", [("ci: build on a matrix", [("write", ".forgejo/workflows/ci.yml", STATIC[".forgejo/workflows/ci.yml"].replace("  build:\n    runs-on: docker\n", "  build:\n    runs-on: docker\n    strategy:\n      matrix:\n        target: [linux, macos]\n"))])], M),
    "refactor/error-types": ("bob", [("server: one error type", ["src/server/router.rs"]), ("native: error codes", ["native/hash.c"])], M),
    "feat/i18n-strings": ("grace", [("android: extract strings", ["android/app/src/main/java/dev/acme/app/MainActivity.java"])], M),
}
heads = {b: branch(b, base, who, steps) for b, (who, steps, base) in others.items()}
# A local-only branch, never pushed (needsPush); a branch squash-merged on the forge (Merged badge); a hotfix branch
# that a cherry-pick will stop on; a fork's commit served only as refs/pull/N/head.
branch("spike/sqlite-fts", M, "alice", [("spike: sqlite FTS for search", [("write", "tools/fts.py", '"""FTS5 spike."""\n')])])
readme = branch("fix/readme-wording", M, "alice", [("docs: clearer README heading", [("sub", "README.md", "## Running\n", "## Running locally\n")])])
branch("hotfix/pool", M, "dave", [("db: pool of 20 for the hotfix", [("write", "src/server/db.rs", _re.sub(r"pub const MAX_CONNECTIONS: usize = \d+;", "pub const MAX_CONNECTIONS: usize = 20;", R(APP, "src/server/db.rs")))])])
fork = branch("fork-tmp", M, "frank", [("search: index customer names too", [("write", "docs/search.md", "# Search (fork)\n\nIndexes customer names too.\n")])])[0]

# ---------------------------------------------------------------------------------------------------------------------
# The forge: push everything, then others push to main and to billing-v2 (fetched), and one more (not yet fetched).
# ---------------------------------------------------------------------------------------------------------------------
log("forge: push")
git("init", "-q", "--bare", APP_BARE, cwd=REPOS)
pushed = ["main", "feature/search-index", "feature/billing-v2", "feature/sessions", "feat/payments-api", *others]
pushed += [p["head"] for p in pulls if p["keep_branch"]]
git("push", "-q", "--no-verify", APP_BARE, *[f"refs/heads/{b}:refs/heads/{b}" for b in pushed], "refs/tags/*:refs/tags/*", cwd=APP)
git("push", "-q", "--no-verify", APP_BARE, f"{router[3]}:refs/heads/feature/router-cleanup", cwd=APP)   # 4 of its 10 commits
git("push", "-q", "--no-verify", APP_BARE, *[f"{p['head_sha']}:refs/pull/{p['number']}/head" for p in pulls], cwd=APP)
git("push", "-q", "--no-verify", APP_BARE, f"{fork}:refs/forks/frank/feature/search-index", cwd=APP)
for b in [p["head"] for p in pulls] + list(others) + ["feat/payments-api", "fork-tmp"]:
    git("branch", "-q", "-D", b, cwd=APP)   # local only knows them as origin/<b>

SCRATCH = os.path.join(OUT, "scratch")
shutil.rmtree(SCRATCH, ignore_errors=True)
git("clone", "-q", APP_BARE, SCRATCH, cwd=OUT, env={"GIT_LFS_SKIP_SMUDGE": "1"})   # LFS objects aren't in the forge yet
clock_at("2026-11-18T10:00")
d = R(SCRATCH, "src/server/db.rs")
W(SCRATCH, "src/server/db.rs", _re.sub(r"pub const MAX_CONNECTIONS: usize = \d+;", "pub const MAX_CONNECTIONS: usize = 24;", d)
  .replace("    pool.users.get(&id).cloned()\n", "    let found = pool.users.get(&id).cloned();\n    if found.is_none() {\n        eprintln!(\"user {id} not found\");\n    }\n    found\n"))
pool_sha = commit(SCRATCH, "db: raise the pool size and log missed lookups", "carol")
W(SCRATCH, "config/feature-flags.yaml", "flags:\n  payments_v2: false\n  payments_rollout_percent: 10\n")
commit(SCRATCH, "config: feature flags for the payments rollout", "bob")
W(SCRATCH, "tools/report.py", R(SCRATCH, "tools/report.py").replace('GROUP BY customer"))', 'GROUP BY customer, currency"))'))
commit(SCRATCH, "tools: report totals per currency", "carol")
W(SCRATCH, "assets/icon.png", png(64, 64, "icon-main"))   # binary conflict with feature/sessions
commit(SCRATCH, "assets: holiday icon", "carol")
W(SCRATCH, "README.md", R(SCRATCH, "README.md").replace("## Running\n", "## Running locally\n"))   # squash of fix/readme-wording
SQUASH_PR = len(pulls) + 1
opened_at = iso()
squash_sha = commit(SCRATCH, f"Fix README wording (#{SQUASH_PR})", "alice")
pulls.append(dict(repo="acme/app", number=SQUASH_PR, title="Fix README wording", user="alice", head="fix/readme-wording", head_sha=readme[0],
                  base="main", base_sha=git("rev-parse", "HEAD~1", cwd=SCRATCH), state="closed", merged=True, merge_commit_sha=squash_sha,
                  created_at=opened_at, merged_at=iso(), closed_at=iso(), ci="success", body="Squash-merged; the branch is gone on the forge."))
git("push", "-q", "--no-verify", APP_BARE, f"{readme[0]}:refs/pull/{SQUASH_PR}/head", cwd=APP)
git("push", "-q", "--no-verify", "origin", "main", cwd=SCRATCH)
git("checkout", "-q", "feature/billing-v2", cwd=SCRATCH)
commit(SCRATCH, f"billing: add {add_fn(SCRATCH, 'services/billing/invoice.go')}", "bob")
git("push", "-q", "--no-verify", "origin", "feature/billing-v2", cwd=SCRATCH)
blob_id = subprocess.run([GIT, "hash-object", "-w", "--stdin"], cwd=APP_BARE, env=ENV, input=b"<h1>acme docs</h1>\n", capture_output=True, check=True).stdout.decode().strip()
tree_id = subprocess.run([GIT, "mktree"], cwd=APP_BARE, env=ENV, input=f"100644 blob {blob_id}\tindex.html\n".encode(), capture_output=True, check=True).stdout.decode().strip()
parent = []
for k in range(120):
    parent = ["-p", git("commit-tree", tree_id, *parent, "-m", f"Publish docs #{k + 1}", cwd=APP_BARE, who="renovate[bot]")]
git("update-ref", "refs/heads/gh-pages", parent[1], cwd=APP_BARE)
git("remote", "add", "origin", f"{URL}/acme/app.git", cwd=APP)
git("fetch", "-q", "--prune", APP_BARE, "+refs/heads/*:refs/remotes/origin/*", cwd=APP)
git("symbolic-ref", "refs/remotes/origin/HEAD", "refs/remotes/origin/main", cwd=APP)
git("checkout", "-q", "main", cwd=SCRATCH)
W(SCRATCH, "Cargo.toml", R(SCRATCH, "Cargo.toml").replace('version = "0.1.0"', 'version = "1.0.1"'))
changelog(SCRATCH, "1.0.1: pool size and per-currency reports")
commit(SCRATCH, "Release 1.0.1", "dave")
git("push", "-q", "--no-verify", "origin", "main", cwd=SCRATCH)   # not fetched: a fetch in the app brings it
shutil.rmtree(SCRATCH)

# Local main: two commits not pushed (ahead 2), three of origin's not merged (behind 3).
clock_at("2026-11-21T09:00")
changelog(APP, "Document the release checklist")
commit(APP, "docs: release checklist in the changelog")
W(APP, "docs/architecture.md", R(APP, "docs/architecture.md") + "- Payments go through `services/payments` (see docs/payments.md once it lands).\n")
commit(APP, "docs: mention payments in the architecture notes")
for b in ("main", "feature/search-index", "feature/billing-v2", "feature/sessions", "feature/router-cleanup"):
    git("branch", "-q", f"--set-upstream-to=origin/{b}", b, cwd=APP)
git("config", "branch.fix/readme-wording.remote", "origin", cwd=APP)   # its upstream is gone on the forge
git("config", "branch.fix/readme-wording.merge", "refs/heads/fix/readme-wording", cwd=APP)
git("branch", "-q", "feature/fresh", "main", cwd=APP)   # cut from main, no commits of its own
git("remote", "add", "mirror", "https://git.example.invalid/acme/app.git", cwd=APP)   # a second remote with no account

# ---------------------------------------------------------------------------------------------------------------------
# Worktrees.
# ---------------------------------------------------------------------------------------------------------------------
log("worktrees")
WT = {k: os.path.join(REPOS, f"app-{k}") for k in ("search", "billing", "sessions", "router", "merged", "fresh", "spike", "cherry", "v0.3")}
for k, b in (("search", "feature/search-index"), ("billing", "feature/billing-v2"), ("sessions", "feature/sessions"), ("router", "feature/router-cleanup"),
             ("merged", "fix/readme-wording"), ("fresh", "feature/fresh"), ("spike", "spike/sqlite-fts"), ("cherry", "hotfix/pool")):
    git("worktree", "add", "-q", WT[k], b, cwd=APP)
git("worktree", "add", "-q", "--detach", WT["v0.3"], "v0.3.0", cwd=APP)
git("worktree", "lock", "--reason", "on the release laptop", WT["search"], cwd=APP)
WT["nested"] = os.path.join(APP, ".worktree", "scratch")   # a linked worktree inside the main one (git-ignored)
git("worktree", "add", "-q", "-b", "scratch", WT["nested"], "main", cwd=APP)
gone = os.path.join(REPOS, "app-gone")   # deleted with rm -rf: prunable
git("worktree", "add", "-q", "-b", "gone", gone, "main", cwd=APP)
shutil.rmtree(gone)
os.makedirs(os.path.join(REPOS, "app-existing"))   # a destination folder that's already taken
git("cherry-pick", pool_sha, cwd=WT["cherry"], check=False)   # stops on db.rs
with open(os.path.join(REPOS, "app", ".git", "hooks", "pre-commit"), "w") as f:   # fails only where .fail-pre-commit exists
    f.write('#!/bin/sh\nif [ -e .fail-pre-commit ]; then echo "pre-commit: refusing to commit (remove .fail-pre-commit)" >&2; exit 1; fi\n')
os.chmod(os.path.join(REPOS, "app", ".git", "hooks", "pre-commit"), 0o755)
W(WT["spike"], ".fail-pre-commit", "")

# Dirty: staged, unstaged, both, untracked, deleted (staged and not), renamed, mode change, 20k-line file, images, binary, LFS, SQL.
B = WT["billing"]
add_fn(B, "services/billing/invoice.go")
W(B, "db/migrations/0007_refunds.sql", "-- 0007_refunds.sql\nBEGIN;\nCREATE TABLE refunds (\n    id          INTEGER PRIMARY KEY,\n    invoice_id  INTEGER NOT NULL REFERENCES invoices(id),\n    cents       INTEGER NOT NULL,\n    reason      TEXT\n);\nALTER TABLE invoices ADD COLUMN refunded_cents INTEGER NOT NULL DEFAULT 0;\nCOMMIT;\n")
sch = {k: list(v) for k, v in SCHEMA.items()}
sch["invoices"].append("refunded_cents INTEGER NOT NULL DEFAULT 0")
sch["refunds"] = ["id          INTEGER PRIMARY KEY", "invoice_id  INTEGER NOT NULL REFERENCES invoices(id)", "cents       INTEGER NOT NULL", "reason      TEXT"]
W(B, "db/schema.sql", schema_sql(sch, INDEXES))
W(B, "assets/fonts/inter.woff2", blob("font-3", 8192, b"wOF2"))
W(B, "services/billing/main.go", R(B, "services/billing/main.go").replace('log.Printf("skip %s: %v"', 'log.Printf("billing v2: skip %s: %v"'))
os.makedirs(os.path.join(B, "docs/billing"))
git("mv", "docs/billing.md", "docs/billing/overview.md", cwd=B)
W(B, "docs/billing/overview.md", R(B, "docs/billing/overview.md").replace("finalized by the billing worker", "finalized by the v2 billing worker"))
git("rm", "-q", "tools/legacy_export.py", cwd=B)
git("add", "services/billing/invoice.go", "db/migrations/0007_refunds.sql", "db/schema.sql", "assets/fonts/inter.woff2", "services/billing/main.go", "docs/billing/overview.md", cwd=B)
add_fn(B, "services/billing/main.go")   # partially staged: its first hunk is staged, this one isn't
for p in ("src/server/router.rs", "web/src/api.ts", "native/vec.cpp", "android/app/src/main/java/dev/acme/app/Session.java"):
    add_fn(B, p)
tweak(B, "config/app.yaml")
W(B, "web/src/generated/schema.gen.ts", generated_ts(True))
W(B, "assets/icon.png", png(64, 64, "icon-dirty"))
W(B, "assets/design/hero.psd", blob("hero-dirty", 65536, b"8BPS"))
os.chmod(os.path.join(B, "tools/report.py"), 0o755)
os.remove(os.path.join(B, "docker-compose.yml"))
W(B, "notes/billing-todo.md", "# TODO\n\n- [ ] refunds API\n- [x] migration\n- [ ] backfill `refunded_cents`\n")
W(B, "web/src/components/InvoiceTable.tsx", 'import React from "react";\n\nexport function InvoiceTable({ rows }: { rows: { id: number; cents: number }[] }) {\n  return (\n    <table>\n      <tbody>\n        {rows.map((r) => (\n          <tr key={r.id}><td>{r.id}</td><td>{r.cents}</td></tr>\n        ))}\n      </tbody>\n    </table>\n  );\n}\n')
W(B, "assets/badge.svg", svg("#7048e8", "v2"))
W(B, "assets/screens/checkout.png", png(320, 200, "checkout"))
os.remove(os.path.join(B, "docs/architecture.md"))
os.symlink("../README.md", os.path.join(B, "docs/architecture.md"))   # type change: file → symlink
W(B, "go.mod", R(B, "go.mod").rstrip("\n"))   # no newline at end of file (new side)
W(B, ".gitignore", R(B, ".gitignore").replace("*.db\n", "*.db\r\n"))   # one line LF → CRLF
W(B, "web/public/vendor.min.js", R(B, "web/public/vendor.min.js").replace("return a*1234+b", "return a*4321+b"))   # one 100 KB line
W(B, "notes/empty.txt", "")
W(B, "assets/lfs/banner.png", png(200, 80, "banner-dirty"))   # LFS: modified, added, deleted
W(B, "assets/lfs/new.png", png(48, 48, "new"))
os.remove(os.path.join(B, "assets/lfs/old.png"))

# Merge stopped on conflicts: content (db.rs), add/add (feature-flags.yaml), delete/modify (report.py).
git("merge", "origin/main", cwd=WT["sessions"], check=False)

# Lib: a linked worktree stopped at an "edit" step of an interactive rebase onto main.
LIBWT = os.path.join(REPOS, "lib-ct")
git("worktree", "add", "-q", LIBWT, "feature/ct-compare", cwd=LIB)
git("branch", "-q", "--set-upstream-to=origin/feature/ct-compare", "feature/ct-compare", cwd=LIB)
tick()
git("rebase", "-i", "main", cwd=LIBWT, env={"GIT_SEQUENCE_EDITOR": "sed -i '' -e '2s/^pick/edit/' -e '4s/^pick/fixup/'"})

# Main worktree: three stashes and the submodule moved ahead of the recorded commit.
tick()
W(APP, "src/server/router.rs", R(APP, "src/server/router.rs").replace('"/health" => Ok("ok".into()),', '"/health" => Ok("ok".into()),\n        "/ready" => Ok("ready".into()),'))
git("stash", "push", "-q", "-m", "router: readiness probe, half done", cwd=APP)
tick()
W(APP, "tools/fts_spike.py", '"""Trying SQLite FTS5 for invoice search."""\nimport sqlite3\n')
W(APP, "README.md", R(APP, "README.md") + "\n## Search\n\nSee tools/fts_spike.py.\n")
git("stash", "push", "-q", "-u", "-m", "spike: sqlite fts (with untracked files)", cwd=APP)
tick()
changelog(APP, "Faster invoice totals")
git("add", "CHANGELOG.md", cwd=APP)
W(APP, "config/app.yaml", _re.sub(r"timeout_secs: \d+", "timeout_secs: 45", R(APP, "config/app.yaml")))
git("stash", "push", "-q", "-m", "changelog + timeout, staged and unstaged", cwd=APP)
git("checkout", "-q", lib_commits[-1], cwd=SUB)
W(APP, "config/app.yaml", _re.sub(r"timeout_secs: \d+", "timeout_secs: 60", R(APP, "config/app.yaml")))   # conflicts with stash@{0}
W(APP, ".direnv/flake-profile", "ignored churn\n")   # git-ignored folder: file events there must not refresh
W(WT["nested"], "notes.md", "scratch notes\n")
git("reset", "-q", "--hard", "main~2", cwd=LIB)   # lib main: behind origin/main by 2, fast-forwardable
git("config", f"lfs.{URL}/acme/lib.git/info/lfs.locksverify", "false", cwd=LIB)

# ---------------------------------------------------------------------------------------------------------------------
# Forge data: open pull requests, reviews, comments, CI profiles; refs/pull/N/head; LFS objects.
# ---------------------------------------------------------------------------------------------------------------------
log("forge: pull requests")
for p in pulls:
    p.pop("keep_branch", None)
n0 = len(pulls)


def opened(head, user, title, ci, created, **kw):
    pr = dict(repo="acme/app", number=len(pulls) + 1, title=title, user=user, head=head, base="main", state="open", merged=False,
              ci=ci, created_at=created, body=kw.pop("body", f"{title}."), **kw)
    pulls.append(pr)
    return pr["number"]


MD_ALL = ("# Heading 1\n## Heading 2\n### Heading 3\n\nParagraph with **bold**, *italic*, ~~strike~~, `code`, a [link](https://example.com) and an autolink https://acme.example.\n\n"
          "> A quote\n> > nested\n\n- bullet\n  - nested bullet\n1. one\n2. two\n\n- [x] done\n- [ ] todo\n\n| a | b |\n| --- | :-: |\n| 1 | 2 |\n\n"
          "```diff\n- old\n+ new\n```\n\n---\n\n<details><summary>Collapsed</summary>\n\nHidden text.\n</details>\n\n![image](http://127.0.0.1/avatars/bob.png)\n\n@alice @bob :tada:\n")
CODE_BLOCK = "```rust\npub fn fetch_user(pool: &Pool, id: u64) -> Option<User> {\n    pool.users.get(&id).cloned()\n}\n```"
roles = {}
roles["thisBranch"] = opened("feature/search-index", "alice", "Invoice search index", "success", "2026-10-21T10:00:00Z",
                             body="## What\n\nA nightly search index of invoices, and a search box.\n\n## Why\n\nSupport asks for it daily.\n\n## Checklist\n\n- [x] Tests added\n- [ ] Migration reviewed\n- [x] Changelog updated\n",
                             reviews=[dict(user="bob", state="APPROVED", body="Nice and small. :+1:")], labels=["enhancement"], requested_reviewers=["bob"])
roles["dirtyWorktree"] = opened("feature/billing-v2", "alice", "Billing v2: invoice totals and refunds", "running", "2026-10-23T11:00:00Z",
                                body="Billing v2. Refunds still to come (see the dirty worktree).", labels=["billing"], requested_reviewers=["carol"],
                                issue_comments=[dict(user="bob", body="Pushed one more commit with an invoice helper; pull before you continue.")])
roles["conflictWorktree"] = opened("feature/sessions", "alice", "Sessions v2: TTL next to the pool", "failure", "2026-10-25T09:30:00Z",
                                   body="Moves the session TTL next to the pool and drops `tools/report.py`.",
                                   reviews=[dict(user="carol", state="REQUEST_CHANGES", body="`report.py` is still used by finance. Please keep it.")])
roles["rebaseWorktree"] = opened("feature/router-cleanup", "alice", "WIP: Router cleanup", "queued", "2026-10-27T15:00:00Z", draft=True,
                                 body="Draft: splitting the router. Commits will be squashed before review.")
roles["mediumPR"] = opened("feat/payments-api", "bob", "Payments API", "failure+external", "2026-11-02T09:00:00Z",
                           labels=["enhancement", "payments", "needs-review"], assignees=["bob", "alice"], requested_reviewers=["alice", "erin"],
                           body="## What\n\nA payments API: Go service, `/payments` routes, web form, iOS sheet, Android model, a Luhn check, "
                                "and the `payments` table.\n\n## Why\n\nCustomers pay invoices outside the app today.\n\n## How to test\n\n"
                                "```sh\ngo test ./services/payments/...\ncurl -X POST localhost:8080/payments/1999\n```\n\n"
                                "| Area | Files |\n| --- | --- |\n| Go | 9 |\n| Web | 7 |\n| SQL | 2 |\n\n- [x] Tests added\n- [ ] Migration reviewed\n- [x] Changelog updated\n",
                           issue_comments=[
                               dict(user="dave", created_at="2026-11-03T09:00:00Z", body="Before this lands: what happens to an invoice when a payment is **disputed**? The schema has no state for it."),
                               dict(user="bob", created_at="2026-11-03T09:40:00Z", body="Good point. Disputes are `services/payments/dispute.go`; I'd keep them out of `invoices` for now:\n\n```go\nd, err := NewDispute(\"dp_1\", 1999, \"EUR\")\n```\n\nand add a `disputed_at` later."),
                               dict(user="erin", created_at="2026-11-04T14:00:00Z", body="> add a `disputed_at` later\n\nPlease make it a follow-up issue, otherwise it gets lost.\n\n- [ ] follow-up issue\n- [ ] docs"),
                               dict(user="alice", created_at="2026-11-06T10:00:00Z", body="CI fails in `test`: `charge_test.go` expects rounding that `NewCharge` doesn't do. See the job log, line with `charge_test.go:7`."),
                               dict(user="bob", created_at="2026-11-06T10:10:00Z", body="Ugh, yes. Fix incoming. Meanwhile here's the current lookup for reference:\n\n" + CODE_BLOCK),
                               dict(user="bob", created_at="2026-11-06T10:50:00Z", body="(40 minutes later, so a separate card.) Pushed the fix."),
                               dict(user="erin", created_at="2026-11-07T08:00:00Z", body=MD_ALL),
                           ],
                           reviews=[
                               dict(user="carol", state="COMMENT", body="First pass. A few questions inline.", submitted_at="2026-11-03T12:00:00Z", comments=[
                                   dict(path="services/payments/charge.go", side="new", match="func NewCharge", outdated=True, body="(on the first push) Name it `CreateCharge`?"),
                                   dict(path="services/payments/charge.go", side="new", match="if amount <= 0", body="Should zero be allowed for free trials?"),
                                   dict(path="src/server/router.rs", side="old", match='"/health" => Ok', body="Why drop the plain `/health` arm? Monitoring still calls it."),
                                   dict(path="web/src/payments/PaymentForm.tsx", side="new", match="useState(0)", body="Start empty instead of `0`, so the placeholder shows.")]),
                               dict(user="bob", state="COMMENT", body="", comments=[
                                   dict(path="services/payments/charge.go", side="new", match="if amount <= 0", body="No: trials don't create charges at all."),
                                   dict(path="src/server/router.rs", side="old", match='"/health" => Ok', body="It isn't dropped: `/health | /healthz` matches both. Updated in the last commit.")]),
                               dict(user="dave", state="REQUEST_CHANGES", body="The migration needs an index on `payments.status`, and `currency` should be a `CHAR(3)` with a check.\n\n```sql\nCREATE INDEX payments_status ON payments(status);\n```", comments=[
                                   dict(path="db/migrations/0008_payments.sql", side="new", match="currency      TEXT NOT NULL", body="`CHAR(3) NOT NULL CHECK (length(currency) = 3)`"),
                                   dict(path="db/schema.sql", side="new", match="status      TEXT NOT NULL DEFAULT 'pending'", body="And mirror the index here."),
                                   dict(path="web/src/payments/format.ts", side="new", match="const value: string", body="This is the TS2322 in CI: `cents / 100` is a number.")]),
                               dict(user="alice", state="COMMENT", body="", comments=[
                                   dict(path="db/schema.sql", side="new", match="status      TEXT NOT NULL DEFAULT 'pending'", body="+1, and `schema.sql` is checked in CI against the migrations.")]),
                               dict(user="carol", state="APPROVED", body="LGTM once dave's migration comments are in."),
                           ])
PD07 = [dict(user="alice", state="APPROVED", body="", id_hint=1), dict(user="bob", state="REQUEST_CHANGES", body="Let's keep `String` errors at the edges."),
        dict(user="carol", state="APPROVED", body="", dismissed=True), dict(user="dave", state="APPROVED", body="", stale=True),
        dict(user="alice", state="REQUEST_REVIEW", body="")]
for b, title, ci, extra in [("fix/login-timeout", "Retry login once on timeout", "success", {}),
                            ("feat/dark-mode", "Dark mode", "mixed", {"draft": True, "title_prefix": "WIP: "}),
                            ("chore/deps-bump", "chore(deps): update react to 18.3.2", "success+hugelog", {"labels": ["dependencies"]}),
                            ("docs/api-reference", "API reference", None, {"labels": ["docs"]}),
                            ("perf/query-cache", "Cache user lookups", "failure+nolog", {}),
                            ("feat/csv-export", "CSV export of payments", "success", {"reviews": [dict(user="alice", state="APPROVED", body="Ship it.")]}),
                            ("fix/null-invoice-total", "Guard against nil invoice totals", "success", {"body": ""}),
                            ("feat/webhooks", "Webhooks on paid invoices", "queued", {}),
                            ("ci/matrix-builds", "Build on a linux/macos matrix", "matrix", {"mergeable": None}),
                            ("refactor/error-types", "One error type for the server", "success", {"reviews": PD07, "requested_reviewers": ["erin", "alice"]}),
                            ("feat/i18n-strings", "Extract Android strings", "skipped", {"draft": True, "title_prefix": "WIP: ", "user": "Alice"})]:
    who = extra.pop("user", others[b][0])
    tp = extra.pop("title_prefix", "")
    opened(b, who, tp + title, ci, f"2026-11-{rnd.randint(3, 17):02d}T{rnd.randint(8, 18):02d}:00:00Z",
           issue_comments=[dict(user=rnd.choice(["alice", "bob", "carol"]), body=f"Thanks! Looking at `{b}` today.")] if who != "alice" else [], **extra)
roles["fork"] = opened("feature/search-index", "frank", "Index customer names too", "success", "2026-11-12T16:00:00Z", head_repo_id=99,
                       head_sha=fork, body="From frank's fork: same branch name as ours, different repository.")
roles["prDirTaken"] = next(p["number"] for p in pulls if p["head"] == "fix/login-timeout")
os.makedirs(os.path.join(REPOS, f"app-pr-{roles['prDirTaken']}"))   # checkout-in-worktree's destination already on disk
roles["mine"] = [p["number"] for p in pulls if p["state"] == "open" and p["user"] == "alice"]
lib_pulls = [dict(repo="acme/lib", number=1, title="Hex: faster encode with a lookup table", user="bob", head="feat/hex-table", head_sha=lib_commits[6],
                  base="main", base_sha=lib_commits[5], state="closed", merged=True, merge_commit_sha=lib_commits[6], created_at="2026-02-01T10:00:00Z",
                  merged_at="2026-02-02T10:00:00Z", closed_at="2026-02-02T10:00:00Z", ci="success", body="Lookup table."),
             dict(repo="acme/lib", number=2, title="Base64 helpers", user="bob", head="feat/base64", base="main", state="open", merged=False,
                  ci="success", created_at="2026-11-05T10:00:00Z", body="Stub for now."),
             dict(repo="acme/lib", number=3, title="Constant-time compare", user="alice", head="feature/ct-compare", base="main", state="open",
                  merged=False, ci="failure", created_at="2026-11-06T10:00:00Z", body="`ct::eq`, for token checks in the server."),
             dict(repo="acme/lib", number=4, title="Drop the hex module", user="frank", head="drop-hex", head_sha=lib_commits[2], base="main",
                  base_sha=lib_commits[1], state="closed", merged=False, ci=None, created_at="2026-03-01T10:00:00Z", closed_at="2026-03-02T10:00:00Z", body="No.")]
for p in pulls:
    if p["state"] == "open" and "head_repo_id" not in p:
        p["head_sha"] = git("rev-parse", f"refs/heads/{p['head']}", cwd=APP_BARE)
    if p["state"] == "open":
        git("update-ref", f"refs/pull/{p['number']}/head", p["head_sha"], cwd=APP_BARE)
for p in lib_pulls:
    if "head_sha" not in p:
        p["head_sha"] = git("rev-parse", f"refs/heads/{p['head']}", cwd=LIB_BARE)
    git("update-ref", f"refs/pull/{p['number']}/head", p["head_sha"], cwd=LIB_BARE)
for bare in (APP_BARE, LIB_BARE):
    git("config", "http.receivepack", "true", cwd=bare)
    git("symbolic-ref", "HEAD", "refs/heads/main", cwd=bare)
LFSDIR = os.path.join(FORGE, "lfs")
os.makedirs(LFSDIR)
for root, _, files in os.walk(os.path.join(APP, ".git", "lfs", "objects")):
    for f in files:
        shutil.copy(os.path.join(root, f), os.path.join(LFSDIR, f))
old_oid = hashlib.sha256(png(200, 80, "banner-110")).hexdigest()   # objects present for one version, missing for the other
os.remove(os.path.join(APP, ".git", "lfs", "objects", old_oid[:2], old_oid[2:4], old_oid))

# ---------------------------------------------------------------------------------------------------------------------
# acme/infra: a third repo whose remote host has no account (no PRs, no CI), with k8s, Helm and CRD YAML for the lens.
# ---------------------------------------------------------------------------------------------------------------------
log("acme/infra")
INFRA = os.path.join(REPOS, "infra")
git("init", "-q", INFRA, cwd=REPOS)
DEPLOY = """apiVersion: apps/v1
kind: Deployment
metadata:
  name: acme-server
spec:
  replicas: 2
  template:
    spec:
      containers:
        - name: server
          image: registry.example/acme/server:1.0.0
          ports: [{ containerPort: 8080 }]
        - name: sidecar
          image: registry.example/acme/proxy:2.1.0
---
apiVersion: v1
kind: Service
metadata:
  name: acme-server
spec:
  selector: { app: acme-server }
  ports:
    - port: 80
      targetPort: 8080
"""
CRD = """apiVersion: apiextensions.k8s.io/v1
kind: CustomResourceDefinition
metadata:
  name: invoices.acme.example
spec:
  group: acme.example
  versions:
    - name: v1
      served: true
      storage: true
      schema:
        openAPIV3Schema:
          type: object
          properties:
            spec:
              type: object
              properties:
                cents: { type: integer }
"""
W(INFRA, "k8s/server.yaml", DEPLOY)
W(INFRA, "k8s/rbac.yaml", "apiVersion: rbac.authorization.k8s.io/v1\nkind: Role\nmetadata:\n  name: reader\nrules:\n  - apiGroups: [\"\"]\n    resources: [pods]\n    verbs: [get, list]\n")
W(INFRA, "helm/acme/templates/configmap.yaml", "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: {{ .Release.Name }}-config\ndata:\n  motd: |\n    Welcome to acme.\n    Be nice.\n  summary: >\n    folded\n    text\n")
W(INFRA, "crds/invoices.yaml", CRD)
W(INFRA, "crds/payments.yaml", CRD.replace("invoices", "payments").replace("cents: { type: integer }", "amount: { type: integer }"))
W(INFRA, "README.md", "# acme/infra\n\nDeployment manifests. Its remote has no Loupe account.\n")
commit(INFRA, "Initial manifests", "frank")
W(INFRA, "k8s/server.yaml", DEPLOY.replace("replicas: 2", "replicas: 3").replace("server:1.0.0", "server:1.0.1"))
for c in ("crds/invoices.yaml", "crds/payments.yaml"):   # the same new field in both, one with a type change too
    t = R(INFRA, c).replace("              properties:\n", "              required: [currency]\n              properties:\n                currency: { type: string }\n", 1)
    W(INFRA, c, t.replace("amount: { type: integer }", "amount: { type: string }"))
commit(INFRA, "server 1.0.1, three replicas; currency on invoices and payments", "frank")
git("remote", "add", "origin", "https://git.example.invalid/acme/infra.git", cwd=INFRA)
W(INFRA, "k8s/rbac.yaml", R(INFRA, "k8s/rbac.yaml").replace("verbs: [get, list]", "verbs: [get, list, watch]"))

users = {k: {"full_name": v[0], "email": v[1]} for k, v in PEOPLE.items() if k != "xavier"}
users["Alice"] = dict(users["alice"], email="alice.work@example.com")   # the same person, another case
users["frank"]["avatar"] = "404"   # avatar_url that 404s
users["grace"]["avatar"] = None    # no avatar_url
state = {"me": "alice", "token": "flows-token", "users": users,
         "lfs_locks": {"acme/app": [dict(path="assets/design/hero.psd", owner="bob"), dict(path="assets/lfs/banner.png", owner="carol")]},
         "repos": {"acme/app": {"id": 1, "description": "The acme billing platform", "pulls": pulls},
                   "acme/lib": {"id": 2, "description": "Small helpers shared by acme services", "pulls": lib_pulls}}}
with open(os.path.join(FORGE, "state.json"), "w") as f:
    json.dump(state, f, indent=1)

# ---------------------------------------------------------------------------------------------------------------------
# Extra conflict repos (merge editor, MG-01..26) and a stub AI CLI.
# ---------------------------------------------------------------------------------------------------------------------
log("extra conflict repos")
subprocess.run(["sh", os.path.join(os.path.dirname(os.path.abspath(__file__)), "merge-fixture.sh"), OUT], env=ENV, check=True, stdout=subprocess.DEVNULL)
EXTRA = {n: os.path.join(REPOS, "extra", n) for n in ("merge-plain", "merge-diff3", "rebase", "cherry", "revert", "resolved", "outside", "special")}
# A fake Claude Code: a fenced merge for a conflict prompt, a commit message / PR text otherwise, nothing at all when
# <out>/ai-empty exists. Every call goes to <out>/ai-calls.log. /opt/homebrew/bin/claude is found before it, so the GPUI
# app takes it through LOUPE_FLOWS_AI=<this path> (loupe_forge::ai, scenario runs only); the Swift app can't be pointed.
AI = os.path.join(HOME, ".local", "bin", "claude")
os.makedirs(os.path.dirname(AI), exist_ok=True)
with open(AI, "w") as f:
    f.write(f"""#!/bin/sh
# Stub Claude Code for the flows fixture (scripts/flows/mkflows.py).
echo "$0 $*" >> '{OUT}/ai-calls.log'
p=$(cat)
[ -f '{OUT}/ai-empty' ] && exit 0
case "$p" in
  *onflict*) printf '```\\n  merged by AI: both changes\\n```\\n' ;;
  *) printf 'Tidy the flows fixture\\n\\nWritten by the stub AI of the flows fixture.\\n' ;;
esac
""")
os.chmod(AI, 0o755)

worktrees = [APP, *WT.values(), LIB, LIBWT, INFRA]
big_commit = git("log", "-1", "--format=%H", "--", "web/src/generated/schema.gen.ts", cwd=APP)
scenario = {
    "forge": URL, "token": "flows-token", "user": "alice",
    "repos": [APP, LIB, INFRA], "worktrees": worktrees,
    # StressDriver.Scenario's required fields; with steps [] only repos, forge, token, user and big (the selection) matter.
    "big": APP, "conflict": WT["sessions"], "submodule": APP, "stashes": APP,
    "bigFileWorktree": WT["billing"], "bigFile": "web/src/generated/schema.gen.ts", "minifiedFile": "web/public/vendor.min.js",
    "hugeCommit": big_commit, "hugePR": roles["mediumPR"], "prs": [p["number"] for p in pulls if p["state"] == "open"],
    "iterations": 1, "pause": 0.5, "logSeconds": 5, "background": False, "steps": [],
    "fixture": {
        "home": HOME, "forgeDir": FORGE,
        "worktrees": {"main": APP, "clean": WT["search"], "dirty": WT["billing"], "merge": WT["sessions"], "unpushed": WT["router"],
                      "squashMerged": WT["merged"], "fresh": WT["fresh"], "localOnly": WT["spike"], "cherryPick": WT["cherry"],
                      "detached": WT["v0.3"], "nested": WT["nested"], "locked": WT["search"], "prunable": gone,
                      "lib": LIB, "rebase": LIBWT, "noAccount": INFRA},
        "takenPaths": [os.path.join(REPOS, "app-existing"), os.path.join(REPOS, f"app-pr-{roles['prDirTaken']}")],
        "pulls": {"acme/app": {**roles, "open": [p["number"] for p in pulls if p["state"] == "open"],
                               "closed": [p["number"] for p in pulls if p["state"] == "closed"]},
                  "acme/lib": {"open": [2, 3], "closed": [1, 4]}},
        "hugeCommit": "the commit that added the 20k-line generated file",
        "extra": EXTRA,   # not in "repos": add them there (or open them) to check the merge editor
        "ai": AI,         # export LOUPE_FLOWS_AI=<this> for the GPUI app
    },
}
with open(os.path.join(OUT, "scenario.json"), "w") as f:
    json.dump(scenario, f, indent=2)
log(f"done: {OUT}/scenario.json  (app main {git('rev-parse', '--short', 'main', cwd=APP)}, "
    f"{git('rev-list', '--count', '--all', cwd=APP)} commits, {len(pulls)} app PRs)")
