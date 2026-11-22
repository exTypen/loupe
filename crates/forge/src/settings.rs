//! The persisted settings: the Swift app's UserDefaults domain `dev.extypen.loupe`, same keys and value shapes,
//! so an existing install keeps its settings.
//!
//! Read and written through cfprefsd (`defaults export` / `defaults import`) and parsed with the plist crate:
//! writing ~/Library/Preferences/dev.extypen.loupe.plist directly would race cfprefsd's cache, which can flush an
//! older copy over it. Falls back to reading the file when `defaults` isn't usable.
//!
//! Keys (Swift type → plist value):
//! - `accounts`: JSON data of [`Account`]s. `spaces`: JSON data of [`Space`]s. `currentSpace`: string.
//! - `repos`, `selection`: pre-spaces repo list and selection (read once to build the default space).
//! - `recent` (worktree paths, MRU), `collapsed` (repo paths): string arrays.
//! - `tab` ("changes" | "history" | "pulls"), `settingsTab`, `appearance` ("system" | "light" | "dark"),
//!   `diffSide`, `filePanel`, `filePanelWidth` (real), `markdownPreview`, `commitOverview`, `prFullMode`,
//!   `pullsMine`, `lfsMissingBannerDismissed` (bools), `prBannerDismissed` ("<path>\t<branch>" lines).
//! - `editorLightTheme`, `editorDarkTheme`, `editorFont`, `editorFontSize`, `editorLineHeight`.
//! - `aiHarness` ("auto" | "off" | harness id), `aiStyle`, `aiInstructions`, `aiModel.<harness id>`.
//! - `authorAvatars`: dict email → avatar URL.
//! - `viewed:<host>/<owner>/<repo>#<n>`: dict path → fingerprint as a decimal string.
//! - `drafts:<host>/<owner>/<repo>#<n>`: JSON data of [`DraftComment`]s.
//!
//! Per-PR keys exist for every PR ever opened: an empty value removes its key instead of keeping it.

use crate::account::{Account, DEFAULT_SPACE};
use crate::models::DraftComment;
use crate::{Error, RemoteRepo, Result};
use plist::{Dictionary, Value};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Duration;

pub const DOMAIN: &str = "dev.extypen.loupe";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Space {
    pub id: String,
    pub name: String,
    pub repos: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selection: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
enum Target {
    Domain(String),
    File(PathBuf),
}

#[derive(Debug, Clone)]
pub struct Prefs {
    dict: Dictionary,
    target: Target,
    shared: bool,
}

/// The app's settings, loaded on first use. Changes made through it are saved shortly after, off the caller's
/// thread.
pub fn shared() -> MutexGuard<'static, Prefs> {
    static P: OnceLock<Mutex<Prefs>> = OnceLock::new();
    let domain = SHARED_DOMAIN.get_or_init(|| DOMAIN.into());
    P.get_or_init(|| Mutex::new(Prefs { shared: true, ..Prefs::load_domain(domain) })).lock().unwrap()
}

static SHARED_DOMAIN: OnceLock<String> = OnceLock::new();

/// Points `shared()` at another domain (test and scenario runs), wiping it first. Call before anything reads
/// settings; false when `shared()` already picked its domain.
pub fn use_fresh_domain(domain: &str) -> bool {
    if SHARED_DOMAIN.set(domain.into()).is_err() { return false }
    let _ = Command::new("/usr/bin/defaults").args(["delete", domain]).stdout(Stdio::null()).stderr(Stdio::null()).status();
    true
}

/// The domain `shared()` reads and writes.
pub fn shared_domain() -> &'static str { SHARED_DOMAIN.get_or_init(|| DOMAIN.into()) }

fn schedule_save() {
    static PENDING: AtomicBool = AtomicBool::new(false);
    if PENDING.swap(true, Ordering::SeqCst) { return; }
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_millis(300)); // one write for a burst of changes
        PENDING.store(false, Ordering::SeqCst);
        let p = shared().clone();
        let _ = p.save();
    });
}

pub fn preferences_file(domain: &str) -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(format!("Library/Preferences/{domain}.plist"))
}

