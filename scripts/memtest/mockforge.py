#!/usr/bin/env python3
"""A mock Forgejo for Loupe's memory stress test: just the API v1 endpoints Loupe calls (Forgejo.swift, CIForges.swift,
PullModel.swift, PullCreate.swift), with big data.

    scripts/memtest/mockforge.py <big-repo> [--port 8765] [--open 2000] [--closed 3000]

- acme/big: PR #1 is huge-pr (5,000 files, ~250k diff lines, the diff computed by git from the mock repo, so Loupe's
  local reads of the same shas agree), 20 reviews x 25 inline comments, 100 issue comments, 120 commits. PRs #2.. use
  the repo's feature/bNNN branches. Thousands of PRs, paged with page/limit as Forgejo does.
- Every sha gets 30 commit statuses tied to Actions run 9001; jobs 1-3 run forever and their logs grow ~25 lines/s
  from server start, served with Range (206 + Content-Range, 416 when nothing is new), so Loupe's 2 s tail poll never ends.
- /avatars/<name>.png: small generated PNGs. Unknown paths answer 404 and are logged to stderr.
"""
import argparse, json, re, struct, subprocess, sys, threading, time, zlib
from http.server import ThreadingHTTPServer, BaseHTTPRequestHandler
from urllib.parse import urlsplit, parse_qs

GIT = "/opt/homebrew/bin/git"
ap = argparse.ArgumentParser()
ap.add_argument("repo")
ap.add_argument("--port", type=int, default=8765)
ap.add_argument("--open", type=int, default=2000)
ap.add_argument("--closed", type=int, default=3000)
ap.add_argument("--verbose", action="store_true", help="log every request to stderr")
ap.add_argument("--log-rate", type=float, default=25.0, help="log lines per second for running jobs")
args = ap.parse_args()
BASE = f"http://127.0.0.1:{args.port}"
START = time.time()


def git(*a):
    return subprocess.run([GIT, *a], cwd=args.repo, stdout=subprocess.PIPE, check=True).stdout


refs = dict(l.split(" ", 1)[::-1] for l in git("for-each-ref", "--format=%(objectname) %(refname:short)", "refs/heads").decode().split("\n") if l)
MAIN = refs["main"]
FEATURES = sorted(r for r in refs if r.startswith("feature/"))
MAIN_LOG = git("log", "-n", "120", "--format=%H%x1f%s%x1f%aI", "main").decode().strip().split("\n")
TS = "2026-11-01T10:00:00Z"


def user(name):
    return {"login": name, "avatar_url": f"{BASE}/avatars/{name}.png", "id": zlib.crc32(name.encode()) % 10000}


PEOPLE = ["alice", "bob", "carol", "dave", "erin", "frank", "grace", "heidi"]


def pr_head(n):
    if n == 1:
        return "huge-pr", refs["huge-pr"]
    b = FEATURES[(n - 2) % len(FEATURES)]
    return b, refs[b]


def pr_json(n):
    total = args.open + args.closed
    if not 1 <= n <= total:
        return None
    ref, sha = pr_head(n)
    state = "open" if n <= args.open else "closed"
    merged = state == "closed" and n % 3 == 0
    body = (f"## Summary\n\nPull request {n} changes `{ref}`.\n\n- point one\n- point two with **bold** and `code`\n\n"
            + "Lorem ipsum dolor sit amet, consectetur adipiscing elit. " * 6)
    return {"id": 100000 + n, "number": n, "title": f"{'Rewrite 5000 generated files' if n == 1 else 'Feature ' + ref} (#{n})",
            "body": body, "state": state, "draft": n % 17 == 0, "user": user(PEOPLE[n % len(PEOPLE)]),
            "head": {"ref": ref, "sha": sha, "label": ref, "repo_id": 1}, "base": {"ref": "main", "sha": MAIN, "label": "main", "repo_id": 1},
            "additions": 200000 if n == 1 else 31, "deletions": 50000 if n == 1 else 1, "changed_files": 5000 if n == 1 else 2,
            "comments": 100 if n == 1 else 5, "mergeable": True, "merged": merged, "merged_at": TS if merged else None,
            "requested_reviewers": [user("bob"), user("carol")], "html_url": f"{BASE}/acme/big/pulls/{n}",
            "updated_at": TS, "created_at": TS}


_diffs, _diff_lock = {}, threading.Lock()


def pr_diff(n):
    with _diff_lock:
        if n not in _diffs:
            ref, sha = pr_head(n)
            _diffs[n] = git("diff", "--no-color", "--no-ext-diff", "-M", f"{MAIN}...{sha}")
        return _diffs[n]


def pr_files(n):
    return [l[6:] for l in pr_diff(n).decode(errors="replace").split("\n") if l.startswith("+++ b/")]


def reviews(n):
    k = 20 if n == 1 else 3
    states = ["COMMENT", "APPROVED", "REQUEST_CHANGES", "COMMENT"]
    return [{"id": n * 1000 + i, "user": user(PEOPLE[i % len(PEOPLE)]), "state": states[i % 4], "body": f"Review {i} on #{n}. " * 3,
             "commit_id": pr_head(n)[1], "stale": False, "dismissed": False, "submitted_at": TS} for i in range(k)]


