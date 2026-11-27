#!/usr/bin/env python3
"""The flows mock forge: a Forgejo API v1 (plus git smart HTTP and the LFS API) over the fixture mkflows.py built.

    scripts/flows/mockforge-flows.py <out> [--port 8790] [--version 17.2.0+gitea-1.22.0] [--log-seconds 60]
                                           [--ignore-range] [--more-open N] [--more-closed N] [--verbose]

Serves <out>/forge: bare repos acme/app.git and acme/lib.git, pull requests and reviews from state.json, and CI
computed per head sha. Everything Loupe writes changes server state and the repos: PR create, review submit, comment
post, merge (a real merge, rebase or squash in the served repo), CI rerun/cancel, LFS locks, git push. State lives in
memory (restart = back to state.json) except the repos, which only mkflows.py resets.

Test controls (not Forgejo):
    GET    /_mock                          state summary
    POST   /_mock/fault  {"method": "POST", "path": "regex", "status": 500, "body": {...} or "text", "times": 1}
    DELETE /_mock/fault                    clear faults
    POST   /_mock/version {"version": "1.26.0"}   what GET /api/v1/version answers ("500" makes it fail)
    POST   /_mock/pr {"repo": "acme/app", "number": 26, "title": "..."}   edit a PR's fields (polling tests)
Unknown endpoints answer 404 and are logged to stderr.
"""
import argparse, gzip, hashlib, json, os, re, struct, subprocess, sys, threading, time, zlib
from http.server import ThreadingHTTPServer, BaseHTTPRequestHandler
from urllib.parse import urlsplit, parse_qs, quote, unquote

GIT = "/opt/homebrew/bin/git"
ap = argparse.ArgumentParser()
ap.add_argument("out")
ap.add_argument("--port", type=int, default=8790)
ap.add_argument("--version", default="17.2.0+gitea-1.22.0", help="GET /api/v1/version; Forgejo 17+ shows rerun and cancel")
ap.add_argument("--log-seconds", type=float, default=60, help="how long a running job runs (its log grows meanwhile)")
ap.add_argument("--rerun-seconds", type=float, default=30, help="how long a rerun job runs before it succeeds")
ap.add_argument("--log-rate", type=float, default=15, help="log lines per second of a running job")
ap.add_argument("--ignore-range", action="store_true", help="serve logs whole (200), as a server that ignores Range")
ap.add_argument("--more-open", type=int, default=0, help="extra filler open PRs on acme/app (paging)")
ap.add_argument("--more-closed", type=int, default=0, help="extra filler closed PRs on acme/app (paging)")
ap.add_argument("--verbose", action="store_true", help="log every request")
args = ap.parse_args()
OUT = os.path.abspath(args.out)
FORGE = os.path.join(OUT, "forge")
BASE = f"http://127.0.0.1:{args.port}"
STATE = json.load(open(os.path.join(FORGE, "state.json")))
ME, TOKEN = STATE["me"], STATE["token"]
LOCK = threading.RLock()   # ponytail: one lock for all state; fine at human scale
VERSION = args.version
FAULTS = []


def now_iso():
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())


def parse_iso(s):
    return int(time.mktime(time.strptime(s, "%Y-%m-%dT%H:%M:%SZ")) - time.timezone)


def iso(t):
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(t))


def bare(repo):
    return os.path.join(FORGE, repo + ".git")


def git(repo, *a, check=True, env=None):
    r = subprocess.run([GIT, *a], cwd=bare(repo), stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                       env=dict(os.environ, HOME=os.path.join(OUT, "home"), GIT_CONFIG_NOSYSTEM="1", **(env or {})))
    if check and r.returncode:
        raise RuntimeError(f"git {' '.join(a)}: {r.stderr.decode()[:500]}")
    return r.stdout.decode()


def heads(repo):
    out = git(repo, "for-each-ref", "--format=%(objectname) %(refname)", "refs/heads")
    return {l.split(" ", 1)[1][11:]: l.split(" ", 1)[0] for l in out.splitlines()}


# ---------------------------------------------------------------------------------------------------------------------
# Users
# ---------------------------------------------------------------------------------------------------------------------
USERS = STATE["users"]
EMAIL = {u["email"]: login for login, u in USERS.items() if login == login.lower() or login not in USERS}


def user(login):
    if login is None:
        return None
    u = USERS.get(login, {"full_name": login, "email": f"{login}@example.com"})
    av = u.get("avatar", "ok")
    url = None if av is None else f"{BASE}/avatars/{'missing/' if av == '404' else ''}{quote(login)}.png"
    return {"id": zlib.crc32(login.encode()) % 100000, "login": login, "full_name": u["full_name"], "email": u["email"], "avatar_url": url}


def png(name):
    h = zlib.crc32(name.encode())
    r, g, b = h % 200 + 30, h // 7 % 200 + 30, h // 49 % 200 + 30
    raw = b"".join(b"\x00" + bytes([r, g, b]) * 64 for _ in range(64))
    chunk = lambda t, d: struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d) & 0xffffffff)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 64, 64, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b"")


# ---------------------------------------------------------------------------------------------------------------------
# Pull requests
# ---------------------------------------------------------------------------------------------------------------------
REPOS = STATE["repos"]
_ids = {"review": 1000, "comment": 50000, "issue": 9000}


def next_id(kind):
    _ids[kind] += 1
    return _ids[kind]


for name, r in REPOS.items():
    for p in r["pulls"]:
        p["initial_head"] = p["head_sha"]
        p["issue_comments"] = [dict(c, id=next_id("issue")) for c in p.get("issue_comments", [])]
        p["reviews"] = [dict(v, id=next_id("review"), comments=[dict(c, id=next_id("comment")) for c in v.get("comments", [])])
                        for v in p.get("reviews", [])]
app = REPOS["acme/app"]["pulls"]
for k in range(args.more_open + args.more_closed):
    src = app[k % 20]
    n = max(p["number"] for p in app) + 1
    app.append(dict(src, number=n, title=f"Filler {k + 1}", head=f"filler/{k + 1}", state="open" if k < args.more_open else "closed",
                    merged=False, issue_comments=[], reviews=[], initial_head=src["head_sha"], ci=None))


def find_pr(repo, n):
    return next((p for p in REPOS.get(repo, {}).get("pulls", []) if p["number"] == n), None)


_cache = {}


def cached(key, fn):
    if key not in _cache:
        _cache[key] = fn()
    return _cache[key]


