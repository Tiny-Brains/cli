//! Which games exist, and where their artifacts are.
//!
//! The same four things `loader/run.sh` writes onto the `games` row, in a file, so the CLI resolves
//! a game with no database and no network -- and the digest a competitor plays against is the same
//! string the ladder pins.
//!
//! An entry resolves by `path` (a cartridge's artifact set on disk -- a checkout's `dist/`, or an
//! image's extracted `/artifacts/`; the digest is whatever the file hashes to) or by `release` (the
//! same tree published as ONE archive on a GitHub release, unpacked under
//! `~/.cache/tinybrains/cartridges/<archive digest>/`, and refused unless the archive hashes to what
//! the registry declares and the component inside it hashes to the declared `engine`).
//!
//! One archive rather than a file per artifact, because the consumers read a tree: `check` wants
//! `reference/`, `view` wants `viz/`, `maps export` wants `maps/`. A release that pinned only the
//! component and the manifest could play a match and do nothing else, which is what it used to do.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

use crate::store;

#[derive(Debug, Deserialize)]
pub struct Registry {
    #[serde(default)]
    pub games: BTreeMap<String, GameEntry>,
}

#[derive(Debug, Deserialize)]
pub struct GameEntry {
    pub name: String,
    /// A checkout to read artifacts from, relative to the registry file. Wins over `release`.
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub repo: Option<String>,
    #[serde(default)]
    pub release: Option<String>,
    /// The artifact set as a `.tar.gz`, attached to `release`.
    #[serde(default)]
    pub artifacts: Option<Artifact>,
    /// The component's digest: the string the ladder pins as `games.active_engine_digest`.
    #[serde(default)]
    pub engine: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Artifact {
    pub file: String,
    pub sha256: String,
}

/// A game, resolved to files on this machine.
pub struct Game {
    pub slug: String,
    pub name: String,
    pub component: PathBuf,
    pub engine_digest: String,
    /// `cartridge.json`: presets, seats, limits, budgets, and the board catalogue.
    pub manifest: Value,
    /// Where the boards live as files, when the game ships them.
    pub maps_dir: Option<PathBuf>,
    pub source: String,
}

impl Registry {
    pub fn load(path: &Path) -> Result<Registry, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// `games.toml` in the working directory first: a project carries its own games the way it
    /// carries its own matches, so a clone of a game's starter kit needs no environment variable.
    pub fn find() -> Result<PathBuf, String> {
        if let Ok(p) = std::env::var("TINYBRAINS_REGISTRY") {
            return Ok(PathBuf::from(p));
        }
        let built_in = Path::new(env!("CARGO_MANIFEST_DIR")).join("../games/registry.toml");
        let cached = store::root().map(|r| r.join("registry.toml"));
        let candidates = [
            Some(PathBuf::from("games.toml")),
            Some(PathBuf::from("tinybrains.toml")),
            Some(built_in),
            cached.ok(),
        ];
        candidates.into_iter().flatten().find(|p| p.exists()).ok_or_else(|| {
            "no games registry.\n\
                 A project carries its own as `games.toml`; clone a starter kit for one that works\n\
                 (github.com/Tiny-Brains/<game>-starter),\n\
                 or point TINYBRAINS_REGISTRY at a registry.toml."
                .to_string()
        })
    }

    pub fn resolve(&self, slug: &str, registry_path: &Path) -> Result<Game, String> {
        let entry = self.games.get(slug).ok_or_else(|| {
            let known: Vec<&str> = self.games.keys().map(String::as_str).collect();
            format!("no game '{slug}' in the registry (it has: {})", known.join(", "))
        })?;
        let base = registry_path.parent().unwrap_or(Path::new("."));

        if let Some(rel) = &entry.path {
            let checkout = base.join(rel);
            let source = format!("checkout {}", checkout.display());
            return read_artifact_set(slug, &entry.name, &checkout, source);
        }

        let (repo, release, archive, engine) =
            match (&entry.repo, &entry.release, &entry.artifacts, &entry.engine) {
                (Some(r), Some(v), Some(a), Some(e)) => (r, v, a, e),
                (None, None, None, None) => {
                    return Err(format!(
                        "game '{slug}' declares neither a `path` checkout nor a `release`"
                    ));
                }
                _ => {
                    return Err(format!(
                        "game '{slug}': a release needs all four of `repo`, `release`, \
                         `artifacts` and `engine`"
                    ));
                }
            };
        let dir = fetch_release(repo, release, archive)?;
        let game = read_artifact_set(slug, &entry.name, &dir, format!("release {repo}@{release}"))?;
        // The archive's digest pins every byte in it, so this cannot fail on a download. It fails
        // on a registry whose two digests were copied from different releases -- and the engine
        // is the one a reader compares with the ladder, so it is the one that must not lie.
        if &game.engine_digest != engine {
            return Err(format!(
                "game '{slug}': release {release} carries engine\n  {}\nand the registry declares\n  {engine}",
                game.engine_digest
            ));
        }
        Ok(game)
    }
}

/// A cartridge's artifact set, laid out as `ants/dist/` and the image's `/artifacts/` both are.
fn read_artifact_set(slug: &str, name: &str, dir: &Path, source: String) -> Result<Game, String> {
    let component = find_component(dir)?;
    let bytes = std::fs::read(&component)
        .map_err(|e| format!("cannot read {}: {e}", component.display()))?;
    let maps = dir.join("maps");
    Ok(Game {
        slug: slug.to_string(),
        name: name.to_string(),
        engine_digest: store::digest(&bytes),
        component,
        manifest: read_json(&dir.join("cartridge.json"))?,
        maps_dir: maps.is_dir().then_some(maps),
        source,
    })
}

fn read_json(path: &Path) -> Result<Value, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// The component by extension, never by name: this binary knows no game.
fn find_component(checkout: &Path) -> Result<PathBuf, String> {
    let entries = std::fs::read_dir(checkout)
        .map_err(|e| format!("cannot read {}: {e}", checkout.display()))?;
    let mut found: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("wasm"))
        .collect();
    found.sort();
    found.into_iter().next().ok_or_else(|| {
        format!("no .wasm component in {} -- run its build first", checkout.display())
    })
}