impl Prefs {
    pub fn load_domain(domain: &str) -> Self {
        let out = Command::new("/usr/bin/defaults").args(["export", domain, "-"]).stderr(Stdio::null()).output();
        let dict = out
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| Value::from_reader(std::io::Cursor::new(o.stdout)).ok())
            .or_else(|| Value::from_file(preferences_file(domain)).ok())
            .and_then(Value::into_dictionary)
            .unwrap_or_default();
        Self { dict, target: Target::Domain(domain.into()), shared: false }
    }

    /// A plain plist file (tests, migration tools).
    pub fn open_file(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let dict = Value::from_file(&path).ok().and_then(Value::into_dictionary).unwrap_or_default();
        Self { dict, target: Target::File(path), shared: false }
    }

    pub fn save(&self) -> Result<()> {
        let mut xml = Vec::new();
        Value::Dictionary(self.dict.clone()).to_writer_xml(&mut xml).map_err(|e| Error::msg(e.to_string()))?;
        match &self.target {
            Target::File(p) => {
                let tmp = p.with_extension("plist.tmp");
                std::fs::write(&tmp, &xml).and_then(|_| std::fs::rename(&tmp, p)).map_err(|e| Error::msg(e.to_string()))
            }
            Target::Domain(d) => {
                let mut child = Command::new("/usr/bin/defaults")
                    .args(["import", d, "-"])
                    .stdin(Stdio::piped())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .map_err(|e| Error::msg(e.to_string()))?;
                child.stdin.take().unwrap().write_all(&xml).map_err(|e| Error::msg(e.to_string()))?;
                let ok = child.wait().map_err(|e| Error::msg(e.to_string()))?.success();
                if ok { Ok(()) } else { Err(Error::msg("defaults import failed")) }
            }
        }
    }

    // MARK: Generic access

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.dict.get(key)
    }

    /// `None` removes the key.
    pub fn set(&mut self, key: &str, value: Option<Value>) {
        match value {
            Some(v) => {
                if self.dict.get(key) == Some(&v) { return; }
                self.dict.insert(key.into(), v);
            }
            None => {
                if self.dict.remove(key).is_none() { return; }
            }
        }
        if self.shared { schedule_save(); }
    }

    pub fn string(&self, key: &str) -> Option<String> {
        self.get(key)?.as_string().map(str::to_string)
    }
    /// UserDefaults' bool(forKey:): false when missing; numbers and "YES"/"true" strings count too.
    pub fn bool(&self, key: &str) -> bool {
        match self.get(key) {
            Some(Value::Boolean(b)) => *b,
            Some(Value::Integer(i)) => i.as_signed().unwrap_or(0) != 0,
            Some(Value::Real(r)) => *r != 0.0,
            Some(Value::String(s)) => matches!(s.to_lowercase().as_str(), "yes" | "true" | "1"),
            _ => false,
        }
    }
    pub fn f64(&self, key: &str) -> Option<f64> {
        match self.get(key)? {
            Value::Real(r) => Some(*r),
            Value::Integer(i) => i.as_signed().map(|i| i as f64),
            Value::String(s) => s.parse().ok(),
            _ => None,
        }
    }
    pub fn strings(&self, key: &str) -> Vec<String> {
        self.get(key).and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_string().map(str::to_string)).collect()).unwrap_or_default()
    }
    pub fn string_map(&self, key: &str) -> HashMap<String, String> {
        self.get(key)
            .and_then(Value::as_dictionary)
            .map(|d| d.iter().filter_map(|(k, v)| v.as_string().map(|s| (k.clone(), s.to_string()))).collect())
            .unwrap_or_default()
    }
    pub fn data(&self, key: &str) -> Option<&[u8]> {
        self.get(key)?.as_data()
    }

    pub fn set_string(&mut self, key: &str, v: &str) {
        self.set(key, Some(Value::String(v.into())));
    }
    pub fn set_bool(&mut self, key: &str, v: bool) {
        self.set(key, Some(Value::Boolean(v)));
    }
    pub fn set_f64(&mut self, key: &str, v: f64) {
        self.set(key, Some(Value::Real(v)));
    }
    pub fn set_strings(&mut self, key: &str, v: &[String]) {
        self.set(key, Some(Value::Array(v.iter().map(|s| Value::String(s.clone())).collect())));
    }
    pub fn set_string_map(&mut self, key: &str, v: &HashMap<String, String>) {
        let d: Dictionary = v.iter().map(|(k, v)| (k.clone(), Value::String(v.clone()))).collect();
        self.set(key, Some(Value::Dictionary(d)));
    }
    fn json<T: for<'de> Deserialize<'de>>(&self, key: &str) -> Option<T> {
        serde_json::from_slice(self.data(key)?).ok()
    }
    fn set_json(&mut self, key: &str, v: &impl Serialize) {
        self.set(key, Some(Value::Data(serde_json::to_vec(v).unwrap())));
    }

    // MARK: Typed

    pub fn accounts(&self) -> Vec<Account> {
        self.json("accounts").unwrap_or_default()
    }
    pub fn set_accounts(&mut self, a: &[Account]) {
        self.set_json("accounts", &a);
    }

    /// The spaces; before spaces existed, one default space made of `repos` and `selection`.
    pub fn spaces(&self) -> Vec<Space> {
        self.json("spaces").unwrap_or_else(|| {
            vec![Space { id: DEFAULT_SPACE.into(), name: "Default".into(), repos: self.strings("repos"), selection: self.string("selection") }]
        })
    }
    pub fn set_spaces(&mut self, s: &[Space]) {
        self.set_json("spaces", &s);
    }
    pub fn current_space(&self) -> String {
        self.string("currentSpace").unwrap_or_else(|| DEFAULT_SPACE.into())
    }

    pub fn viewed(&self, repo: &RemoteRepo, n: i64) -> HashMap<String, u64> {
        self.string_map(&viewed_key(repo, n)).into_iter().filter_map(|(k, v)| v.parse().ok().map(|v| (k, v))).collect()
    }
    pub fn set_viewed(&mut self, repo: &RemoteRepo, n: i64, v: &HashMap<String, u64>) {
        let key = viewed_key(repo, n);
        if v.is_empty() { return self.set(&key, None); }
        self.set_string_map(&key, &v.iter().map(|(k, v)| (k.clone(), v.to_string())).collect());
    }

    pub fn drafts(&self, repo: &RemoteRepo, n: i64) -> Vec<DraftComment> {
        self.json(&drafts_key(repo, n)).unwrap_or_default()
    }
    pub fn set_drafts(&mut self, repo: &RemoteRepo, n: i64, d: &[DraftComment]) {
        let key = drafts_key(repo, n);
        if d.is_empty() { return self.set(&key, None); }
        self.set_json(&key, &d);
    }

    /// "Don't offer a pull request for this branch again".
    pub fn pr_banner_dismissed(&self, path: &str, branch: &str) -> bool {
        let key = format!("{path}\t{branch}");
        self.string("prBannerDismissed").unwrap_or_default().split('\n').any(|l| l == key)
    }
    pub fn dismiss_pr_banner(&mut self, path: &str, branch: &str) {
        let cur = self.string("prBannerDismissed").unwrap_or_default();
        let next = if cur.is_empty() { format!("{path}\t{branch}") } else { format!("{cur}\n{path}\t{branch}") };
        self.set_string("prBannerDismissed", &next);
    }
}