def shas(repo, p, refs=None):
    """(base, head) of a PR: an open one follows its branches (a push moves it); a closed one is frozen."""
    if p["state"] != "open":
        return p["base_sha"], p["head_sha"]
    refs = refs if refs is not None else heads(repo)
    if p.get("head_repo_id") is None and p["head"] in refs and refs[p["head"]] != p["head_sha"]:
        p["head_sha"] = refs[p["head"]]   # pushed: the PR moves, refs/pull/N/head follows
        git(repo, "update-ref", f"refs/pull/{p['number']}/head", p["head_sha"])
    return refs.get(p["base"], p.get("base_sha")), p["head_sha"]


def diff(repo, b, h):
    return cached(("diff", repo, b, h), lambda: git(repo, "diff", "--no-color", "--no-ext-diff", "-M", f"{b}...{h}").encode())


def numstat(repo, b, h):
    def f():
        a = d = n = 0
        for l in git(repo, "diff", "--numstat", "-M", f"{b}...{h}").splitlines():
            x, y, _ = l.split("\t", 2)
            n += 1
            a += int(x) if x != "-" else 0
            d += int(y) if y != "-" else 0
        return a, d, n
    return cached(("numstat", repo, b, h), f)


def mergeable(repo, p, b, h):
    if "mergeable" in p:
        return p["mergeable"]
    if p["state"] != "open":
        return False
    return cached(("mergeable", repo, b, h), lambda: subprocess.run([GIT, "merge-tree", "--write-tree", b, h], cwd=bare(repo),
                                                                    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode == 0)


def label(name):
    colors = {"enhancement": "84b6eb", "billing": "fbca04", "payments": "0e8a16", "needs-review": "d93f0b", "docs": "c5def5", "dependencies": "0366d6"}
    return {"id": zlib.crc32(name.encode()) % 1000, "name": name, "color": colors.get(name, "ededed"), "description": ""}


def pr_json(repo, p, refs=None):
    b, h = shas(repo, p, refs)
    a, d, n = numstat(repo, b, h)
    rid = REPOS[repo]["id"]
    hrid = p.get("head_repo_id", rid)
    o = repo.split("/")[0]
    return {"id": rid * 100000 + p["number"], "number": p["number"], "title": p["title"], "body": p.get("body", ""), "state": p["state"],
            "draft": p.get("draft", False), "user": user(p["user"]),
            "head": {"ref": p["head"], "sha": h, "label": f"frank:{p['head']}" if hrid != rid else p["head"], "repo_id": hrid},
            "base": {"ref": p["base"], "sha": b, "label": p["base"], "repo_id": rid},
            "additions": a, "deletions": d, "changed_files": n, "comments": len(p["issue_comments"]),
            "mergeable": mergeable(repo, p, b, h), "merged": p.get("merged", False), "merged_at": p.get("merged_at"),
            "merge_commit_sha": p.get("merge_commit_sha"), "closed_at": p.get("closed_at"),
            "requested_reviewers": [user(u) for u in p.get("requested_reviewers", [])],
            "labels": [label(l) for l in p.get("labels", [])], "assignees": [user(u) for u in p.get("assignees", [])],
            "assignee": user(p["assignees"][0]) if p.get("assignees") else None,
            "html_url": f"{BASE}/{repo}/pulls/{p['number']}", "diff_url": f"{BASE}/{repo}/pulls/{p['number']}.diff",
            "created_at": p.get("created_at"), "updated_at": p.get("updated_at") or p.get("closed_at") or p.get("created_at"), "owner": o}


def pr_commits(repo, p):
    b, h = shas(repo, p)
    out = git(repo, "log", "--reverse", "--format=%H%x1f%an%x1f%ae%x1f%aI%x1f%cn%x1f%cI%x1f%B%x1e", f"{b}..{h}")
    rows = []
    for rec in out.split("\x1e"):
        f = rec.strip("\n").split("\x1f")
        if len(f) < 7:
            continue
        rows.append({"sha": f[0], "html_url": f"{BASE}/{repo}/commit/{f[0]}", "author": user(EMAIL.get(f[2])), "committer": user(EMAIL.get(f[2])),
                     "commit": {"message": f[6].rstrip("\n"), "author": {"name": f[1], "email": f[2], "date": f[3]},
                                "committer": {"name": f[4], "date": f[5]}}})
    return rows


def diff_lines(data, path):
    """[(kind, old, new, text, hunk so far)] for one file of a unified diff."""
    out, cur, old_path, header, old, new, hunk = [], None, None, False, 0, 0, []
    for l in data.decode(errors="replace").split("\n"):
        if l.startswith("diff --git "):
            cur, header, hunk = None, True, []
        elif header and l.startswith("--- "):
            old_path = l[6:] if l.startswith("--- a/") else None
        elif header and l.startswith("+++ "):
            cur = (l[6:] if l.startswith("+++ b/") else None) or old_path
        elif l.startswith("@@"):
            header = False
            m = re.match(r"@@ -(\d+)(?:,\d+)? \+(\d+)", l)
            old, new, hunk = int(m.group(1)), int(m.group(2)), [l]
        elif cur == path and hunk and l[:1] in ("+", "-", " "):
            hunk.append(l)
            k = l[0]
            out.append((k, old if k != "+" else None, new if k != "-" else None, l[1:], "\n".join(hunk)))
            old += k != "+"
            new += k != "-"
    return out


def resolve(repo, p, c):
    """A fixture comment gives a path, a side and a substring; the forge finds its line in the PR's diff."""
    if "position" in c:
        return
    b, h = shas(repo, p)
    lines = diff_lines(diff(repo, b, h), c["path"])
    want = "+" if c["side"] == "new" else "-"
    hit = next((x for x in lines if x[0] == want and c["match"] in x[3]), None) or next((x for x in lines if c["match"] in x[3]), None)
    if not hit:
        print(f"fixture comment not found in diff: {c['path']} {c['side']} {c['match']!r}", file=sys.stderr, flush=True)
        hit = lines[0] if lines else (" ", 1, 1, "", "")
    c["position"] = hit[2] if c["side"] == "new" else 0
    c["original_position"] = hit[1] if c["side"] == "old" else 0
    c["diff_hunk"] = hit[4]
    if c.get("outdated"):
        c["commit_id"] = git(repo, "rev-list", "--reverse", f"{b}..{h}").split("\n")[0]


def review_time(p, i, v):
    return v.get("submitted_at") or iso(parse_iso(p["created_at"]) + (i + 1) * 5400)


def reviews_json(repo, p):
    _, h = shas(repo, p)
    return [{"id": v["id"], "user": user(v["user"]), "state": v["state"], "body": v.get("body", ""), "commit_id": v.get("commit_id", h),
             "stale": v.get("stale", False), "dismissed": v.get("dismissed", False), "official": True, "comments_count": len(v["comments"]),
             "submitted_at": review_time(p, i, v), "html_url": f"{BASE}/{repo}/pulls/{p['number']}#pullrequestreview-{v['id']}"}
            for i, v in enumerate(p["reviews"])]


def review_comments_json(repo, p, rid):
    _, h = shas(repo, p)
    for i, v in enumerate(p["reviews"]):
        if v["id"] == rid:
            out = []
            for c in v["comments"]:
                resolve(repo, p, c)
                out.append({"id": c["id"], "user": user(c.get("user", v["user"])), "path": c["path"], "body": c["body"],
                            "position": c["position"], "original_position": c["original_position"], "commit_id": c.get("commit_id", h),
                            "original_commit_id": c.get("commit_id", h), "diff_hunk": c.get("diff_hunk"), "pull_request_review_id": rid,
                            "created_at": c.get("created_at") or review_time(p, i, v), "updated_at": review_time(p, i, v),
                            "html_url": f"{BASE}/{repo}/pulls/{p['number']}/files#issuecomment-{c['id']}"})
            return out
    return None


def issue_comments_json(repo, p):
    base = parse_iso(p["created_at"]) if p.get("created_at") else time.time()
    return [{"id": c["id"], "user": user(c["user"]), "body": c["body"], "created_at": c.get("created_at") or iso(base + (i + 1) * 7200),
             "updated_at": c.get("created_at") or iso(base + (i + 1) * 7200), "html_url": f"{BASE}/{repo}/pulls/{p['number']}#issuecomment-{c['id']}"}
            for i, c in enumerate(p["issue_comments"])]


def merge(repo, p, style):
    """What the forge does: a merge commit, a squash, or a rebase of the PR onto the base, in the served repo."""
    b, h = shas(repo, p)
    tree = subprocess.run([GIT, "merge-tree", "--write-tree", b, h], cwd=bare(repo), stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
    if tree.returncode:
        return None
    tree = tree.stdout.decode().split("\n")[0]
    env = {"GIT_AUTHOR_NAME": USERS[ME]["full_name"], "GIT_AUTHOR_EMAIL": USERS[ME]["email"],
           "GIT_COMMITTER_NAME": USERS[ME]["full_name"], "GIT_COMMITTER_EMAIL": USERS[ME]["email"]}
    if style == "squash":
        c = git(repo, "commit-tree", tree, "-p", b, "-m", f"{p['title']} (#{p['number']})", env=env).strip()
    elif style in ("rebase", "rebase-merge", "fast-forward-only"):
        c = b
        for k in git(repo, "rev-list", "--reverse", f"{b}..{h}").split():
            t = subprocess.run([GIT, "merge-tree", "--write-tree", "--merge-base", f"{k}^", c, k], cwd=bare(repo), stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
            if t.returncode:
                return None
            msg = git(repo, "log", "-1", "--format=%B", k)
            a = git(repo, "log", "-1", "--format=%an%x1f%ae%x1f%aI", k).strip().split("\x1f")
            c = git(repo, "commit-tree", t.stdout.decode().split("\n")[0], "-p", c, "-m", msg,
                    env=dict(env, GIT_AUTHOR_NAME=a[0], GIT_AUTHOR_EMAIL=a[1], GIT_AUTHOR_DATE=a[2])).strip()
    else:
        c = git(repo, "commit-tree", tree, "-p", b, "-p", h, "-m", f"Merge pull request '{p['title']}' (#{p['number']}) from {p['head']} into {p['base']}", env=env).strip()
    git(repo, "update-ref", f"refs/heads/{p['base']}", c, b)
    p.update(state="closed", merged=True, merged_at=now_iso(), closed_at=now_iso(), base_sha=b, head_sha=h, merge_commit_sha=c)
    return c


# ---------------------------------------------------------------------------------------------------------------------
# CI: a run per (repo, head sha), its jobs and logs computed from the PR's profile; reruns add attempts.
# ---------------------------------------------------------------------------------------------------------------------
BASIC = {"success": [("lint", "success"), ("build", "success"), ("test", "success"), ("web", "success")],
         "failure": [("lint", "success"), ("build", "success"), ("test", "failure"), ("web", "success")],
         "running": [("lint", "success"), ("build", "success"), ("test", "running"), ("web", "running")],
         "queued": [("lint", "waiting"), ("build", "waiting"), ("test", "waiting"), ("web", "waiting")],
         "skipped": [("lint", "skipped"), ("build", "skipped"), ("test", "skipped"), ("web", "skipped")],
         "mixed": [("lint", "success"), ("build", "cancelled"), ("test", "failure"), ("web", "skipped")]}
EXTERNAL = [("codecov/project", "success", "84.2% (+0.3%) compared to main", "https://codecov.example/acme/app/pull/{n}"),
            ("security/snyk", "failure", "2 high severity vulnerabilities", "https://snyk.example/org/acme/pr/{n}"),
            ("staged-ai-review/summary", "success", "Reviewed the change: 3 suggestions", "https://review.example/acme/app/pulls/{n}"),
            ("deploy/preview", "pending", "Queued", "{BASE}/preview/{n}")]
MIXED = EXTERNAL + [("lint/markdown", "warning", "3 warnings", None), ("docs/build", "skipped", "Skipped: no docs changes", "/acme/app/actions"),
                    ("legacy/jenkins", "error", "Build error", "https://jenkins.example/job/acme/{n}"), ("perf/bench", "pending", "running benchmarks (3/5)", None)]


def profile(name):
    """(jobs [(name, final)], external statuses)"""
    if not name:
        return [], []
    base, _, extra = name.partition("+")
    jobs = list(BASIC.get(base, [])) if base not in ("external", "matrix") else []
    ext = EXTERNAL if "external" in name else MIXED if base == "mixed" else []
    if extra == "hugelog":
        jobs.append(("e2e", "success-huge"))
    if extra == "nolog":
        jobs.append(("flaky", "failure-nolog"))
    if base == "matrix":
        jobs = [(f"matrix ({os_}, {v})", "running" if k in (7, 31) else "failure" if k == 44 else "success")
                for k, (os_, v) in enumerate((o, v) for o in ("linux", "macos", "windows") for v in range(20))]
    return jobs, ext


RUNS = {}       # (repo, sha) → run
RUN_BY_ID = {}  # run id → run
JOB_BY_ID = {}  # job id → (run, job)
_run_ids = {"id": 3100, "job": 52000, "index": {}}


def ci_profile(repo, sha):
    for p in REPOS.get(repo, {}).get("pulls", []):
        cur = p["head_sha"]
        if sha == p["initial_head"]:
            return p.get("ci"), p
        if sha == cur:
            return ("running" if p.get("ci") else None), p   # a new push starts a fresh run
    return None, None


def run_for(repo, sha):
    if (repo, sha) in RUNS:
        return RUNS[(repo, sha)]
    prof, p = ci_profile(repo, sha)
    jobs, ext = profile(prof)
    run = None
    if jobs:
        _run_ids["id"] += 1
        idx = _run_ids["index"][repo] = _run_ids["index"].get(repo, 400) + 1
        run = {"id": _run_ids["id"], "index": idx, "repo": repo, "sha": sha, "title": p["title"] if p else sha[:7], "jobs": []}
        for n, (name, final) in enumerate(jobs):
            add_attempt(run, n, name, final)
        RUN_BY_ID[run["id"]] = run
    RUNS[(repo, sha)] = run
    if run is not None or ext:
        RUNS[(repo, sha, "ext")] = [(c, s, d, u.replace("{BASE}", BASE).replace("{n}", str(p["number"] if p else 0)) if u else None) for c, s, d, u in ext]
    return run


def add_attempt(run, n, name, final, dur=None):
    _run_ids["job"] += 1
    attempt = 1 + sum(1 for j in run["jobs"] if j["n"] == n)
    timed = final == "running" or dur is not None
    j = {"id": _run_ids["job"], "n": n, "name": name, "final": "success" if timed else final.split("-")[0], "kind": final,
         "attempt": attempt, "started": None, "dur": dur if dur is not None else args.log_seconds if timed else 0, "timed": timed, "cancelled": False}
    run["jobs"].append(j)
    JOB_BY_ID[j["id"]] = (run, j)
    return j


def job_status(j):
    if j["cancelled"]:
        return "cancelled"
    if j["timed"]:
        if j["started"] is None:
            j["started"] = time.time()   # the clock starts the first time anyone looks
        return "running" if time.time() - j["started"] < j["dur"] else j["final"]
    return j["final"]


def latest(run):
    out = {}
    for j in run["jobs"]:
        out[j["n"]] = j
    return [out[k] for k in sorted(out)]


def run_status(run):
    st = [job_status(j) for j in latest(run)]
    if "running" in st:
        return "running"
    if "waiting" in st:
        return "waiting"
    if "failure" in st:
        return "failure"
    if "cancelled" in st:
        return "cancelled"
    return "success"


def done(status):
    return status in ("success", "failure", "cancelled", "skipped")


DESC = {"success": ("success", "Successful in 1m4s"), "failure": ("failure", "Failing after 2m10s"), "running": ("pending", "Has started running"),
        "waiting": ("pending", "Waiting to run"), "cancelled": ("error", "Has been cancelled"), "skipped": ("skipped", "Has been skipped")}


def statuses(repo, sha):
    run = run_for(repo, sha)
    out = []
    if run:
        for j in latest(run):
            st, desc = DESC[job_status(j)]
            # "web" links to the run only, so Loupe finds its job by name (the "wf / job" fallback)
            url = f"/{repo}/actions/runs/{run['index']}" + ("" if j["name"] == "web" else f"/jobs/{j['n']}")
            out.append({"id": j["id"], "status": st, "context": f"CI / {j['name']} (pull_request)", "description": desc, "target_url": url,
                        "created_at": now_iso(), "updated_at": now_iso()})
    for k, (c, s, d, u) in enumerate(RUNS.get((repo, sha, "ext"), [])):
        out.append({"id": 900000 + k, "status": s, "context": c, "description": d, "target_url": u, "created_at": now_iso(), "updated_at": now_iso()})
    return out


def combined(sts):
    s = {x["status"] for x in sts}
    return "failure" if s & {"failure", "error"} else "pending" if "pending" in s else "success" if sts else ""


def run_json(run):
    return {"id": run["id"], "index_in_repo": run["index"], "title": run["title"], "workflow_id": "ci.yml", "status": run_status(run),
            "event": "pull_request", "head_sha": run["sha"], "html_url": f"{BASE}/{run['repo']}/actions/runs/{run['index']}"}


STEP_NAMES = ["Set up job", "actions/checkout@v4", "Run", "Post actions/checkout@v4", "Complete job"]


def job_json(run, j, steps=False):
    st = job_status(j)
    out = {"id": j["id"], "run_id": run["id"], "attempt": j["attempt"], "name": j["name"], "status": st, "needs": ["build"] if j["name"] == "test" else [],
           "runs_on": ["docker"], "task_id": j["id"] + 100000,
           "html_url": f"{BASE}/{run['repo']}/actions/runs/{run['index']}/jobs/{j['n']}/attempt/{j['attempt']}"}
    if steps:
        t0 = iso(j["started"] or time.time() - 120)
        s = []
        for k, nm in enumerate(STEP_NAMES):
            nm = f"Run {j['name']}" if nm == "Run" else nm
            if st == "running":
                s.append({"number": k + 1, "name": nm, "status": "success" if k < 2 else "running" if k == 2 else "waiting", "started": t0 if k <= 2 else None, "stopped": t0 if k < 2 else None})
            elif st == "failure":
                s.append({"number": k + 1, "name": nm, "status": "success" if k < 2 else "failure" if k == 2 else "skipped", "started": t0 if k <= 2 else None, "stopped": t0 if k <= 2 else None})
            elif st in ("waiting", "cancelled", "skipped"):
                s.append({"number": k + 1, "name": nm, "status": st, "started": None, "stopped": None})
            else:
                s.append({"number": k + 1, "name": nm, "status": "success", "started": t0, "stopped": t0})
        out["steps"] = s
    return out


def ts(i):
    return f"2026-11-18T10:{(i // 60) % 60:02d}:{i % 60:02d}.{(i * 7919) % 10 ** 7:07d}Z "


def setup(name):
    return ["﻿" + ts(0) + "Current runner version: 'v6.0.1'", ts(1) + "##[group]Set up job", ts(1) + f"Job {name} on docker",
            ts(2) + "##[endgroup]", ts(3) + "##[group]Run actions/checkout@v4", ts(3) + "[command]/usr/bin/git checkout --progress --force refs/remotes/pull/1/merge",
            ts(4) + "##[endgroup]"]


def failing_log(name):
    L = setup(name) + [ts(5) + "##[group]Run cargo test", ts(5) + "[command]cargo test --workspace",
                       ts(6) + "   Compiling acme-server v0.1.0 (/workspace/acme/app)",
                       ts(7) + "\x1b[1m\x1b[31merror[E0308]\x1b[0m: mismatched types",
                       ts(7) + "  --> src/server/payments.rs:7:24",
                       ts(7) + "src/server/payments.rs:7:24: error: expected `i64`, found `Result<i64, ParseIntError>`",
                       ts(8) + "thread 'router::tests::payments' panicked at src/server/router.rs:12:9:",
                       ts(8) + "test router::tests::payments ... FAILED", ts(9) + "##[endgroup]",
                       ts(10) + "##[group]Run go test ./services/...", ts(10) + "--- FAIL: TestChargeRounding (0.00s)",
                       ts(10) + "    services/payments/charge_test.go:7: got &{ch_1 1999 EUR}, want 2000",
                       ts(11) + "FAIL\texample.com/acme/app/services/payments\t0.012s", ts(11) + "##[endgroup]",
                       ts(12) + "##[group]Run cargo clippy (220 findings)"]
    L += [ts(12) + f"src/server/db.rs:{40 + k}:9: \x1b[31merror\x1b[0m: unused variable: `x{k}`" for k in range(220)]
    L += [ts(13) + "##[endgroup]",
          ts(14) + "web/src/payments/format.ts(4,9): error TS2322: Type 'number' is not assignable to type 'string'.",
          ts(14) + "##[warning]Node.js 16 actions are deprecated. Please update actions/setup-node.",
          ts(15) + "##[notice]Cache restored from key: cargo-linux-1a2b3c",
          ts(15) + "##[debug]exit code 101",
          ts(16) + "⭐ 41 passed, 2 failed  ✅ 3 suites run",
          ts(16) + "Windows line ending here\r",
          ts(17) + "Downloading 10%\rDownloading 55%\rDownloading 100%",
          ts(18) + "##[error]Process completed with exit code 101."]
    return L


def ok_log(name, n=120):
    L = setup(name) + [ts(5) + f"##[group]Run {name}", ts(5) + f"[command]make {name}"]
    L += [ts(6 + k // 10) + f"   Compiling module_{k} v0.{k}.0 ... ok ({k * 7 % 97} ms)" for k in range(n)]
    return L + [ts(30) + "##[endgroup]", ts(31) + "✅ Job succeeded"]


def growing_log(name, total):
    L = setup(name)
    for k in range(total):
        if k % 100 == 0:
            L.append(ts(5 + k // 15) + f"##[group]Step {k // 100 + 1}: compile batch {k // 100 + 1}")
        L.append(ts(5 + k // 15) + f"   Compiling crate_{k} v1.{k % 30}.0 ({k * 13 % 211} ms)")
        if k % 100 == 99:
            L.append(ts(5 + k // 15) + "##[endgroup]")
    return L


_huge = {}


def job_log(run, j):
    """The job's log so far, or None (404): queued jobs and the 'flaky' job never have one."""
    st = job_status(j)
    if st in ("waiting", "skipped") or j["kind"] == "failure-nolog":
        return None
    if j["kind"] == "success-huge":
        if "x" not in _huge:   # a finished log over 16 MB
            _huge["x"] = ("\n".join(ok_log(j["name"], 200_000)) + "\n").encode()
        return _huge["x"]
    if j["timed"]:
        total = int(j["dur"] * args.log_rate)
        if st == "running":
            k = int((time.time() - j["started"]) / j["dur"] * total)
            return ("\n".join(growing_log(j["name"], k)) + "\n").encode()
        return ("\n".join(growing_log(j["name"], total) + [ts(59) + ("✅ Job succeeded" if st == "success" else "##[error]Cancelled")]) + "\n").encode()
    if st == "cancelled":
        return ("\n".join(setup(j["name"]) + [ts(9) + "##[error]The operation was canceled."]) + "\n").encode()
    return ("\n".join(failing_log(j["name"]) if st == "failure" else ok_log(j["name"])) + "\n").encode()


def ci_action(kind, run_id, job_id=None):
    """Rerun / cancel, as Forgejo and Gitea refuse them: reruns need the run finished, cancel needs it moving."""
    run = RUN_BY_ID.get(run_id)
    if not run:
        return 404, {"message": "run not found"}
    st = run_status(run)
    if kind == "cancel":
        if done(st):
            return 409, {"message": "run is not running"}
        for j in latest(run):
            if job_status(j) in ("running", "waiting"):
                j["cancelled"] = True
        return 204, None
    if not done(st):
        return 409, {"message": "run is still running"}
    targets = [j for j in latest(run) if (kind == "all") or (kind == "failed" and job_status(j) in ("failure", "cancelled")) or (kind == "job" and j["id"] == job_id)]
    if not targets:
        return 404, {"message": "job not found"}
    for j in targets:
        add_attempt(run, j["n"], j["name"], "success", dur=args.rerun_seconds)
    return 204, None


# ---------------------------------------------------------------------------------------------------------------------
# LFS: objects (batch API + basic transfer) and locks.
# ---------------------------------------------------------------------------------------------------------------------
LFS_DIR = os.path.join(FORGE, "lfs")
LOCKS = {repo: [dict(l, id=str(k + 1), locked_at="2026-11-15T09:00:00Z") for k, l in enumerate(ls)] for repo, ls in STATE.get("lfs_locks", {}).items()}


def lock_json(l):
    return {"id": l["id"], "path": l["path"], "locked_at": l["locked_at"], "owner": {"name": l["owner"]}}


# ---------------------------------------------------------------------------------------------------------------------
# HTTP
# ---------------------------------------------------------------------------------------------------------------------
class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, fmt, *a):
        if args.verbose:
            print(f"{self.command} {self.path} {self.headers.get('Range') or ''} -> {a[1] if len(a) > 1 else ''}"
                  f" [{self.headers.get('User-Agent', '')[:24]} | {self.headers.get('Accept', '')} | {(self.headers.get('Authorization') or '').split(' ')[0]}]", file=sys.stderr, flush=True)

    def send(self, code, body=None, ctype="application/json", headers=()):
        if body is None:
            body = b""
        elif not isinstance(body, (bytes, bytearray)):
            body = json.dumps(body).encode()
        self.send_response(code)
        if code != 204:
            self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        for k, v in headers:
            self.send_header(k, v)
        self.end_headers()
        self.wfile.write(body)

    def miss(self):
        print(f"404 unknown endpoint: {self.command} {self.path}", file=sys.stderr, flush=True)
        self.send(404, {"message": "not found (mock)", "url": self.path})

    def body(self):
        if self.headers.get("Transfer-Encoding", "").lower() == "chunked":
            data = bytearray()
            while True:
                size = int(self.rfile.readline().strip().split(b";")[0], 16)
                if size == 0:
                    self.rfile.readline()
                    break
                data += self.rfile.read(size)
                self.rfile.readline()
            return bytes(data)
        n = int(self.headers.get("Content-Length") or 0)
        return self.rfile.read(n) if n else b""

    def json_body(self, raw):
        try:
            return json.loads(raw or b"{}")
        except ValueError:
            return {}

    def fault(self):
        for f in FAULTS:
            if f["method"] in ("*", self.command) and re.search(f["path"], self.path):
                if f.get("times", -1) == 0:
                    continue
                f["times"] = f.get("times", -1) - 1 if f.get("times", -1) > 0 else f.get("times", -1)
                b = f.get("body", {"message": f"mock fault {f['status']}"})
                print(f"fault {f['status']}: {self.command} {self.path}", file=sys.stderr, flush=True)
                self.send(f["status"], b if not isinstance(b, str) else b.encode(), "application/json" if not isinstance(b, str) else "text/plain")
                return True
        return False

    def handle_any(self):
        u = urlsplit(self.path)
        p, q = u.path, parse_qs(u.query)
        if ".git/" in p and ("/info/refs" in p or p.endswith("git-upload-pack") or p.endswith("git-receive-pack")):
            if not self.fault():
                self.git_backend(u)
            return
        raw = self.body() if self.command in ("POST", "PUT", "PATCH", "DELETE") else b""
        if p.startswith("/_mock"):
            return self.control(p, self.json_body(raw))
        if self.fault():
            return
        if raw and self.command != "PUT":
            print(f"{self.command} {p} body: {raw[:600].decode(errors='replace')}", file=sys.stderr, flush=True)
        m = re.match(r"^/avatars/(missing/)?([^/]+)\.png$", p)
        if m:
            return self.miss() if m.group(1) else self.send(200, png(unquote(m.group(2))), "image/png", [("Cache-Control", "max-age=3600")])
        m = re.match(r"^/([\w.-]+/[\w.-]+)\.git/info/lfs/(.*)$", p)
        if m:
            return self.lfs(m.group(1), m.group(2), self.json_body(raw))
        m = re.match(r"^/lfs-objects/([0-9a-f]{64})$", p)
        if m:
            f = os.path.join(LFS_DIR, m.group(1))
            if self.command == "PUT":
                open(f, "wb").write(raw)
                return self.send(200, {})
            return self.send(200, open(f, "rb").read(), "application/octet-stream") if os.path.exists(f) else self.miss()
        if p.startswith("/api/v1/"):
            auth = self.headers.get("Authorization", "")
            if auth.split(" ")[-1] != TOKEN:
                return self.send(401, {"message": "token is required" if not auth else "user does not exist"})
            with LOCK:
                return self.api(p[7:], q, self.json_body(raw))
        if self.command == "GET" and re.match(r"^/(acme/[\w.-]+/(pulls|actions|commit)|preview)/", p):
            return self.send(200, f"<html><body><h1>mock forge</h1><p>{p}</p></body></html>".encode(), "text/html")
        return self.miss()

    do_GET = do_POST = do_PUT = do_PATCH = do_DELETE = handle_any

    def control(self, p, b):
        global VERSION
        if p == "/_mock/fault" and self.command == "POST":
            FAULTS.insert(0, {"method": b.get("method", "*"), "path": b["path"], "status": int(b.get("status", 500)), "body": b.get("body", {"message": "mock fault"}), "times": b.get("times", -1)})
            return self.send(200, {"faults": FAULTS})
        if p == "/_mock/fault" and self.command == "DELETE":
            FAULTS.clear()
            return self.send(200, {"faults": []})
        if p == "/_mock/version":
            VERSION = b.get("version", VERSION)
            return self.send(200, {"version": VERSION})
        if p == "/_mock/pr":
            with LOCK:
                pr = find_pr(b.pop("repo", "acme/app"), int(b.pop("number", 0)))
                if not pr:
                    return self.miss()
                pr.update(b)
                return self.send(200, {k: pr[k] for k in ("number", "title", "state")})
        if p == "/_mock":
            return self.send(200, {"version": VERSION, "faults": FAULTS, "runs": len(RUN_BY_ID),
                                   "pulls": {r: [(x["number"], x["state"], x["head"]) for x in v["pulls"]] for r, v in REPOS.items()}})
        return self.miss()

    def api(self, p, q, b):
        cmd = self.command
        if p == "/version" and cmd == "GET":
            return self.send(500, {"message": "version unavailable"}) if VERSION == "500" else self.send(200, {"version": VERSION})
        if p == "/user" and cmd == "GET":
            return self.send(200, user(ME))
        m = re.match(r"^/repos/([\w.-]+/[\w.-]+)(/.*)?$", p)
        if not m or m.group(1) not in REPOS:
            return self.miss()
        repo, rest = m.group(1), m.group(2) or ""
        R = REPOS[repo]
        if rest == "" and cmd == "GET":
            return self.send(200, {"id": R["id"], "name": repo.split("/")[1], "full_name": repo, "owner": user(repo.split("/")[0]), "default_branch": "main",
                                   "description": R["description"], "private": False, "fork": False, "html_url": f"{BASE}/{repo}",
                                   "clone_url": f"{BASE}/{repo}.git", "has_pull_requests": True, "has_actions": True,
                                   "permissions": {"admin": True, "push": True, "pull": True}})
        if rest == "/pulls" and cmd == "GET":
            st = q.get("state", ["open"])[0]
            refs = heads(repo)
            items = sorted((x for x in R["pulls"] if st == "all" or x["state"] == st), key=lambda x: -x["number"])
            return self.send(200, [pr_json(repo, x, refs) for x in paged(items, q)])
        if rest == "/pulls" and cmd == "POST":
            return self.create_pull(repo, b)
        m = re.match(r"^/pulls/(\d+)(\.diff|\.patch)?$", rest)
        if m and cmd == "GET":
            pr = find_pr(repo, int(m.group(1)))
            if not pr:
                return self.miss()
            if m.group(2):
                bs, hs = shas(repo, pr)
                return self.send(200, diff(repo, bs, hs), "text/plain; charset=utf-8")
            return self.send(200, pr_json(repo, pr))
        m = re.match(r"^/pulls/(\d+)/(reviews|commits|files|merge)$", rest)
        pr = find_pr(repo, int(m.group(1))) if m else None
        if m and not pr:
            return self.miss()
        if m and m.group(2) == "reviews" and cmd == "GET":
            return self.send(200, paged(reviews_json(repo, pr), q))
        if m and m.group(2) == "reviews" and cmd == "POST":
            return self.submit_review(repo, pr, b)
        if m and m.group(2) == "commits" and cmd == "GET":
            return self.send(200, paged(pr_commits(repo, pr), q))
        if m and m.group(2) == "merge" and cmd == "POST":
            if pr["state"] != "open":
                return self.send(405, {"message": "The pull request is already closed or merged"})
            bs, hs = shas(repo, pr)
            if not mergeable(repo, pr, bs, hs) or merge(repo, pr, b.get("Do", "merge")) is None:
                return self.send(405, {"message": "Please try again later: this pull request has conflicts"})
            _cache.clear()
            return self.send(200, {})
        m = re.match(r"^/pulls/(\d+)/reviews/(\d+)/comments$", rest)
        if m and cmd == "GET":
            pr = find_pr(repo, int(m.group(1)))
            out = review_comments_json(repo, pr, int(m.group(2))) if pr else None
            return self.send(200, out) if out is not None else self.miss()
        m = re.match(r"^/issues/(\d+)/comments$", rest)
        if m:
            pr = find_pr(repo, int(m.group(1)))
            if not pr:
                return self.miss()
            if cmd == "POST":
                if not (b.get("body") or "").strip():
                    return self.send(422, {"message": "body is empty"})
                c = {"id": next_id("issue"), "user": ME, "body": b["body"], "created_at": now_iso()}
                pr["issue_comments"].append(c)
                pr["updated_at"] = now_iso()
                return self.send(201, issue_comments_json(repo, pr)[-1])
            return self.send(200, issue_comments_json(repo, pr))
        m = re.match(r"^/commits/([0-9a-f]{40})/status(es)?$", rest)
        if m and cmd == "GET":
            s = statuses(repo, m.group(1))
            if m.group(2):
                return self.send(200, paged(s, q))
            return self.send(200, {"state": combined(s), "sha": m.group(1), "total_count": len(s), "statuses": paged(s, q)})
        if rest == "/commits" and cmd == "GET":
            sha = q.get("sha", [""])[0]
            info = git(repo, "log", "-1", "--format=%H%x1f%ae%x1f%an%x1f%aI%x1f%s", sha, "--", check=False).strip().split("\x1f")
            if len(info) < 5:
                return self.send(200, [])
            return self.send(200, [{"sha": info[0], "author": user(EMAIL.get(info[1])), "committer": user(EMAIL.get(info[1])),
                                    "commit": {"message": info[4], "author": {"name": info[2], "email": info[1], "date": info[3]}}}])
        if rest == "/actions/runs" and cmd == "GET":
            sha = q.get("head_sha", [""])[0]
            run = run_for(repo, sha) if sha else None
            runs = [run_json(run)] if run else ([run_json(r) for r in RUN_BY_ID.values() if r["repo"] == repo] if not sha else [])
            return self.send(200, {"workflow_runs": runs, "total_count": len(runs)})
        m = re.match(r"^/actions/runs/(\d+)(/jobs|/rerun|/rerun-failed-jobs|/cancel|/jobs/(\d+)/rerun)?$", rest)
        if m:
            run = RUN_BY_ID.get(int(m.group(1)))
            if not run:
                return self.miss()
            sub = m.group(2) or ""
            if cmd == "GET" and sub == "":
                return self.send(200, run_json(run))
            if cmd == "GET" and sub == "/jobs":
                return self.send(200, [job_json(run, j) for j in run["jobs"]])   # every attempt; Loupe keeps the newest per job
            if cmd == "POST":
                kind = "cancel" if sub == "/cancel" else "all" if sub == "/rerun" else "failed" if sub == "/rerun-failed-jobs" else "job"
                code, body = ci_action(kind, run["id"], int(m.group(3)) if m.group(3) else None)
                return self.send(code, body)
        m = re.match(r"^/actions/jobs/(\d+)(/logs|/rerun)?$", rest)
        if m and int(m.group(1)) in JOB_BY_ID:
            run, j = JOB_BY_ID[int(m.group(1))]
            if cmd == "POST" and m.group(2) == "/rerun":
                code, body = ci_action("job", run["id"], j["id"])
                return self.send(code, body)
            if cmd == "GET" and not m.group(2):
                return self.send(200, job_json(run, j, steps=True))
            if cmd == "GET":
                data = job_log(run, j)
                return self.miss() if data is None else self.serve_log(data)
        return self.miss()

    def create_pull(self, repo, b):
        R = REPOS[repo]
        head, base = b.get("head", ""), b.get("base", "main")
        refs = heads(repo)
        if head not in refs or base not in refs:
            return self.send(422, {"message": f"branch {head if head not in refs else base} does not exist"})
        ex = next((x for x in R["pulls"] if x["state"] == "open" and x["head"] == head and x.get("head_repo_id") is None and x["base"] == base), None)
        if ex:
            return self.send(409, {"message": f"pull request already exists for these targets [id: {ex['number']}, issue_id: {ex['number']}, head_repo_id: {R['id']}, base_repo_id: {R['id']}, head_branch: {head}, base_branch: {base}]"})
        title = (b.get("title") or "").strip()
        if not title:
            return self.send(422, {"message": "title is required"})
        n = max([x["number"] for x in R["pulls"]] + [0]) + 1
        pr = dict(repo=repo, number=n, title=title, body=b.get("body", ""), user=ME, head=head, base=base, state="open", merged=False,
                  draft=title.lower().startswith(("wip:", "[wip]")), head_sha=refs[head], initial_head=refs[head], ci="running",
                  created_at=now_iso(), issue_comments=[], reviews=[])
        R["pulls"].append(pr)
        git(repo, "update-ref", f"refs/pull/{n}/head", refs[head])
        return self.send(201, pr_json(repo, pr, refs))

    def submit_review(self, repo, pr, b):
        event = (b.get("event") or "COMMENT").upper()
        if event in ("APPROVED", "APPROVE", "REQUEST_CHANGES") and pr["user"] == ME:
            return self.send(422, {"message": f"{'approve' if event.startswith('APPROVE') else 'reject'} your own pull is not allowed"})
        if event == "COMMENT" and not (b.get("body") or "").strip() and not b.get("comments"):
            return self.send(422, {"message": "review event COMMENT requires a body or comments"})
        bs, hs = shas(repo, pr)
        comments = []
        for c in b.get("comments") or []:
            new, old = c.get("new_position") or 0, c.get("old_position") or 0
            lines = diff_lines(diff(repo, bs, hs), c["path"])
            hit = next((x for x in lines if (new and x[2] == new and x[0] != "-") or (old and not new and x[1] == old and x[0] != "+")), None)
            comments.append({"id": next_id("comment"), "user": ME, "path": c["path"], "body": c["body"], "position": new, "original_position": old,
                             "diff_hunk": hit[4] if hit else None, "created_at": now_iso()})
        v = {"id": next_id("review"), "user": ME, "state": "APPROVED" if event == "APPROVE" else event, "body": b.get("body", ""),
             "commit_id": b.get("commit_id") or hs, "submitted_at": now_iso(), "comments": comments}
        pr["reviews"].append(v)
        pr["updated_at"] = now_iso()
        return self.send(200, next(x for x in reviews_json(repo, pr) if x["id"] == v["id"]))

    def lfs(self, repo, rest, b):
        locks = LOCKS.setdefault(repo, [])
        ctype = "application/vnd.git-lfs+json"
        if rest == "objects/batch" and self.command == "POST":
            out = []
            for o in b.get("objects", []):
                have = os.path.exists(os.path.join(LFS_DIR, o["oid"]))
                e = {"oid": o["oid"], "size": o["size"], "authenticated": True}
                href = {"href": f"{BASE}/lfs-objects/{o['oid']}"}
                if b.get("operation") == "upload":
                    if not have:
                        e["actions"] = {"upload": href}
                elif have:
                    e["actions"] = {"download": href}
                else:
                    e["error"] = {"code": 404, "message": "Object does not exist"}
                out.append(e)
            return self.send(200, {"transfer": "basic", "objects": out}, ctype)
        if rest == "locks" and self.command == "GET":
            return self.send(200, {"locks": [lock_json(l) for l in locks]}, ctype)
        if rest == "locks" and self.command == "POST":
            ex = next((l for l in locks if l["path"] == b.get("path")), None)
            if ex:
                return self.send(409, {"lock": lock_json(ex), "message": "already created lock"}, ctype)
            l = {"id": str(max([int(x["id"]) for x in locks] + [0]) + 1), "path": b.get("path"), "owner": ME, "locked_at": now_iso()}
            locks.append(l)
            return self.send(201, {"lock": lock_json(l)}, ctype)
        if rest == "locks/verify" and self.command == "POST":
            return self.send(200, {"ours": [lock_json(l) for l in locks if l["owner"] == ME], "theirs": [lock_json(l) for l in locks if l["owner"] != ME]}, ctype)
        m = re.match(r"^locks/(\w+)/unlock$", rest)
        if m and self.command == "POST":
            l = next((x for x in locks if x["id"] == m.group(1)), None)
            if not l:
                return self.send(404, {"message": "lock not found"}, ctype)
            if l["owner"] != ME and not b.get("force"):
                return self.send(403, {"message": f"lock is owned by {l['owner']}"}, ctype)
            locks.remove(l)
            return self.send(200, {"lock": lock_json(l)}, ctype)
        return self.miss()

    def git_backend(self, u):
        """git http-backend as CGI: clone, fetch and push of the served repos."""
        body = self.body()
        if self.headers.get("Content-Encoding") == "gzip":
            body = gzip.decompress(body)
        env = {"GIT_PROJECT_ROOT": FORGE, "GIT_HTTP_EXPORT_ALL": "1", "PATH_INFO": unquote(u.path), "QUERY_STRING": u.query,
               "REQUEST_METHOD": self.command, "CONTENT_TYPE": self.headers.get("Content-Type", ""), "CONTENT_LENGTH": str(len(body)),
               "HTTP_GIT_PROTOCOL": self.headers.get("Git-Protocol", ""), "REMOTE_USER": ME, "REMOTE_ADDR": "127.0.0.1",
               "PATH": "/usr/bin:/bin:/opt/homebrew/bin", "HOME": os.path.join(OUT, "home"), "GIT_CONFIG_NOSYSTEM": "1"}
        out = subprocess.run([GIT, "http-backend"], input=body, env=env, stdout=subprocess.PIPE).stdout
        head, _, payload = out.partition(b"\r\n\r\n")
        status, headers = 200, []
        for line in head.decode().split("\r\n"):
            k, _, v = line.partition(": ")
            if k.lower() == "status":
                status = int(v.split()[0])
            elif k:
                headers.append((k, v))
        self.send_response(status)
        for k, v in headers:
            self.send_header(k, v)
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)
        if u.path.endswith("git-receive-pack"):
            with LOCK:
                _cache.clear()   # a push moves PR heads and bases

    def serve_log(self, data):
        size = len(data)
        rng = self.headers.get("Range")
        m = re.match(r"bytes=(\d*)-(\d*)$", (rng or "").strip())
        if not rng or not m or args.ignore_range:
            return self.send(200, data, "text/plain; charset=utf-8")
        a, b = m.group(1), m.group(2)
        start, end = (max(0, size - int(b)), size - 1) if a == "" else (int(a), min(size - 1, int(b)) if b else size - 1)
        if start >= size:
            return self.send(416, b"", "text/plain", [("Content-Range", f"bytes */{size}")])
        return self.send(206, data[start:end + 1], "text/plain; charset=utf-8", [("Content-Range", f"bytes {start}-{end}/{size}")])


def paged(items, q, default=30):
    page = max(1, int(q.get("page", ["1"])[0]))
    limit = int(q.get("limit", q.get("per_page", [str(default)]))[0])
    return items[(page - 1) * limit: page * limit]


if __name__ == "__main__":
    ThreadingHTTPServer.daemon_threads = True
    srv = ThreadingHTTPServer(("127.0.0.1", args.port), H)
    print(f"flows mock forge on {BASE} (version {VERSION}; {sum(len(r['pulls']) for r in REPOS.values())} PRs; repos {', '.join(REPOS)})", flush=True)
    srv.serve_forever()
