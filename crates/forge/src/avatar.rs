//! Avatar images, fetched once per URL however many rows ask at the same time. `avatar_url` is absolute and
//! public on every forge, so no token is sent.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

/// Bounded: a long session sees thousands of people.
const KEEP: usize = 500;

type Bytes = Arc<Vec<u8>>;

#[derive(Default)]
struct State {
    done: HashMap<String, Option<Bytes>>,
    order: VecDeque<String>,
    loading: HashMap<String, Arc<(Mutex<Option<Option<Bytes>>>, Condvar)>>,
}

fn state() -> &'static Mutex<State> {
    static S: OnceLock<Mutex<State>> = OnceLock::new();
    S.get_or_init(Default::default)
}

/// What's already here, without fetching. `Some(None)`: fetched, but there's no image (show initials).
pub fn cached(url: &str) -> Option<Option<Bytes>> {
    state().lock().unwrap().done.get(url).cloned()
}

/// The image bytes (PNG/JPEG as served; the UI decodes them at the size it shows), or None when it can't be
/// fetched. Blocking; concurrent calls for one URL share a single request.
pub fn load(url: &str) -> Option<Bytes> {
    let slot = {
        let mut s = state().lock().unwrap();
        if let Some(d) = s.done.get(url) { return d.clone(); }
        if let Some(w) = s.loading.get(url).cloned() {
            drop(s);
            let (m, c) = &*w;
            let g = c.wait_while(m.lock().unwrap(), |r| r.is_none()).unwrap();
            return g.clone().flatten();
        }
        let w = Arc::new((Mutex::new(None), Condvar::new()));
        s.loading.insert(url.into(), w.clone());
        w
    };
    let got = fetch(url).map(Arc::new);
    {
        let mut s = state().lock().unwrap();
        s.loading.remove(url);
        s.done.insert(url.into(), got.clone());
        s.order.push_back(url.into());
        while s.order.len() > KEEP {
            if let Some(old) = s.order.pop_front() { s.done.remove(&old); }
        }
    }
    *slot.0.lock().unwrap() = Some(got.clone());
    slot.1.notify_all();
    got
}

fn fetch(url: &str) -> Option<Vec<u8>> {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    let agent = AGENT.get_or_init(|| {
        ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(20)))
            .tls_config(ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build())
            .build()
            .into()
    });
    let mut r = agent.get(url).call().ok()?;
    r.body_mut().with_config().limit(8 << 20).read_to_vec().ok().filter(|b| !b.is_empty())
}

/// Initials fallback colour: a hue in 0..1 from the login, the same one the Swift app gave each person.
pub fn initials_hue(login: &str) -> f32 {
    let h = login.chars().fold(0u32, |a, c| (a.wrapping_mul(31).wrapping_add(c as u32)) & 0xffff);
    (h % 360) as f32 / 360.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrent_loads_share_one_request_and_failures_are_remembered() {
        let url = "http://127.0.0.1:1/avatar.png"; // refused at once
        let hs: Vec<_> = (0..4).map(|_| std::thread::spawn(move || load(url))).collect();
        for h in hs { assert!(h.join().unwrap().is_none()); }
        assert_eq!(cached(url), Some(None));
        assert!(initials_hue("alice") >= 0.0 && initials_hue("alice") < 1.0);
    }
}
