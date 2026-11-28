//! Images from https URLs, only when turned on at run time with `--remote`
//! or `MARTTY_REMOTE=1`. They are fetched in parallel before the document is
//! laid out, kept in memory for the session and on disk for a day; past that
//! a stale copy still beats a failed request.
//!
//! A document is untrusted input, so fetching is fenced in: https only
//! (redirects too), no loopback, private or link-local addresses (checked
//! after DNS, so a public name pointing inside the network is refused as
//! well), at most `MAX_IMAGES` URLs and `MAX_TOTAL` bytes per run, and a
//! cache pruned to `CACHE_MAX_AGE` and `CACHE_MAX_BYTES`. Until `enable` is
//! called nothing here touches the network.

use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub use net::{fetch, prefetch};

const CACHE_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const CACHE_MAX_BYTES: u64 = 100 << 20;

static ENABLED: AtomicBool = AtomicBool::new(false);

/// Allows fetching for this run; off until called.
pub fn enable() {
    ENABLED.store(true, Ordering::Relaxed);
}

fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// An http(s) URL: never a local path, fetched or not.
pub fn is_remote(url: &str) -> bool {
    let lower = url.get(..8).unwrap_or(url).to_ascii_lowercase();
    lower.starts_with("https://") || lower.starts_with("http://")
}

/// Only https URLs are fetched.
fn is_https(url: &str) -> bool {
    url.get(..8)
        .is_some_and(|s| s.eq_ignore_ascii_case("https://"))
}

/// An address on the public internet: not loopback, private, link-local,
/// carrier-grade NAT, documentation, multicast or reserved, including IPv4
/// wrapped in IPv6.
fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_public_v4(v4),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public_v4(v4);
            }
            let s = v6.segments();
            let [.., a, b, c, d] = v6.octets();
            // NAT64 (64:ff9b::/96) and the deprecated IPv4-compatible form
            // (::/96, which also covers :: and ::1) carry an IPv4 address in
            // their low bits.
            if s[..6] == [0x64, 0xff9b, 0, 0, 0, 0] || s[..6] == [0; 6] {
                return is_public_v4(Ipv4Addr::new(a, b, c, d));
            }
            // 6to4 (2002::/16) and Teredo (2001::/32) tunnel to IPv4 hosts
            // that the address doesn't plainly show; nothing needs them.
            if s[0] == 0x2002 || (s[0] == 0x2001 && s[1] == 0) {
                return false;
            }
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (s[0] & 0xfe00) == 0xfc00 // unique local
                || (s[0] & 0xffc0) == 0xfe80 // link-local
                || (s[0] == 0x2001 && s[1] == 0x0db8)) // documentation
        }
    }
}

fn is_public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_multicast()
        || a == 0
        || (a == 100 && (b & 0xc0) == 64) // carrier-grade NAT, 100.64/10
        || (a == 192 && b == 0 && ip.octets()[2] == 0) // IETF, 192.0.0/24
        || (a == 198 && (b & 0xfe) == 18) // benchmarking, 198.18/15
        || a >= 240) // reserved
}

/// Whether a response's Content-Type may be an image. Missing is fine, and
/// so are the generic types some hosts (raw file servers) send; the bytes
/// are recognized by content anyway, this only refuses pages early.
fn may_be_image(content_type: Option<&str>) -> bool {
    let Some(ct) = content_type else {
        return true;
    };
    let mime = ct
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    mime.starts_with("image/")
        || mime == "application/octet-stream"
        || mime == "binary/octet-stream"
        || mime == "text/plain"
}

/// Cache files to delete: older than `CACHE_MAX_AGE`, then the oldest of
/// the rest until they fit in `CACHE_MAX_BYTES`. Entries are path, size in
/// bytes and age.
fn to_prune(mut files: Vec<(PathBuf, u64, Duration)>) -> Vec<PathBuf> {
    files.sort_by_key(|f| f.2);
    let mut kept = 0u64;
    files
        .into_iter()
        .filter_map(|(path, size, age)| {
            kept += size;
            (age > CACHE_MAX_AGE || kept > CACHE_MAX_BYTES).then_some(path)
        })
        .collect()
}

