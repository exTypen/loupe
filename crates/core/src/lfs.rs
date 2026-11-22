//! Git LFS: detection, pointers, locks.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use crate::diff::{FileDiff, LineKind};
use crate::git::{self, Result};

/// What a pointer file in the repository stands for: the object's sha256 and size.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LfsPointer {
    pub oid: String,
    pub size: u64,
}

impl LfsPointer {
    /// The pointer's lines (spec v1: `version`, then sorted keys, `oid` and `size` among them).
    pub fn from_lines<S: AsRef<str>>(lines: &[S]) -> Option<Self> {
        let v = lines.first()?.as_ref();
        if !v.starts_with("version https://git-lfs.github.com/spec/") && !v.starts_with("version https://hawser.github.com/spec/") {
            return None;
        }
        let (mut oid, mut size) = (None, None);
        for l in &lines[1..] {
            let l = l.as_ref();
            if let Some(o) = l.strip_prefix("oid sha256:") { oid = Some(o.to_string()) }
            else if let Some(s) = l.strip_prefix("size ") { size = s.parse().ok() }
        }
        let oid = oid.filter(|o| o.len() == 64)?;
        Some(LfsPointer { oid, size: size? })
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::from_lines(&text.split('\n').filter(|l| !l.is_empty()).collect::<Vec<_>>())
    }

    /// Where `git lfs` keeps the object once it's been downloaded or added: `<git-common-dir>/lfs/objects/ab/cd/<oid>`.
    pub fn object_path(&self, common_dir: &str) -> String {
        format!("{common_dir}/lfs/objects/{}/{}/{}", &self.oid[..2], &self.oid[2..4], self.oid)
    }
}

/// A diff whose sides are LFS pointers (or nothing, for an added or deleted file).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LfsDiff {
    pub old: Option<LfsPointer>,
    pub new: Option<LfsPointer>,
}

impl LfsDiff {
    pub fn from_diff(d: &FileDiff) -> Option<Self> {
        if d.is_binary || d.lines.is_empty() || d.lines.len() >= 24 { return None } // a pointer is a handful of lines
        let (mut a, mut b) = (vec![], vec![]);
        for l in &d.lines {
            match l.kind {
                LineKind::Context => { a.push(d.text(l)); b.push(d.text(l)) }
                LineKind::Removed => a.push(d.text(l)),
                LineKind::Added => b.push(d.text(l)),
                _ => {}
            }
        }
        let (o, n) = (LfsPointer::from_lines(&a), LfsPointer::from_lines(&b));
        if (o.is_none() && n.is_none()) || (o.is_none() && !a.is_empty()) || (n.is_none() && !b.is_empty()) { return None }
        Some(LfsDiff { old: o, new: n })
    }
}

pub const INSTALL_HINT: &str = "brew install git-lfs && git lfs install";

/// git-lfs where git (with `git::search_path`) will find it.
pub fn installed() -> bool {
    static INSTALLED: LazyLock<bool> = LazyLock::new(|| {
        git::search_path(std::env::var("PATH").ok().as_deref()).split(':').any(|d| git::is_executable(&Path::new(d).join("git-lfs")))
    });
    *INSTALLED
}

/// A `.gitattributes` that routes some path through the LFS filter.
pub fn uses_lfs(gitattributes: &str) -> bool {
    gitattributes.lines().any(|line| {
        let t = line.trim();
        !t.starts_with('#') && t.split_whitespace().skip(1).any(|a| a == "filter=lfs")
    })
}

#[derive(serde::Deserialize)]
struct Lock {
    path: String,
    owner: Option<Owner>,
}
#[derive(serde::Deserialize)]
struct Owner { name: String }

/// `git lfs locks --json`: path → owner.
pub fn parse_locks(data: &[u8]) -> Option<HashMap<String, String>> {
    let locks: Vec<Lock> = serde_json::from_slice(data).ok()?;
    let mut out = HashMap::new();
    for l in locks { out.entry(l.path).or_insert_with(|| l.owner.map(|o| o.name).unwrap_or_else(|| "someone".into())); }
    Some(out)
}

