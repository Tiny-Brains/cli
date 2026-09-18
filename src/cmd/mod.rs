pub mod adapt;
pub mod check;
pub mod conform;
pub mod env;
pub mod games;
pub mod maps;
pub mod play;
pub mod view;

use serde_json::Value;

use crate::registry::{Game, Registry};
use crate::store;

pub fn open_game(slug: Option<&str>) -> Result<Game, String> {
    let path = Registry::find()?;
    let reg = Registry::load(&path)?;
    let slug = match slug {
        Some(s) => s.to_string(),
        None => reg.games.keys().next().cloned().ok_or("the registry lists no games")?,
    };
    reg.resolve(&slug, &path)
}

/// The loaded models of one run, keyed by what identifies a session on a node: the artifact's
/// digest and the manifest that binds it.
///
/// A node's session cache is keyed on `(digest, runtime, device, binding)` since Orion 1.8.1 --
/// the binding being what the load actually reads out of the manifest. This is the same idea with
/// one runtime and one device, and for the same reason: two manifests over one artifact are two
/// plans, and serving one from the other's session is the silent wrong answer that fix was for.
#[derive(Default)]
pub struct Models {
    loaded: std::cell::RefCell<
        std::collections::HashMap<(String, String), std::rc::Rc<crate::model::Model>>,
    >,
}

impl Models {
    pub fn new() -> Models {
        Models::default()
    }

    /// The model for one seat, loaded on first use from the local store.
    pub fn get(
        &self,
        weights_hash: &str,
        manifest_hash: &str,
    ) -> Result<std::rc::Rc<crate::model::Model>, String> {
        let key = (weights_hash.to_string(), manifest_hash.to_string());
        if let Some(m) = self.loaded.borrow().get(&key) {
            return Ok(m.clone());
        }
        let onnx = store::get(store::Kind::Weights, weights_hash)
            .ok_or_else(|| format!("no artifact stored under {weights_hash}"))?;
        let mbytes = store::get(store::Kind::Manifest, manifest_hash)
            .ok_or_else(|| format!("no manifest stored under {manifest_hash}"))?;
        let manifest: serde_json::Value = serde_json::from_slice(&mbytes)
            .map_err(|e| format!("the manifest under {manifest_hash} is not JSON: {e}"))?;
        let t = crate::timing::start();
        let model = std::rc::Rc::new(crate::model::Model::load(&manifest, &onnx)?);
        crate::timing::stop(crate::timing::P::ModelLoad, t);
        self.loaded.borrow_mut().insert(key, model.clone());
        Ok(model)
    }
}

/// `--game SLUG`, plus the positional arguments, for the commands that take nothing else.
pub fn game_and_rest(args: &[String]) -> Result<(Option<String>, Vec<String>), String> {
    let mut slug = None;
    let mut rest = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--game" => {
                i += 1;
                slug = Some(args.get(i).ok_or("--game needs a slug")?.clone());
            }
            other if !other.starts_with('-') => rest.push(other.to_string()),
            other => return Err(format!("unknown option '{other}'\n\n{}", crate::USAGE)),
        }
        i += 1;
    }
    Ok((slug, rest))
}

/// The cartridge's own reference observations: what admission validates an adapter against, and so
/// the only set on which agreeing with the platform proves anything. Read by `check` and by
/// `adapt`, from one place, because two readers of one file is two readers that can drift.
pub fn reference_observations(game: &Game) -> Result<Vec<Value>, String> {
    let refs = game
        .component
        .parent()
        .map(|d| d.join("reference").join("observations.json"))
        .filter(|p| p.exists())
        .ok_or_else(|| {
            format!(
                "{} ships no reference observations, so there is nothing to validate against.\n\
                 From a cartridge checkout that is `./build.sh`, which writes it to `dist/`.",
                game.slug
            )
        })?;
    let doc: Value = serde_json::from_str(
        &std::fs::read_to_string(&refs).map_err(|e| format!("{}: {e}", refs.display()))?,
    )
    .map_err(|e| format!("{}: {e}", refs.display()))?;
    let observations: Vec<Value> = doc["observations"].as_array().cloned().unwrap_or_default();
    if observations.is_empty() {
        return Err(format!("{} holds no observations", refs.display()));
    }
    Ok(observations)
}
