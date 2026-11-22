//! Remotes, accounts and their Keychain tokens.

use crate::{Client, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const DEFAULT_SPACE: &str = "default";

/// A repository on a forge, parsed from a git remote URL.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RemoteRepo {
    /// "git.example.com", or "localhost:3300" (the port is kept for http(s) remotes only).
    pub host: String,
    pub owner: String,
    pub name: String,
}

impl RemoteRepo {
    /// Accepts https://host/o/r(.git), ssh://git@host:2222/o/r.git, git@host:o/r.git
    pub fn parse(remote_url: &str) -> Option<Self> {
        let mut s = remote_url.trim();
        if let Some(t) = s.strip_suffix(".git") { s = t; }
        let (host, path) = if let Some((scheme, rest)) = s.split_once("://").filter(|(sc, _)| !sc.is_empty() && !sc.contains('@')) {
            let (authority, path) = rest.split_once('/').map(|(a, p)| (a, p)).unwrap_or((rest, ""));
            let hostport = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
            let (h, port) = hostport.split_once(':').map_or((hostport, None), |(h, p)| (h, Some(p)));
            if h.is_empty() { return None; }
            let host = match port {
                Some(p) if (scheme == "http" || scheme == "https") && !p.is_empty() => format!("{h}:{p}"),
                _ => h.to_string(),
            };
            (host, path.to_string())
        } else {
            let at = s.find('@')?;
            let after = &s[at + 1..];
            let colon = after.find(':')?;
            (after[..colon].to_string(), after[colon + 1..].to_string())
        };
        let comps: Vec<&str> = path.split('/').filter(|c| !c.is_empty()).collect();
        if comps.len() < 2 { return None; }
        Some(Self { host, owner: comps[comps.len() - 2].into(), name: comps[comps.len() - 1].into() })
    }

    /// "host/owner/name": the per-repository part of persisted keys.
    pub fn key(&self) -> String {
        format!("{}/{}/{}", self.host, self.owner, self.name)
    }
}

/// An account on a forge, as persisted in the `accounts` setting (JSON array). Field names are the Swift app's.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Account {
    /// "https://git.example.com", "https://github.com"
    #[serde(rename = "baseURL")]
    pub base_url: String,
    pub username: String,
    /// None in accounts saved before GitHub support: Forgejo.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub github: Option<bool>,
    /// None: the default space (accounts saved before spaces).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub space: Option<String>,
}

impl Account {
    pub fn new(base_url: &str, username: &str, github: bool, space: &str) -> Self {
        Self {
            base_url: base_url.into(),
            username: username.into(),
            github: github.then_some(true),
            space: (space != DEFAULT_SPACE).then(|| space.to_string()),
        }
    }
    pub fn is_github(&self) -> bool {
        self.github == Some(true)
    }
    pub fn space_id(&self) -> &str {
        self.space.as_deref().unwrap_or(DEFAULT_SPACE)
    }
    /// Also the Keychain account name.
    pub fn id(&self) -> String {
        match &self.space {
            Some(s) => format!("{s} {}", self.base_url),
            None => self.base_url.clone(),
        }
    }
    /// "host" or "host:port" of the base URL.
    pub fn host_key(&self) -> String {
        let rest = self.base_url.split_once("://").map_or(self.base_url.as_str(), |(_, r)| r);
        let authority = rest.split('/').next().unwrap_or("");
        authority.rsplit_once('@').map_or(authority, |(_, h)| h).to_string()
    }

    /// Keychain reads can block for seconds (securityd decrypt, ACL checks): never on the UI thread. Load all
    /// tokens once with [`load_tokens`] and serve them from memory.
    pub fn read_token(&self) -> Option<String> {
        Keychain::get(&self.id())
    }
    /// `None` removes the token.
    pub fn save_token(&self, token: Option<&str>) {
        match token {
            Some(t) => Keychain::set(t, &self.id()),
            None => Keychain::delete(&self.id()),
        }
    }

    pub fn client(&self, token: &str) -> Client {
        Client::new(&self.base_url, token, self.is_github())
    }

    /// Makes git authenticate to this account's server with its token instead of whatever credential helper the
    /// user has (osxkeychain holds one login per host, which is what spaces are meant to get around). Only HTTPS
    /// remotes ask for credentials; SSH remotes keep using the user's keys.
    pub fn git_auth_env(&self, token: &str) -> Vec<(String, String)> {
        let key = format!("credential.{}.helper", self.base_url);
        [
            ("GIT_CONFIG_COUNT", "2"),
            ("GIT_CONFIG_KEY_0", &key),
            ("GIT_CONFIG_VALUE_0", ""), // empty resets the helper list for this URL
            ("GIT_CONFIG_KEY_1", &key),
            ("GIT_CONFIG_VALUE_1", "!f() { test \"$1\" = get && echo username=\"$LOUPE_GIT_USER\" && echo password=\"$LOUPE_GIT_TOKEN\"; }; f"),
            ("LOUPE_GIT_USER", &self.username),
            ("LOUPE_GIT_TOKEN", token),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
    }
}

/// The account for a remote: same space, same hostname (ports ignored: an SSH remote's port is not the web one).
pub fn find_account<'a>(accounts: &'a [Account], space: &str, remote: &RemoteRepo) -> Option<&'a Account> {
    let host = remote.host.split(':').next().unwrap_or("");
    accounts.iter().find(|a| a.space_id() == space && a.host_key().split(':').next() == Some(host))
}

