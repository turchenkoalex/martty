//! Images from http(s) URLs (feature `remote`). They are fetched in parallel
//! before the document is laid out, kept in memory for the session and on
//! disk for a day; past that a stale copy still beats a failed request.
//! Without the feature nothing here touches the network.

#[cfg(feature = "remote")]
pub use enabled::{fetch, prefetch};

#[cfg(not(feature = "remote"))]
pub fn fetch(_url: &str) -> Option<Vec<u8>> {
    None
}

#[cfg(not(feature = "remote"))]
pub fn prefetch(_urls: &[&str]) {}

#[cfg_attr(not(any(feature = "remote", test)), expect(dead_code))]
fn is_remote(url: &str) -> bool {
    let lower = url.get(..8).unwrap_or(url).to_ascii_lowercase();
    lower.starts_with("https://") || lower.starts_with("http://")
}

#[cfg(feature = "remote")]
mod enabled {
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex, OnceLock};
    use std::time::{Duration, SystemTime};

    use super::is_remote;

    const MAX_BYTES: u64 = 32 << 20;
    const TIMEOUT: Duration = Duration::from_secs(10);
    const FRESH: Duration = Duration::from_secs(24 * 60 * 60);
    const PARALLEL: usize = 8;

    type Memo = Mutex<HashMap<String, Option<Arc<Vec<u8>>>>>;

    /// Every URL asked for in this process, including failures, so a broken
    /// link costs one timeout and not one per relayout.
    fn memo() -> &'static Memo {
        static MEMO: OnceLock<Memo> = OnceLock::new();
        MEMO.get_or_init(Memo::default)
    }

    pub fn fetch(url: &str) -> Option<Vec<u8>> {
        if !is_remote(url) {
            return None;
        }
        let known = memo().lock().ok()?.get(url).cloned();
        let data = match known {
            Some(data) => data,
            None => {
                let data = load(url).map(Arc::new);
                memo().lock().ok()?.insert(url.to_owned(), data.clone());
                data
            }
        };
        data.map(|d| d.as_ref().clone())
    }

    /// Fetches the URLs not seen yet, several at a time.
    pub fn prefetch(urls: &[&str]) {
        let Ok(seen) = memo().lock() else {
            return;
        };
        let mut todo: Vec<&str> = urls
            .iter()
            .copied()
            .filter(|u| is_remote(u) && !seen.contains_key(*u))
            .collect();
        drop(seen);
        todo.sort_unstable();
        todo.dedup();
        for batch in todo.chunks(PARALLEL) {
            let results: Vec<(&str, Option<Vec<u8>>)> = std::thread::scope(|s| {
                let handles: Vec<_> = batch.iter().map(|u| (*u, s.spawn(|| load(u)))).collect();
                handles
                    .into_iter()
                    .map(|(u, h)| (u, h.join().ok().flatten()))
                    .collect()
            });
            if let Ok(mut memo) = memo().lock() {
                for (url, data) in results {
                    memo.insert(url.to_owned(), data.map(Arc::new));
                }
            }
        }
    }

    /// From the disk cache while fresh, else from the network, else stale.
    fn load(url: &str) -> Option<Vec<u8>> {
        let path = cache_path(url);
        let age = path
            .as_ref()
            .and_then(|p| p.metadata().ok()?.modified().ok())
            .and_then(|t| SystemTime::now().duration_since(t).ok());
        let cached = || path.as_ref().and_then(|p| std::fs::read(p).ok());
        if age.is_some_and(|a| a < FRESH) {
            return cached();
        }
        match download(url) {
            Some(data) => {
                if let Some(p) = &path {
                    store(p, &data);
                }
                Some(data)
            }
            None => cached(),
        }
    }

    fn download(url: &str) -> Option<Vec<u8>> {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(TIMEOUT))
            .user_agent(concat!("martty/", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        let mut response = agent.get(url).call().ok()?;
        response
            .body_mut()
            .with_config()
            .limit(MAX_BYTES)
            .read_to_vec()
            .ok()
    }

    /// Written to a temporary file first, so a reader never sees half a file.
    fn store(path: &PathBuf, data: &[u8]) {
        let Some(dir) = path.parent() else {
            return;
        };
        let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
        if std::fs::create_dir_all(dir).is_ok() && std::fs::write(&tmp, data).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }

    /// `~/Library/Caches/martty` on macOS, the XDG cache dir elsewhere; one
    /// file per URL, named by its FNV-1a hash.
    fn cache_path(url: &str) -> Option<PathBuf> {
        let home = || std::env::var_os("HOME").map(PathBuf::from);
        let base = if cfg!(target_os = "macos") {
            home()?.join("Library/Caches")
        } else {
            std::env::var_os("XDG_CACHE_HOME")
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .or_else(|| Some(home()?.join(".cache")))?
        };
        let hash = url.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
        });
        Some(base.join("martty").join(format!("{hash:016x}")))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn cache_path_should_differ_per_url() {
            assert_ne!(cache_path("https://a/1.png"), cache_path("https://a/2.png"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_remote_should_accept_only_http() {
        assert!(is_remote("https://example.com/a.png"));
        assert!(is_remote("HTTP://example.com/a.png"));
        assert!(!is_remote("file:///a.png"));
        assert!(!is_remote("img/a.png"));
    }
}