/// The pattern `git lfs track` gets for "every file like this one": its extension, or None without one.
pub fn extension_pattern(path: &str) -> Option<String> {
    Path::new(path).extension().map(|e| format!("*.{}", e.to_string_lossy()))
}

// MARK: - Per worktree state

/// Per worktree: whether the repo uses LFS, and who holds which lock.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LfsState {
    pub used: bool,
    pub locks: HashMap<String, String>, // path → owner
    /// `git lfs locks` answered and locking wasn't turned off for the remote.
    pub locking_supported: bool,
    pub common_dir: Option<String>,
    locks_loaded: Option<Instant>,
}

/// What one status refresh found out; `LfsState::apply` takes it in.
#[derive(Clone, Debug, PartialEq)]
pub struct LfsCheck {
    pub used: bool,
    pub common_dir: Option<String>,
    /// The repository's last lock answer, still fresh.
    pub shared: Option<(HashMap<String, String>, bool)>,
    /// Time to ask the server: run `load_locks` off to the side and `apply_locks` its answer.
    pub locks_due: bool,
}

/// Last lock answer per repository (git common dir), shared by its worktrees.
static SHARED: LazyLock<Mutex<HashMap<String, (Instant, HashMap<String, String>, bool)>>> = LazyLock::new(Default::default);
/// Worktrees with a lock request in flight: an unreachable server can take longer than the minute between refreshes.
static LOADING: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Default::default);
const LOCK_INTERVAL: Duration = Duration::from_secs(60);

impl LfsState {
    /// The key locks are shared under: the repository, not the worktree.
    pub fn key(&self, path: &Path) -> String { self.common_dir.clone().unwrap_or_else(|| path.to_string_lossy().into_owned()) }

    /// On every status refresh: one small file read. The locks (a request to the server) at most once a minute per
    /// repository (11 worktrees each asking sent ~16 requests a minute to the LFS server).
    pub fn check(&self, path: &Path) -> LfsCheck {
        let common_dir = self.common_dir.clone().or_else(|| git::common_dir(path));
        let attrs = std::fs::read_to_string(path.join(".gitattributes")).unwrap_or_default();
        let used = uses_lfs(&attrs) || common_dir.as_ref().is_some_and(|c| Path::new(c).join("lfs/objects").exists());
        let mut c = LfsCheck { used, common_dir, shared: None, locks_due: false };
        if !used || !installed() { return c }
        let key = c.common_dir.clone().unwrap_or_else(|| path.to_string_lossy().into_owned());
        let mut shared = SHARED.lock().unwrap();
        if let Some((_, locks, supported)) = shared.get(&key).filter(|s| s.0.elapsed() <= LOCK_INTERVAL) {
            c.shared = Some((locks.clone(), *supported));
            return c;
        }
        if self.locks_loaded.is_none_or(|t| t.elapsed() > LOCK_INTERVAL) {
            // The others wait for this one's answer.
            shared.insert(key, (Instant::now(), self.locks.clone(), self.locking_supported));
            c.locks_due = true;
        }
        c
    }

    pub fn apply(&mut self, c: LfsCheck) {
        self.used = c.used;
        self.common_dir = c.common_dir;
        if let Some((locks, supported)) = c.shared {
            self.locks = locks;
            self.locking_supported = supported;
        }
        if c.locks_due { self.locks_loaded = Some(Instant::now()) }
    }

    pub fn apply_locks(&mut self, answer: Option<(HashMap<String, String>, bool)>) {
        if let Some((locks, supported)) = answer {
            self.locks = locks;
            self.locking_supported = supported;
        }
        self.locks_loaded = Some(Instant::now());
    }
}