def review_comments(n, rid):
    i = rid - n * 1000
    per = 25 if n == 1 else 3
    files = pr_files(n) or ["README.md"]
    out = []
    for c in range(per):
        f = files[(i * per + c) * 7 % len(files)]
        out.append({"id": rid * 100 + c, "user": user(PEOPLE[(i + c) % len(PEOPLE)]), "path": f,
                    "body": f"Inline comment {c} of review {i}: consider renaming this.\n\n```\nlet x = {c}\n```",
                    "position": 2 + c % 20, "original_position": 0, "created_at": TS, "commit_id": pr_head(n)[1],
                    "pull_request_review_id": rid, "diff_hunk": "@@ -1,3 +1,3 @@\n context\n-old\n+new"})
    return out


def issue_comments(n):
    return [{"id": n * 10000 + i, "user": user(PEOPLE[i % len(PEOPLE)]), "body": f"Comment {i}: looks good, but see the note about **naming**.\n\n> quoted\n\n- a\n- b",
             "created_at": TS} for i in range(100 if n == 1 else 5)]


def pr_commits(n):
    rows = MAIN_LOG if n == 1 else MAIN_LOG[:3]
    return [{"sha": h, "commit": {"message": s, "author": {"name": "Alice Mock", "date": d}, "committer": {"name": "Alice Mock", "date": d}},
             "author": user("alice")} for h, s, d in (r.split("\x1f") for r in rows)]


RUN_ID, RUN_INDEX, JOBS = 9001, 501, 30


def job_status(k):
    return "running" if k <= 3 else "failure" if k in (7, 19) else "success"


def statuses(sha):
    out = []
    for k in range(1, JOBS + 1):
        st = job_status(k)
        out.append({"id": k, "status": {"running": "pending", "failure": "failure"}.get(st, "success"), "context": f"ci / job-{k:02d} (push)",
                    "description": "Running" if st == "running" else "Failing after 2m" if st == "failure" else "Successful in 1m",
                    "target_url": f"/acme/big/actions/runs/{RUN_INDEX}/jobs/{k - 1}", "created_at": TS, "updated_at": TS})
    return out


def job(k, steps=False):
    st = job_status(k)
    j = {"id": 70000 + k, "run_id": RUN_ID, "attempt": 1, "name": f"job-{k:02d}", "html_url": f"{BASE}/acme/big/actions/runs/{RUN_INDEX}/jobs/{k - 1}/attempt/1",
         "needs": [], "runs_on": ["docker"], "task_id": 5000 + k, "status": st}
    if steps:
        j["steps"] = [{"number": s, "name": f"step {s}", "status": st if s == 4 else "success", "started": TS, "stopped": None if st == "running" and s == 4 else TS}
                      for s in range(1, 6)]
    return j


def log_line(i):
    if i % 400 == 0:
        return f"##[group]Section {i // 400}\n"
    if i % 400 == 399:
        return "##[endgroup]\n"
    if i % 997 == 0:
        return f"2026-11-11T10:00:00Z error: something failed at step {i}\n"
    return f"2026-11-11T10:00:00Z compiling module_{i} ... ok ({i % 97} ms)\n"


class Log:
    def __init__(self):
        self.buf, self.lines, self.lock = bytearray(), 0, threading.Lock()

    def upto(self, n):
        with self.lock:
            while self.lines < n:
                self.buf += log_line(self.lines).encode()
                self.lines += 1
            return bytes(self.buf)


_logs = {}


def job_log(k):
    lg = _logs.setdefault(k, Log())
    n = 2000 + int((time.time() - START) * args.log_rate) if job_status(k) == "running" else 3000
    return lg.upto(n)


def png(name):
    h = zlib.crc32(name.encode())
    r, g, b = h % 200 + 30, h // 7 % 200 + 30, h // 49 % 200 + 30
    raw = b"".join(b"\x00" + bytes([r, g, b]) * 64 for _ in range(64))
    chunk = lambda t, d: struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d) & 0xffffffff)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 64, 64, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b"")


def paged(items, q, default=50):
    page = int(q.get("page", ["1"])[0])
    limit = int(q.get("limit", q.get("per_page", [str(default)]))[0])
    return items[(page - 1) * limit: page * limit]