/// Fetch a release's artifact archive once, and unpack it into the cache under its own digest.
///
/// Nothing reaches the cache that did not hash to the declared digest, and nothing is unpacked in
/// place: the tree is written beside its final name and renamed into it, so an interrupted run
/// leaves no half a cartridge that a later run would take for a whole one.
fn fetch_release(repo: &str, release: &str, art: &Artifact) -> Result<PathBuf, String> {
    let hex = art
        .sha256
        .strip_prefix("sha256:")
        .filter(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or_else(|| format!("{}: sha256 must be 'sha256:<64 hex>'", art.file))?;
    let cartridges = store::root()?.join("cartridges");
    let dir = cartridges.join(hex);
    if dir.is_dir() {
        return Ok(dir);
    }
    let url = format!("https://github.com/{repo}/releases/download/{release}/{}", art.file);
    eprintln!("fetching {url}");
    let bytes = store::fetch_url(&url)?;
    let got = store::digest(&bytes);
    if got != art.sha256 {
        return Err(format!(
            "{url}\n  declared {}\n  actual   {got}\nrefusing to use it",
            art.sha256
        ));
    }

    let partial = cartridges.join(format!("{hex}.partial-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&partial);
    std::fs::create_dir_all(&partial).map_err(|e| format!("{}: {e}", partial.display()))?;
    if let Err(e) = unpack(&bytes, &partial) {
        let _ = std::fs::remove_dir_all(&partial);
        return Err(format!("{url}: {e}"));
    }
    if let Err(e) = std::fs::rename(&partial, &dir) {
        let _ = std::fs::remove_dir_all(&partial);
        // Another run unpacked the same digest first; its tree is the same bytes.
        if !dir.is_dir() {
            return Err(format!("{}: {e}", dir.display()));
        }
    }
    Ok(dir)
}

/// Files and directories only. The digest already vouches for the bytes; this refuses the entry
/// kinds an artifact set never has, so a link cannot point a read outside the cache.
fn unpack(gz: &[u8], into: &Path) -> Result<(), String> {
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(gz));
    for entry in archive.entries().map_err(|e| format!("not a .tar.gz: {e}"))? {
        let mut entry = entry.map_err(|e| format!("not a .tar.gz: {e}"))?;
        let kind = entry.header().entry_type();
        if !(kind.is_file() || kind.is_dir()) {
            let name = entry.path().map(|p| p.display().to_string()).unwrap_or_default();
            return Err(format!("'{name}' is not a file or a directory"));
        }
        let name = entry.path().map(|p| p.display().to_string()).unwrap_or_default();
        if !entry.unpack_in(into).map_err(|e| format!("{name}: {e}"))? {
            return Err(format!("'{name}' would unpack outside the cartridge"));
        }
    }
    Ok(())
}

impl Game {
    /// The boards this game publishes, from `cartridge.json`'s catalogue.
    pub fn catalogue(&self) -> Vec<&Value> {
        self.manifest
            .get("maps")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().collect())
            .unwrap_or_default()
    }

    pub fn limit(&self, key: &str, dflt: u64) -> u64 {
        self.number("limits", key, dflt)
    }

    pub fn budget(&self, key: &str, dflt: u64) -> u64 {
        self.number("budgets", key, dflt)
    }

    /// No number the game owns is ever typed into this binary.
    fn number(&self, table: &str, key: &str, dflt: u64) -> u64 {
        self.manifest.get(table).and_then(|t| t.get(key)).and_then(|v| v.as_u64()).unwrap_or(dflt)
    }
}
