//! LOUPE_STRESS=<scenario.json>: the test mode every flow is checked in (StressDriver.swift's bootstrap, without
//! its stress steps). The app then:
//! - uses its own settings domain, `dev.extypen.loupe.flows-gpui`, wiped at launch, and never the real
//!   `dev.extypen.loupe` domain or the Keychain;
//! - opens the scenario's repos in the default space (the `big` one selected) with an in-memory account for the
//!   mock forge (`forge`, `user`, `token`);
//! - talks plain http to it (ureq allows it; nothing to switch on).
use loupe_core::model::{DEFAULT_SPACE, Space};
use loupe_forge::settings;
use serde::Deserialize;

pub const DOMAIN: &str = "dev.extypen.loupe.flows-gpui";

/// The fields the bootstrap needs; the rest of StressDriver's format (steps, iterations, …) is ignored, and missing
/// fields default.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Scenario {
    pub forge: String,
    pub token: String,
    pub user: String,
    pub repos: Vec<String>,
    pub big: Option<String>,
}

/// Reads the scenario named by LOUPE_STRESS and prepares its settings domain. Exits 2 on an unreadable file, as the
/// Swift app did. Must run before anything reads settings.
pub fn from_env() -> Option<Scenario> {
    let path = std::env::var("LOUPE_STRESS").ok().filter(|p| !p.is_empty())?;
    let s: Scenario = match std::fs::read(&path).map_err(|e| e.to_string()).and_then(|d| serde_json::from_slice(&d).map_err(|e| e.to_string())) {
        Ok(s) => s,
        Err(e) => { eprintln!("LOUPE_STRESS: can't read {path}: {e}"); std::process::exit(2) }
    };
    if !settings::use_fresh_domain(DOMAIN) || settings::shared_domain() != DOMAIN {
        eprintln!("LOUPE_STRESS refused: settings were read before the scenario picked its domain");
        std::process::exit(2)
    }
    let mut p = settings::shared();
    let selection = s.big.clone().or_else(|| s.repos.first().cloned());
    let spaces = vec![Space { id: DEFAULT_SPACE.into(), name: "Default".into(), repos: s.repos.clone(), selection }];
    p.set("spaces", Some(plist::Value::Data(serde_json::to_vec(&spaces).unwrap_or_default())));
    p.set_string("tab", "changes");
    p.set_bool("prFullMode", false);
    p.set_string("diffSide", "diff");
    p.set_bool("lfsMissingBannerDismissed", true);
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_stress_driver_format_and_tolerates_extra_or_missing_fields() {
        let s: Scenario = serde_json::from_str(r#"{"forge":"http://127.0.0.1:8765","token":"t","user":"alice",
            "repos":["/a","/b"],"big":"/a","steps":["worktrees"],"iterations":50,"hugePR":1}"#).unwrap();
        assert_eq!((s.forge.as_str(), s.user.as_str(), s.repos.len(), s.big.as_deref()), ("http://127.0.0.1:8765", "alice", 2, Some("/a")));
        let s: Scenario = serde_json::from_str(r#"{"repos":["/x"]}"#).unwrap();
        assert!(s.token.is_empty() && s.big.is_none());
    }
}
