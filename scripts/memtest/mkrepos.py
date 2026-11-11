#!/usr/bin/env python3
"""Builds the memory stress-test repositories for Loupe.

    scripts/memtest/mkrepos.py <out-dir> [--port 8765] [--commits 100000] [--worktrees 20]

Writes <out-dir>/data/... (repos), <out-dir>/home (a HOME with its own .gitconfig, so neither generation nor the test
app reads the owner's ~/.gitconfig) and <out-dir>/scenario.json for the stress driver (Sources/Loupe/StressDriver.swift).
Every remote points at the mock forge (scripts/memtest/mockforge.py) on 127.0.0.1:<port>. Re-running wipes data/.
"""
import argparse, json, os, random, shutil, subprocess, sys, time

GIT = "/opt/homebrew/bin/git"   # a shell hook rewrites plain `git` output
ap = argparse.ArgumentParser()
ap.add_argument("out")
ap.add_argument("--port", type=int, default=8765)
ap.add_argument("--commits", type=int, default=100_000)
ap.add_argument("--worktrees", type=int, default=20)
ap.add_argument("--branches", type=int, default=300)
ap.add_argument("--tags", type=int, default=500)
args = ap.parse_args()

OUT = os.path.abspath(args.out)
DATA = os.path.join(OUT, "data")
HOME = os.path.join(OUT, "home")
FORGE = f"http://127.0.0.1:{args.port}"
os.makedirs(HOME, exist_ok=True)
with open(os.path.join(HOME, ".gitconfig"), "w") as f:
    f.write("[user]\n\tname = Alice Mock\n\temail = alice@example.com\n[init]\n\tdefaultBranch = main\n"
            "[protocol \"file\"]\n\tallow = always\n[advice]\n\tdetachedHead = false\n[gc]\n\tauto = 0\n"
            "[maintenance]\n\tauto = false\n[core]\n\tfsmonitor = false\n")
ENV = dict(os.environ, HOME=HOME, GIT_CONFIG_NOSYSTEM="1", GIT_AUTHOR_NAME="Alice Mock", GIT_AUTHOR_EMAIL="alice@example.com",
           GIT_COMMITTER_NAME="Alice Mock", GIT_COMMITTER_EMAIL="alice@example.com")
ENV.pop("GIT_DIR", None)