/// Asks the server who holds which lock (`git lfs locks --json`) and shares the answer with the repository's other
/// worktrees. None when a request for this worktree is already running.
pub fn load_locks(path: &Path, key: &str, env: &[(String, String)]) -> Option<(HashMap<String, String>, bool)> {
    let me = path.to_string_lossy().into_owned();
    if !LOADING.lock().unwrap().insert(me.clone()) { return None }
    // git-lfs answers `[]` even when the server is unreachable or has no locking API, so the only "no" is a
    // remote with `lfs.<url>.locksverify false`, what git-lfs tells you to set for such a server.
    let verify = git::string(path, ["config", "--get-regexp", r"^lfs\..*\.locksverify$"]).unwrap_or_default();
    let off = verify.lines().any(|l| l.ends_with(" false"));
    let found = if off { None } else {
        git::git(path, ["lfs", "locks", "--json"]).env(env.iter().map(|(k, v)| (k, v))).run().ok().and_then(|d| parse_locks(&d))
    };
    let answer = (found.clone().unwrap_or_default(), found.is_some());
    SHARED.lock().unwrap().insert(key.to_string(), (Instant::now(), answer.0.clone(), answer.1));
    LOADING.lock().unwrap().remove(&me);
    Some(answer)
}

/// `git lfs track`: every file with this extension, or just this one (`--filename` keeps its name literal).
/// It edits .gitattributes, which shows up as a change to commit.
pub fn track(path: &Path, pattern: &str, literal: bool) -> Result<()> {
    let mut a = vec!["lfs", "track"];
    if literal { a.push("--filename") }
    a.extend(["--", pattern]);
    git::run(path, a).map(drop)
}

pub fn lock(path: &Path, files: &[String], unlock: bool, env: &[(String, String)]) -> Result<()> {
    for f in files {
        git::git(path, ["lfs", if unlock { "unlock" } else { "lock" }, "--", f]).env(env.iter().map(|(k, v)| (k, v))).run()?;
    }
    Ok(())
}

pub const IMAGE_TYPES: &[&str] = &["png", "jpg", "jpeg", "gif", "tif", "tiff", "bmp", "heic", "webp", "psd", "ico", "icns"];

