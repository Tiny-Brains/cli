//! The local content-addressed store: `sha256:<hex>` under `~/.cache/tinybrains/models`.
//!
//! The digest is the one the platform uses, because it is the one a submission declares and the
//! one an Orion node re-hashes the fetched object against before it will run it. Two files under
//! one hash is the same file.
//!
//! There is no host allowlist here, deliberately: a competitor testing locally points at a file on
//! disk. "It loaded locally" is therefore not "it will be admitted".

use std::path::{Path, PathBuf};

/// `~/.cache/tinybrains`, or `$TINYBRAINS_HOME`.
pub fn root() -> Result<PathBuf, String> {
    if let Ok(p) = std::env::var("TINYBRAINS_HOME") {
        return Ok(PathBuf::from(p));
    }
    dirs::cache_dir()
        .map(|d| d.join("tinybrains"))
        .ok_or_else(|| "no cache directory -- set TINYBRAINS_HOME".to_string())
}

pub fn models_dir() -> Result<PathBuf, String> {
    let d = root()?.join("models");
    std::fs::create_dir_all(&d).map_err(|e| format!("{}: {e}", d.display()))?;
    Ok(d)
}

pub fn digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("sha256:{:x}", Sha256::digest(bytes))
}

/// What a stored object is: the graph, or the manifest that declares how to feed it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Weights,
    Manifest,
}

impl Kind {
    fn dir(self) -> &'static str {
        match self {
            Kind::Weights => "weights",
            Kind::Manifest => "manifests",
        }
    }
}

/// `sha256:` plus the first twelve hex characters -- enough to tell two digests apart on one line.
pub fn short(hash: &str) -> &str {
    &hash[..19.min(hash.len())]
}

/// Store bytes under their own hash, and answer with it. Idempotent: the same bytes are the same
/// file, so a second put is a no-op rather than a rewrite.
pub fn put(kind: Kind, bytes: &[u8]) -> Result<String, String> {
    let hash = digest(bytes);
    let dir = models_dir()?.join(kind.dir());
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(hash.replace(':', "-"));
    if !path.exists() {
        std::fs::write(&path, bytes).map_err(|e| format!("cannot store {hash}: {e}"))?;
    }
    Ok(hash)
}

/// The bytes stored under `hash`, if they are here.
pub fn get(kind: Kind, hash: &str) -> Option<Vec<u8>> {
    let path = models_dir().ok()?.join(kind.dir()).join(hash.replace(':', "-"));
    std::fs::read(path).ok()
}

/// Read a local file, or fetch a URL. The one place a `weights`/`adapter` value becomes bytes.
pub fn bytes_of(spec: &str, base: &Path) -> Result<Vec<u8>, String> {
    if spec.starts_with("http://") || spec.starts_with("https://") {
        return fetch_url(spec);
    }
    let p = if Path::new(spec).is_absolute() { PathBuf::from(spec) } else { base.join(spec) };
    std::fs::read(&p).map_err(|e| format!("cannot read {}: {e}", p.display()))
}

pub fn fetch_url(url: &str) -> Result<Vec<u8>, String> {
    let resp = ureq::get(url).call().map_err(|e| format!("GET {url} failed: {e}"))?;
    let mut buf = Vec::new();
    std::io::Read::read_to_end(&mut resp.into_reader(), &mut buf)
        .map_err(|e| format!("GET {url}: {e}"))?;
    Ok(buf)
}