class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, fmt, *a):
        if args.verbose:
            print(f"{self.command} {self.path} {self.headers.get('Range') or ''} -> {a[1] if len(a) > 1 else ''}", file=sys.stderr, flush=True)

    def send(self, code, body, ctype="application/json", headers=()):
        if not isinstance(body, (bytes, bytearray)):
            body = json.dumps(body).encode()
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        for k, v in headers:
            self.send_header(k, v)
        self.end_headers()
        self.wfile.write(body)

    def miss(self):
        print(f"404 {self.command} {self.path}", file=sys.stderr, flush=True)
        self.send(404, {"message": "not found (mock)"})

    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        self.rfile.read(n)
        if self.path.endswith("/info/lfs/locks/verify"):
            return self.send(200, {"ours": [], "theirs": []}, "application/vnd.git-lfs+json")
        self.send(201, {"message": "mock: ignored"})

    def do_GET(self):
        u = urlsplit(self.path)
        p, q = u.path, parse_qs(u.query)
        m = re.match(r"^/avatars/([\w.-]+)\.png$", p)
        if m:
            return self.send(200, png(m.group(1)), "image/png", [("Cache-Control", "max-age=3600")])
        if re.match(r"^/[\w.-]+/[\w.-]+\.git/info/lfs/locks$", p):   # git lfs locks (LFSModel)
            return self.send(200, {"locks": []}, "application/vnd.git-lfs+json")
        if p == "/api/v1/version":
            return self.send(200, {"version": "11.0.0+gitea-1.22.0"})
        if p == "/api/v1/user":
            return self.send(200, user("alice"))
        m = re.match(r"^/api/v1/repos/([\w.-]+)/([\w.-]+)(/.*)?$", p)
        if not m:
            return self.miss()
        owner, name, rest = m.group(1), m.group(2), m.group(3) or ""
        big = name == "big"
        if rest == "":
            return self.send(200, {"id": 1, "name": name, "full_name": f"{owner}/{name}", "default_branch": "main", "owner": user(owner)})
        if rest == "/pulls":
            if not big:
                return self.send(200, [])
            st = q.get("state", ["open"])[0]
            nums = range(1, args.open + 1) if st == "open" else range(args.open + 1, args.open + args.closed + 1) if st == "closed" else range(1, args.open + args.closed + 1)
            return self.send(200, [pr_json(n) for n in paged(list(nums), q)])
        m = re.match(r"^/pulls/(\d+)(\.diff)?$", rest)
        if m and big:
            n = int(m.group(1))
            pr = pr_json(n)
            if not pr:
                return self.miss()
            return self.send(200, pr_diff(n), "text/plain; charset=utf-8") if m.group(2) else self.send(200, pr)
        m = re.match(r"^/pulls/(\d+)/(reviews|commits)$", rest)
        if m and big:
            n = int(m.group(1))
            return self.send(200, paged(reviews(n) if m.group(2) == "reviews" else pr_commits(n), q))
        m = re.match(r"^/pulls/(\d+)/reviews/(\d+)/comments$", rest)
        if m and big:
            return self.send(200, review_comments(int(m.group(1)), int(m.group(2))))
        m = re.match(r"^/issues/(\d+)/comments$", rest)
        if m:
            return self.send(200, issue_comments(int(m.group(1))) if big else [])
        m = re.match(r"^/commits/([0-9a-f]+)/status$", rest)
        if m:
            s = statuses(m.group(1))
            return self.send(200, {"state": "pending", "total_count": len(s), "statuses": paged(s, q)})
        if rest == "/commits":
            return self.send(200, [{"sha": q.get("sha", [""])[0], "author": user("alice")}])
        if rest == "/actions/runs":
            return self.send(200, {"workflow_runs": [{"id": RUN_ID, "index_in_repo": RUN_INDEX, "title": "CI", "workflow_id": "ci.yml",
                                                      "status": "running", "event": "push", "html_url": f"{BASE}/acme/{name}/actions/runs/{RUN_INDEX}"}],
                                   "total_count": 1})
        if rest == f"/actions/runs/{RUN_ID}/jobs":
            return self.send(200, [job(k) for k in range(1, JOBS + 1)])
        m = re.match(r"^/actions/jobs/(\d+)(/logs)?$", rest)
        if m and 70001 <= int(m.group(1)) <= 70000 + JOBS:
            k = int(m.group(1)) - 70000
            if not m.group(2):
                return self.send(200, job(k, steps=True))
            return self.serve_log(job_log(k))
        return self.miss()

    def serve_log(self, data):
        size = len(data)
        rng = self.headers.get("Range")
        if not rng:
            return self.send(200, data, "text/plain; charset=utf-8")
        m = re.match(r"bytes=(\d*)-(\d*)$", rng.strip())
        if not m:
            return self.send(200, data, "text/plain; charset=utf-8")
        a, b = m.group(1), m.group(2)
        if a == "":   # suffix: the last N bytes
            start, end = max(0, size - int(b)), size - 1
        else:
            start, end = int(a), min(size - 1, int(b)) if b else size - 1
        if start >= size:
            return self.send(416, b"", "text/plain", [("Content-Range", f"bytes */{size}")])
        return self.send(206, data[start:end + 1], "text/plain; charset=utf-8", [("Content-Range", f"bytes {start}-{end}/{size}")])


if __name__ == "__main__":
    print(f"mock forge on {BASE} ({args.open} open + {args.closed} closed PRs, repo {args.repo})", flush=True)
    ThreadingHTTPServer.daemon_threads = True
    ThreadingHTTPServer(("127.0.0.1", args.port), H).serve_forever()