mod net {
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex, Once, OnceLock};
    use std::time::{Duration, SystemTime};

    use ureq::config::AutoHeaderValue;
    use ureq::unversioned::resolver::{DefaultResolver, ResolvedSocketAddrs, Resolver};
    use ureq::unversioned::transport::{DefaultConnector, NextTimeout};

    use super::{enabled, is_https, is_public, may_be_image, to_prune};
    use crate::progress::{self, Busy};

    /// Distinct remote images one run may load.
    const MAX_IMAGES: usize = 32;
    /// One image at most.
    const MAX_BYTES: u64 = 16 << 20;
    /// Everything downloaded in one run.
    const MAX_TOTAL: u64 = 64 << 20;
    /// Reserved for a response without `Content-Length`: a share of the
    /// budget that every parallel download can have at once.
    const UNSIZED: u64 = MAX_TOTAL / PARALLEL as u64;
    const TIMEOUT: Duration = Duration::from_secs(10);
    const FRESH: Duration = Duration::from_secs(24 * 60 * 60);
    const PARALLEL: usize = 8;

    type Data = Arc<Vec<u8>>;
    type Memo = Mutex<HashMap<String, Option<Data>>>;

    /// Bytes downloaded or reserved for downloads in flight.
    static DOWNLOADED: AtomicU64 = AtomicU64::new(0);

    /// Every URL asked for in this process, including failures, so a broken
    /// link costs one timeout and not one per relayout.
    fn memo() -> &'static Memo {
        static MEMO: OnceLock<Memo> = OnceLock::new();
        MEMO.get_or_init(Memo::default)
    }

    fn wanted(url: &str) -> bool {
        enabled() && is_https(url)
    }

    pub fn fetch(url: &str) -> Option<Data> {
        if !wanted(url) {
            return None;
        }
        let memo = memo().lock().ok()?;
        match memo.get(url) {
            Some(data) => data.clone(),
            None if memo.len() >= MAX_IMAGES => None,
            None => {
                drop(memo);
                let data = load(url).map(Arc::new);
                self::memo()
                    .lock()
                    .ok()?
                    .insert(url.to_owned(), data.clone());
                data
            }
        }
    }

    /// Fetches the URLs not seen yet, several at a time, in document order
    /// up to the per-run image limit.
    pub fn prefetch(urls: &[&str]) {
        if !enabled() {
            return;
        }
        static PRUNE: Once = Once::new();
        PRUNE.call_once(prune_cache);
        let Ok(seen) = memo().lock() else {
            return;
        };
        let mut todo: Vec<&str> = Vec::new();
        for url in urls.iter().copied() {
            if wanted(url) && !seen.contains_key(url) && !todo.contains(&url) {
                todo.push(url);
            }
        }
        todo.truncate(MAX_IMAGES.saturating_sub(seen.len()));
        drop(seen);
        if todo.is_empty() {
            return;
        }
        let busy = Busy::start(&match todo.len() {
            1 => "Downloading an image".to_owned(),
            n => format!("Downloading {n} images"),
        });
        let total = todo.len() as u64;
        let done = AtomicU64::new(0);
        let load_counted = |url: &str| {
            let data = load(url);
            busy.progress(progress::percent(
                done.fetch_add(1, Ordering::Relaxed) + 1,
                total,
            ));
            data
        };
        for batch in todo.chunks(PARALLEL) {
            let results: Vec<(&str, Option<Vec<u8>>)> = std::thread::scope(|s| {
                let handles: Vec<_> = batch
                    .iter()
                    .map(|u| (*u, s.spawn(|| load_counted(u))))
                    .collect();
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

    /// Resolves names as usual and drops addresses that aren't public, so
    /// a document can't make requests into the local network.
    #[derive(Debug, Default)]
    struct PublicOnly(DefaultResolver);

    impl Resolver for PublicOnly {
        fn resolve(
            &self,
            uri: &ureq::http::Uri,
            config: &ureq::config::Config,
            timeout: NextTimeout,
        ) -> Result<ResolvedSocketAddrs, ureq::Error> {
            let all = self.0.resolve(uri, config, timeout)?;
            let mut public = self.empty();
            for addr in all.iter().filter(|a| is_public(a.ip())) {
                public.push(*addr);
            }
            if public.is_empty() {
                return Err(ureq::Error::HostNotFound);
            }
            Ok(public)
        }
    }

    fn agent() -> &'static ureq::Agent {
        static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
        AGENT.get_or_init(|| {
            let config = ureq::Agent::config_builder()
                .timeout_global(Some(TIMEOUT))
                .https_only(true)
                // A plain name, no version to fingerprint; some hosts
                // refuse requests without one.
                .user_agent(AutoHeaderValue::Provided(Arc::new("martty".to_owned())))
                .build();
            ureq::Agent::with_parts(config, DefaultConnector::new(), PublicOnly::default())
        })
    }

    /// Downloads within what is left of the run's budget. The body's share
    /// is reserved once the headers tell its size (or `UNSIZED` without
    /// one), so parallel downloads can't overrun the budget together, nor
    /// starve each other by reserving more than they need; the unused part
    /// is given back.
    fn download(url: &str) -> Option<Vec<u8>> {
        let mut response = agent().get(url).call().ok()?;
        let headers = response.headers();
        let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
        if !may_be_image(header("content-type")) {
            return None;
        }
        let length = header("content-length").and_then(|v| v.trim().parse::<u64>().ok());
        if length.is_some_and(|n| n > MAX_BYTES) {
            return None;
        }
        let want = length.unwrap_or(UNSIZED);
        let reserved = reserve(want)?;
        let data = if reserved < want {
            None
        } else {
            response
                .body_mut()
                .with_config()
                .limit(reserved)
                .read_to_vec()
                .ok()
        };
        let used = data.as_ref().map_or(0, |d| d.len() as u64);
        DOWNLOADED.fetch_sub(reserved - used.min(reserved), Ordering::Relaxed);
        data
    }

    /// Takes up to `want` bytes of the run's budget; `None` when it is spent.
    fn reserve(want: u64) -> Option<u64> {
        let mut taken = 0;
        DOWNLOADED
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |used| {
                taken = want.min(MAX_TOTAL.saturating_sub(used));
                (taken > 0).then_some(used + taken)
            })
            .ok()?;
        Some(taken)
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

    /// Deletes old and excess files from the cache directory.
    fn prune_cache() {
        let Some(dir) = cache_dir() else {
            return;
        };
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return;
        };
        let now = SystemTime::now();
        let files = entries
            .flatten()
            .filter_map(|entry| {
                let meta = entry.metadata().ok().filter(std::fs::Metadata::is_file)?;
                let age = now
                    .duration_since(meta.modified().ok()?)
                    .unwrap_or_default();
                Some((entry.path(), meta.len(), age))
            })
            .collect();
        for path in to_prune(files) {
            let _ = std::fs::remove_file(path);
        }
    }

    /// `~/Library/Caches/martty` on macOS, the XDG cache dir elsewhere.
    fn cache_dir() -> Option<PathBuf> {
        let home = || std::env::var_os("HOME").map(PathBuf::from);
        let base = if cfg!(target_os = "macos") {
            home()?.join("Library/Caches")
        } else {
            std::env::var_os("XDG_CACHE_HOME")
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .or_else(|| Some(home()?.join(".cache")))?
        };
        Some(base.join("martty"))
    }

    /// One file per URL, named by its FNV-1a hash.
    fn cache_path(url: &str) -> Option<PathBuf> {
        let hash = url.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
        });
        Some(cache_dir()?.join(format!("{hash:016x}")))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn cache_path_should_differ_per_url() {
            assert_ne!(cache_path("https://a/1.png"), cache_path("https://a/2.png"));
        }

        // One test: both use the process-wide budget.
        #[test]
        fn reserve_should_share_the_run_budget() {
            DOWNLOADED.store(0, Ordering::Relaxed);
            let parallel: Vec<_> = (0..PARALLEL).map(|_| reserve(UNSIZED)).collect();
            assert!(parallel.iter().all(Option::is_some), "{parallel:?}");
            DOWNLOADED.store(MAX_TOTAL - 10, Ordering::Relaxed);
            assert_eq!(reserve(MAX_BYTES), Some(10));
            assert_eq!(reserve(MAX_BYTES), None);
            DOWNLOADED.store(0, Ordering::Relaxed);
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

    #[test]
    fn is_https_should_refuse_plain_http() {
        assert!(is_https("HTTPS://example.com/a.png"));
        assert!(!is_https("http://example.com/a.png"));
    }

    #[test]
    fn is_public_should_refuse_local_networks() {
        let refused = [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "::1",
            "fe80::1",
            "fd00::1",
            "::ffff:192.168.1.1",
            "64:ff9b::a00:1",
            "::7f00:1",
            "2002:7f00:1::",
            "2001:0:4136:e378::1",
        ];
        for ip in refused {
            let ip: IpAddr = ip.parse().unwrap_or_else(|e| panic!("{ip}: {e}"));
            assert!(!is_public(ip), "{ip}");
        }
        for ip in ["140.82.112.3", "2606:4700::1111", "::ffff:8.8.8.8"] {
            let ip: IpAddr = ip.parse().unwrap_or_else(|e| panic!("{ip}: {e}"));
            assert!(is_public(ip), "{ip}");
        }
    }

    #[test]
    fn may_be_image_should_refuse_pages() {
        assert!(may_be_image(None));
        assert!(may_be_image(Some("image/svg+xml; charset=utf-8")));
        assert!(may_be_image(Some("application/octet-stream")));
        assert!(!may_be_image(Some("text/html; charset=utf-8")));
        assert!(!may_be_image(Some("application/x-msdownload")));
    }

    #[test]
    fn to_prune_should_drop_old_then_oldest_over_size() {
        let day = Duration::from_secs(24 * 60 * 60);
        let mb = 1 << 20;
        let files = vec![
            (PathBuf::from("old"), 1, day * 8),
            (PathBuf::from("new"), 60 * mb, day),
            (PathBuf::from("mid"), 50 * mb, day * 2),
            (PathBuf::from("newest"), 10 * mb, Duration::ZERO),
        ];
        assert_eq!(
            to_prune(files),
            [PathBuf::from("mid"), PathBuf::from("old")]
        );
    }
}