def git(*a, cwd, inp=None, check=True):
    r = subprocess.run([GIT, *a], cwd=cwd, env=ENV, input=inp, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if check and r.returncode != 0:
        sys.exit(f"git {' '.join(a)} in {cwd} failed: {r.stderr.decode()[:2000]}")
    return r.stdout.decode().strip()


def write(path, text):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as f:
        f.write(text)


def log(msg):
    print(f"[{time.strftime('%H:%M:%S')}] {msg}", flush=True)


if os.path.exists(DATA):
    shutil.rmtree(DATA)
os.makedirs(DATA)
rnd = random.Random(42)

# ---------------------------------------------------------------------------------------------------------------------
# big: ~100k commits, branches and tags (fast-import), a 5,000-file commit near the tip, a 150k-line file, a 30 MB
# minified file, the huge-pr branch the mock forge serves as PR #1.
# ---------------------------------------------------------------------------------------------------------------------
BIG = os.path.join(DATA, "big")
git("init", "-q", BIG, cwd=DATA)
log(f"fast-import: {args.commits} commits")


def blob_cmd(text):
    b = text.encode()
    return b"data %d\n" % len(b) + b + b"\n"


def small_file(mod, j, n):
    return "".join(f"module {mod} file {j} line {k} rev {n if k == n % 20 else 0}\n" for k in range(20))


def gen_file(a, b, rev):
    # rev 0: 20 lines. rev 1 (huge-pr): 10 of them changed and 30 added, ~50 diff lines per file.
    lines = [f"generated {a}/{b} line {k} {'value = ' + str(k * 7)}\n" for k in range(20)]
    if rev:
        for k in range(0, 20, 2):
            lines[k] = f"generated {a}/{b} line {k} value = {k * 7 + 1}  # changed in huge-pr\n"
        lines += [f"generated {a}/{b} added line {k}: lorem ipsum dolor sit amet {k}\n" for k in range(30)]
    return "".join(lines)


t0 = 1_577_836_800   # 2020-01-01
proc = subprocess.Popen([GIT, "fast-import", "--quiet", "--done"], cwd=BIG, env=ENV, stdin=subprocess.PIPE)
w = proc.stdin.write
mark = 0


def commit(ref, msg, files, frm=None, ts=None, deletes=()):
    global mark
    mark += 1
    ts = ts or t0 + mark * 900
    m = msg.encode()
    w(b"commit %s\nmark :%d\nauthor Alice Mock <alice@example.com> %d +0000\ncommitter Alice Mock <alice@example.com> %d +0000\ndata %d\n%s\n"
      % (ref.encode(), mark, ts, ts, len(m), m))
    if frm:
        w(b"from :%d\n" % frm)
    for p in deletes:
        w(b"D %s\n" % p.encode())
    for p, text in files:
        w(b"M 100644 inline %s\n" % p.encode() + blob_cmd(text))
    return mark


first = commit("refs/heads/main", "Initial import", [(f"src/mod{i:02d}/file{j}.txt", small_file(i, j, 0)) for i in range(50) for j in range(10)]
               + [("README.md", "# big\n\nA mock repository for Loupe's memory stress test.\n")])
main_marks = [first]
for n in range(2, args.commits - 3):
    i, j = n % 50, (n // 50) % 10
    main_marks.append(commit("refs/heads/main", f"mod{i:02d}: change {n} in file{j}\n\nCommit body for {n}.", [(f"src/mod{i:02d}/file{j}.txt", small_file(i, j, n))]))
    if n % 20000 == 0:
        log(f"  {n} commits")
huge = commit("refs/heads/main", "huge: generate 5000 files", [(f"gen/g{a:02d}/f{b:03d}.txt", gen_file(a, b, 0)) for a in range(50) for b in range(100)])
big150k = "".join(f"line {k:06d}: the quick brown fox jumps over the lazy dog {k * 31 % 997}\n" for k in range(150_000))
chunk = "".join(f"function f{k}(a,b){{return a*{k}+b-{k % 13}}};var v{k}=f{k}({k},{k % 7});" for k in range(450_000))
minified = "\n".join(chunk[i:i + 10_500_000] for i in range(0, 30_000_000, 10_500_000))[:30_000_000] + "\n"
large = commit("refs/heads/main", "Add large.txt (150k lines) and bundle.min.js (30 MB)", [("large.txt", big150k), ("bundle.min.js", minified)])
tip = commit("refs/heads/main", "Tip: small change after the large files", [("README.md", "# big\n\nTip.\n")])
main_marks += [huge, large, tip]
# huge-pr: every generated file changed
commit("refs/heads/huge-pr", "huge-pr: rewrite 5000 generated files", [(f"gen/g{a:02d}/f{b:03d}.txt", gen_file(a, b, 1)) for a in range(50) for b in range(100)], frm=tip)
# feature branches: one commit each on top of an older main commit
step = max(1, len(main_marks) // (args.branches + 1))
for k in range(args.branches):
    base = main_marks[-1 - k * step]
    commit(f"refs/heads/feature/b{k:03d}", f"feature b{k:03d}: two files", [(f"src/mod{k % 50:02d}/file{k % 10}.txt", small_file(k % 50, k % 10, 777000 + k)),
                                                                            (f"feature/b{k:03d}.md", f"# b{k}\n" + "detail\n" * 30)], frm=base)
tstep = max(1, len(main_marks) // args.tags)
for k in range(args.tags):
    w(b"reset refs/tags/v0.%d.0\nfrom :%d\n\n" % (k, main_marks[k * tstep]))
w(b"done\n")
proc.stdin.close()
if proc.wait() != 0:
    sys.exit("fast-import failed")
git("checkout", "-q", "-f", "main", cwd=BIG)
git("remote", "add", "origin", f"{FORGE}/acme/big.git", cwd=BIG)
for ref in git("for-each-ref", "--format=%(refname:short)", "refs/heads", cwd=BIG).split("\n"):
    git("update-ref", f"refs/remotes/origin/{ref}", ref, cwd=BIG)
git("symbolic-ref", "refs/remotes/origin/HEAD", "refs/remotes/origin/main", cwd=BIG)
git("branch", "-q", "--set-upstream-to=origin/main", "main", cwd=BIG)
log("commit-graph")
git("commit-graph", "write", "--reachable", cwd=BIG)
huge_sha = git("rev-parse", "main~2", cwd=BIG)

# Uncommitted changes in a worktree: staged, staged-then-edited, unstaged, deleted, untracked.
def dirty(wt, seed, n_mod=200, n_untracked=120, large_files=False):
    r = random.Random(seed)
    tracked = [f"src/mod{i:02d}/file{j}.txt" for i in range(50) for j in range(10)] + [f"gen/g{a:02d}/f{b:03d}.txt" for a in range(50) for b in range(100)]
    picks = r.sample(tracked, n_mod + 10)
    for p in picks[:n_mod]:
        with open(os.path.join(wt, p), "a") as f:
            f.write(f"uncommitted edit {seed}\n" * r.randint(1, 5))
    git("add", "--", *picks[:n_mod * 2 // 5], cwd=wt)                    # 40% staged
    for p in picks[:n_mod // 5]:                                          # half of those edited again: both sides
        with open(os.path.join(wt, p), "a") as f:
            f.write("edited after staging\n")
    for p in picks[n_mod:]:
        os.remove(os.path.join(wt, p))                                    # deleted
    for k in range(n_untracked):
        write(os.path.join(wt, f"new/dir{k % 10}/untracked{k:03d}.txt"), f"untracked {k}\n" * 40)
    if large_files:
        with open(os.path.join(wt, "large.txt")) as f:
            lines = f.readlines()
        for k in range(0, len(lines), 1000):
            lines[k] = lines[k].rstrip("\n") + "  // edited\n"
        with open(os.path.join(wt, "large.txt"), "w") as f:
            f.writelines(lines)
        with open(os.path.join(wt, "bundle.min.js"), "r+") as f:
            f.seek(15_000_000)
            f.write("/*edited*/")


log("main worktree changes")
dirty(BIG, 0, n_mod=50, n_untracked=20, large_files=True)
wts = []
for k in range(1, args.worktrees + 1):
    p = os.path.join(DATA, f"big-wt{k:02d}")
    git("worktree", "add", "-q", "-b", f"wt/{k:02d}", p, "main", cwd=BIG)
    dirty(p, k, large_files=(k == 1))
    wts.append(p)
    log(f"  worktree {k}")

# ---------------------------------------------------------------------------------------------------------------------
# conflict: stopped in a merge with ~50 conflicted files
# ---------------------------------------------------------------------------------------------------------------------
log("conflict repo")
CON = os.path.join(DATA, "conflict")
git("init", "-q", CON, cwd=DATA)
for k in range(50):
    write(os.path.join(CON, f"src/c{k:02d}.py"), "".join(f"def f{n}():\n    return {n}\n\n" for n in range(40)))
git("add", "-A", cwd=CON); git("commit", "-qm", "base", cwd=CON)
git("checkout", "-qb", "theirs", cwd=CON)
for k in range(50):
    p = os.path.join(CON, f"src/c{k:02d}.py")
    s = open(p).read().replace("return 5\n", "return 5 + theirs\n").replace("return 30\n", "return 30 * theirs\n")
    write(p, s)
git("commit", "-qam", "theirs: change f5 and f30", cwd=CON)
git("checkout", "-q", "main", cwd=CON)
for k in range(50):
    p = os.path.join(CON, f"src/c{k:02d}.py")
    s = open(p).read().replace("return 5\n", "return 5 + ours\n").replace("return 30\n", "return 30 - ours\n").replace("return 20\n", "return 20  # merges cleanly\n")
    write(p, s)
git("commit", "-qam", "ours: change f5 and f30", cwd=CON)
git("remote", "add", "origin", f"{FORGE}/acme/conflict.git", cwd=CON)
git("merge", "theirs", cwd=CON, check=False)

# ---------------------------------------------------------------------------------------------------------------------
# subrepo: a submodule (moved ahead, so it shows as changed) and LFS pointer files (one modified)
# ---------------------------------------------------------------------------------------------------------------------
log("submodule + LFS repo")
SUBLIB = os.path.join(DATA, "sublib")
git("init", "-q", SUBLIB, cwd=DATA)
write(os.path.join(SUBLIB, "lib.c"), "int lib(void) { return 1; }\n")
git("add", "-A", cwd=SUBLIB); git("commit", "-qm", "lib 1", cwd=SUBLIB)
SUB = os.path.join(DATA, "subrepo")
git("init", "-q", SUB, cwd=DATA)
write(os.path.join(SUB, ".gitattributes"), "*.bin filter=lfs diff=lfs merge=lfs -text\n*.png filter=lfs diff=lfs merge=lfs -text\n")


def pointer(seed, size):
    import hashlib
    return f"version https://git-lfs.github.com/spec/v1\noid sha256:{hashlib.sha256(seed.encode()).hexdigest()}\nsize {size}\n"


for k in range(40):
    write(os.path.join(SUB, f"assets/blob{k:02d}.bin"), pointer(f"blob{k}", 1_000_000 + k))
write(os.path.join(SUB, "assets/logo.png"), pointer("logo", 48_213))
write(os.path.join(SUB, "main.c"), "int main(void) { return 0; }\n")
git("add", "-A", cwd=SUB)
git("-c", "protocol.file.allow=always", "submodule", "add", "-q", SUBLIB, "lib/sublib", cwd=SUB)
git("commit", "-qm", "submodule and LFS pointers", cwd=SUB)
write(os.path.join(SUB, "lib/sublib/lib.c"), "int lib(void) { return 2; }\n")
git("commit", "-qam", "lib 2", cwd=os.path.join(SUB, "lib/sublib"))
write(os.path.join(SUB, "assets/blob00.bin"), pointer("blob0-new", 2_000_000))
git("remote", "add", "origin", f"{FORGE}/acme/subrepo.git", cwd=SUB)

# ---------------------------------------------------------------------------------------------------------------------
# stashes: ~200 stashes
# ---------------------------------------------------------------------------------------------------------------------
log("stash repo (200 stashes)")
ST = os.path.join(DATA, "stashes")
git("init", "-q", ST, cwd=DATA)
for k in range(30):
    write(os.path.join(ST, f"f{k:02d}.txt"), "".join(f"line {n}\n" for n in range(100)))
git("add", "-A", cwd=ST); git("commit", "-qm", "base", cwd=ST)
git("remote", "add", "origin", f"{FORGE}/acme/stashes.git", cwd=ST)
for k in range(200):
    for j in range(3):
        with open(os.path.join(ST, f"f{(k + j) % 30:02d}.txt"), "a") as f:
            f.write(f"stash {k} edit {j}\n" * 5)
    git("stash", "push", "-q", "-m", f"work in progress {k}", cwd=ST)

scenario = {
    "forge": FORGE, "token": "memtest-dummy-token", "user": "alice",
    "repos": [BIG, CON, SUB, ST],
    "worktrees": [BIG] + wts,
    "big": BIG, "conflict": CON, "submodule": SUB, "stashes": ST,
    "bigFileWorktree": BIG, "bigFile": "large.txt", "minifiedFile": "bundle.min.js",
    "hugeCommit": huge_sha,
    "hugePR": 1, "prs": list(range(2, 14)),
    "iterations": 50, "pause": 0.6, "logSeconds": 10, "background": True,
    "steps": ["worktrees", "tabs", "history", "bigfile", "prs", "hugepr", "checks", "merge", "create", "rebase", "stashes", "submodule"],
}
with open(os.path.join(OUT, "scenario.json"), "w") as f:
    json.dump(scenario, f, indent=2)
log(f"done: {OUT}/scenario.json (huge commit {huge_sha[:10]})")