pub fn viewed_key(r: &RemoteRepo, n: i64) -> String {
    format!("viewed:{}#{n}", r.key())
}
pub fn drafts_key(r: &RemoteRepo, n: i64) -> String {
    format!("drafts:{}#{n}", r.key())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_swift_shapes_and_removes_empty_per_pr_keys() {
        let dir = std::env::temp_dir().join(format!("loupe-prefs-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("p.plist");
        // As UserDefaults stores them: JSON blobs as data, viewed as a dict of decimal strings.
        let mut d = Dictionary::new();
        d.insert("accounts".into(), Value::Data(br#"[{"baseURL":"https://git.example.com","username":"alice","space":"S"}]"#.to_vec()));
        d.insert("repos".into(), Value::Array(vec![Value::String("/r".into())]));
        d.insert("prFullMode".into(), Value::Boolean(true));
        let mut v = Dictionary::new();
        v.insert("a.rs".into(), Value::String("18446744073709551615".into()));
        d.insert("viewed:h/o/r#7".into(), Value::Dictionary(v));
        Value::Dictionary(d).to_file_binary(&path).unwrap();

        let mut p = Prefs::open_file(&path);
        assert_eq!(p.accounts()[0].id(), "S https://git.example.com");
        assert_eq!(p.spaces()[0].repos, ["/r"]);
        assert_eq!(p.current_space(), "default");
        assert!(p.bool("prFullMode") && !p.bool("missing"));
        let r = RemoteRepo { host: "h".into(), owner: "o".into(), name: "r".into() };
        assert_eq!(p.viewed(&r, 7)["a.rs"], u64::MAX);

        p.set_viewed(&r, 7, &HashMap::new());
        assert!(p.get("viewed:h/o/r#7").is_none());
        let draft = DraftComment::new("a", Some(1), None, "x");
        p.set_drafts(&r, 7, std::slice::from_ref(&draft));
        assert!(p.get("drafts:h/o/r#7").is_some());
        p.dismiss_pr_banner("/r", "feat");
        p.save().unwrap();

        let q = Prefs::open_file(&path);
        assert_eq!(q.drafts(&r, 7), [draft]);
        assert!(q.pr_banner_dismissed("/r", "feat") && !q.pr_banner_dismissed("/r", "main"));
        let mut q = q;
        q.set_drafts(&r, 7, &[]);
        assert!(q.get("drafts:h/o/r#7").is_none());
        // Swift's DraftComment JSON decodes (UUID as an uppercase string).
        let swift = br#"[{"id":"E621E1F8-C36C-495A-93FC-0C247A3E6E5F","path":"p","newLine":3,"body":"b"}]"#;
        let ds: Vec<DraftComment> = serde_json::from_slice(swift).unwrap();
        assert_eq!((ds[0].new_line, ds[0].old_line), (Some(3), None));
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Reads the real domain without writing it: cargo test -p loupe-forge -- --ignored real_settings
    #[test]
    #[ignore]
    fn real_settings_load() {
        let p = Prefs::load_domain(DOMAIN);
        println!("accounts: {:?}\nspaces: {:?}\ntab: {:?}", p.accounts(), p.spaces(), p.string("tab"));
    }
}