pub fn accounts_in<'a>(accounts: &'a [Account], space: &str) -> impl Iterator<Item = &'a Account> {
    accounts.iter().filter(move |a| a.space_id() == space)
}

/// account id → token for every account that has one. Blocking: call once, off the UI thread.
pub fn load_tokens(accounts: &[Account]) -> HashMap<String, String> {
    accounts.iter().filter_map(|a| a.read_token().map(|t| (a.id(), t))).collect()
}

/// Generic passwords under the Swift app's service, so an existing install keeps its tokens.
pub struct Keychain;

impl Keychain {
    pub const SERVICE: &'static str = "dev.extypen.loupe.forgejo";

    pub fn set(value: &str, account: &str) {
        Self::delete(account);
        let _ = security_framework::passwords::set_generic_password(Self::SERVICE, account, value.as_bytes());
    }
    pub fn get(account: &str) -> Option<String> {
        let d = security_framework::passwords::get_generic_password(Self::SERVICE, account).ok()?;
        String::from_utf8(d).ok()
    }
    pub fn delete(account: &str) {
        let _ = security_framework::passwords::delete_generic_password(Self::SERVICE, account);
    }
}

/// "Add Account…": normalises the URL, checks the token by asking who it belongs to, stores the token.
/// The caller replaces any account with the same id in the `accounts` setting with the one returned.
pub fn add_account(url: &str, token: &str, github: bool, space: &str) -> Result<Account> {
    let mut base = url.trim().to_string();
    if !base.contains("://") { base = format!("https://{base}"); }
    while base.ends_with('/') { base.pop(); }
    let me = Client::new(&base, token, github).me()?;
    let acct = Account::new(&base, &me.login, github, space);
    acct.save_token(Some(token));
    Ok(acct)
}

/// `gh auth token` for a host. A GUI app's PATH has no Homebrew, so look in the usual places.
pub fn gh_token(host: &str) -> Option<String> {
    let gh = ["/opt/homebrew/bin/gh", "/usr/local/bin/gh", "/usr/bin/gh"].into_iter().find(|p| std::path::Path::new(p).exists())?;
    let out = std::process::Command::new(gh)
        .args(["auth", "token", "--hostname", host])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let t = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !t.is_empty()).then_some(t)
}

/// The host part of what the user typed in "Server URL", for `gh auth token`.
pub fn host_of(url: &str) -> String {
    let u = if url.contains("://") { url.to_string() } else { format!("https://{url}") };
    let a = Account { base_url: u, username: String::new(), github: None, space: None };
    let h = a.host_key();
    let h = h.split(':').next().unwrap_or("").to_string();
    if h.is_empty() { "github.com".into() } else { h }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_remotes() {
        let r = RemoteRepo::parse("git@git.example.com:acme/web.git").unwrap();
        assert_eq!((r.host.as_str(), r.owner.as_str(), r.name.as_str()), ("git.example.com", "acme", "web"));
        assert_eq!(RemoteRepo::parse("ssh://git@git.example.com:2222/acme/web.git").unwrap().host, "git.example.com");
        assert_eq!(RemoteRepo::parse("http://localhost:3300/loupe/demo.git").unwrap().host, "localhost:3300");
        let h = RemoteRepo::parse(" https://github.com/microsoft/vscode\n").unwrap();
        assert_eq!((h.owner.as_str(), h.name.as_str()), ("microsoft", "vscode"));
        assert!(RemoteRepo::parse("/local/path").is_none());
        assert!(RemoteRepo::parse("https://github.com/only").is_none());
    }

    #[test]
    fn accounts_keep_the_swift_shape_and_match_by_host() {
        let json = r#"[{"baseURL":"https://git.example.com","username":"alice"},{"baseURL":"https://github.com","username":"bob","github":true,"space":"S2"}]"#;
        let accts: Vec<Account> = serde_json::from_str(json).unwrap();
        assert_eq!(accts[0].id(), "https://git.example.com");
        assert_eq!(accts[1].id(), "S2 https://github.com");
        assert!(!accts[0].is_github() && accts[1].is_github());
        assert_eq!(serde_json::to_string(&accts[0]).unwrap(), r#"{"baseURL":"https://git.example.com","username":"alice"}"#);
        let r = RemoteRepo::parse("ssh://git@git.example.com:2222/acme/web.git").unwrap();
        assert_eq!(find_account(&accts, DEFAULT_SPACE, &r).map(|a| a.username.as_str()), Some("alice"));
        let gh = RemoteRepo::parse("git@github.com:o/r.git").unwrap();
        assert!(find_account(&accts, DEFAULT_SPACE, &gh).is_none());
        assert_eq!(find_account(&accts, "S2", &gh).map(|a| a.username.as_str()), Some("bob"));
        let local = Account::new("http://localhost:3300", "x", false, DEFAULT_SPACE);
        assert_eq!(local.host_key(), "localhost:3300");
        assert_eq!(local.space, None);
        assert_eq!(host_of("github.example.com/"), "github.example.com");
    }
}