/// Where a pointer's content can be read for a preview: the object in the store, or the file on disk when it is that
/// version (the same size, so not a pointer). Objects over 64 MB aren't previewed.
pub fn object_file(p: &LfsPointer, common_dir: Option<&str>, on_disk: Option<&Path>) -> Option<std::path::PathBuf> {
    if p.size >= 64 << 20 { return None }
    let object = common_dir.map(|c| std::path::PathBuf::from(p.object_path(c))).filter(|o| o.exists());
    object.or_else(|| on_disk.filter(|f| std::fs::metadata(f).is_ok_and(|m| m.len() == p.size)).map(Path::to_path_buf))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff;

    const POINTER_A: &str = "version https://git-lfs.github.com/spec/v1\noid sha256:068aebc5295abffe3ce5319714d0066482c1e5fddb86be2771571ace534210e5\nsize 897";

    #[test]
    fn parses_lfs_pointers() {
        let p = LfsPointer::parse(POINTER_A).unwrap();
        assert!(p.size == 897 && p.oid.starts_with("068aebc5"));
        assert_eq!(p.object_path("/r/.git"), "/r/.git/lfs/objects/06/8a/068aebc5295abffe3ce5319714d0066482c1e5fddb86be2771571ace534210e5");
        assert_eq!(LfsPointer::parse("version 1\nsize 3\n"), None);
        assert_eq!(LfsPointer::parse("version https://git-lfs.github.com/spec/v1\noid sha256:abc\nsize 3\n"), None); // not a sha256
    }

    #[test]
    fn recognizes_lfs_pointer_diffs() {
        let changed = diff::parse("diff --git a/img.png b/img.png\nindex bba3cf0..f8b2948 100644\n--- a/img.png\n+++ b/img.png\n@@ -1,3 +1,3 @@\n version https://git-lfs.github.com/spec/v1\n-oid sha256:068aebc5295abffe3ce5319714d0066482c1e5fddb86be2771571ace534210e5\n-size 897\n+oid sha256:4f5f6ea6bdb59175c8c0a654dd3aad54e349ea427c11ce5f7cc3f8a02e060a24\n+size 947\n\n");
        let lfs = LfsDiff::from_diff(&changed[0]).unwrap();
        assert!(lfs.old.as_ref().unwrap().size == 897 && lfs.new.as_ref().unwrap().size == 947 && lfs.new.unwrap().oid.starts_with("4f5f6e"));

        let added = diff::parse("diff --git a/a.bin b/a.bin\nnew file mode 100644\n--- /dev/null\n+++ b/a.bin\n@@ -0,0 +1,3 @@\n+version https://git-lfs.github.com/spec/v1\n+oid sha256:068aebc5295abffe3ce5319714d0066482c1e5fddb86be2771571ace534210e5\n+size 897\n\n");
        let a = LfsDiff::from_diff(&added[0]).unwrap();
        assert!(a.old.is_none() && a.new.unwrap().size == 897);

        let text = diff::parse("diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-x\n+y\n");
        assert_eq!(LfsDiff::from_diff(&text[0]), None);
    }

    #[test]
    fn detects_lfs_in_gitattributes() {
        assert!(uses_lfs("*.psd filter=lfs diff=lfs merge=lfs -text\n"));
        assert!(uses_lfs("*.txt text\n  assets/** filter=lfs -text\n"));
        assert!(!uses_lfs("# *.psd filter=lfs\n*.txt text eol=lf\n"));
        assert!(!uses_lfs(""));
        assert_eq!(extension_pattern("art/logo.PSD").as_deref(), Some("*.PSD"));
        assert_eq!(extension_pattern("Makefile"), None);
    }

    #[test]
    fn parses_lfs_locks() {
        let json = r#"[{"id":"1","path":"art/logo.psd","owner":{"name":"maya"},"locked_at":"2026-11-11T10:00:00Z"},{"id":"2","path":"b.bin"}]"#;
        let l = parse_locks(json.as_bytes()).unwrap();
        assert_eq!(l, HashMap::from([("art/logo.psd".to_string(), "maya".to_string()), ("b.bin".into(), "someone".into())]));
        assert_eq!(parse_locks(b"[]"), Some(HashMap::new()));
        assert_eq!(parse_locks(b"not json"), None);
    }

    /// Real git-lfs, when installed: a tracked file's diff is a pointer change, and the new object is on disk.
    #[test]
    fn lfs_pointer_diff_against_real_repo() {
        if !installed() { return }
        let d = tempfile::tempdir().unwrap();
        let dir = d.path();
        let g = |args: &[&str]| { git::git(dir, ["-c", "user.name=t", "-c", "user.email=t@t"].iter().chain(args)).run().unwrap(); };
        g(&["init", "-q"]);
        for (k, v) in [("clean", "git-lfs clean -- %f"), ("smudge", "git-lfs smudge -- %f"), ("process", "git-lfs filter-process"), ("required", "true")] {
            g(&["config", &format!("filter.lfs.{k}"), v]);
        }
        g(&["lfs", "track", "*.bin"]);
        std::fs::write(dir.join("a.bin"), vec![1u8; 500]).unwrap();
        g(&["add", "."]);
        g(&["commit", "-qm", "one"]);
        std::fs::write(dir.join("a.bin"), vec![2u8; 700]).unwrap();

        let f = git::status(dir).unwrap().files.into_iter().find(|f| f.path == "a.bin").unwrap();
        let df = git::diff_for(&f, dir, 3, None).unwrap().unwrap();
        let lfs = LfsDiff::from_diff(&df).unwrap();
        assert!(lfs.old.as_ref().unwrap().size == 500 && lfs.new.as_ref().unwrap().size == 700);
        let common = git::common_dir(dir).unwrap();
        assert!(Path::new(&lfs.old.unwrap().object_path(&common)).exists());
        let mut st = LfsState::default();
        let c = st.check(dir);
        st.apply(c);
        assert!(st.used);
    }
}
