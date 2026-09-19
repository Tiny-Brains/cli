//! `match.json` -- the rows Kalam claims, in a file.
//!
//! Kalam's `K_WAVE` reads claimed rows out of Postgres and hands them to the workflow as
//! `data.rows`; that JSON, plus the Orion `[vars]` the wave runs under, is the entire input to a
//! match. So it is the entire input here, which makes a conformance run a diff rather than a
//! translation.
//!
//! The one local addition: a seat may name `weights`/`manifest` -- a path or a URL -- instead of
//! `weights_hash`/`manifest_hash`. A file that uses only hashes is byte-compatible with the
//! database. Self-play, an older version, a downloaded release and a baseline all fall out of that.
//!
//! **A row names its board, and there are no presets.** The claim hands
//! a runner the board itself; a file may too, or name one: an id the release ships (`tinybrains
//! maps`), or a path ending `.json`, relative to this file -- which is how a season's board, uploaded
//! to the platform and in no release, is played on a laptop. `resolve_boards` turns every name into
//! the board, and checks it seats as many as the row names.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::registry::Game;
use crate::store;

pub struct MatchFile {
    pub game: String,
    pub engine_digest: Option<String>,
    pub vars: Value,
    pub rows: Vec<Row>,
    /// The file's own directory: what a board's path, like a seat's, is relative to.
    base: PathBuf,
}

pub struct Row {
    pub id: String,
    pub seed: u64,
    pub seat_count: u64,
    /// The board: as the file wrote it -- an id, a path, or the board inline -- until
    /// `MatchFile::resolve_boards`, and the board itself after.
    pub map: Value,
    pub seats: Vec<Seat>,
}

pub struct Seat {
    pub seat: u64,
    pub weights_hash: String,
    pub manifest_hash: String,
    /// Orders written down instead of inferred, one entry per turn: either one order for every ant
    /// (`"E"`) or one per ant in `mine` order (`["E", "W"]`). Past the end of the script a seat
    /// holds. A scripted seat never reaches the loader, so a tutorial costs no ONNX and still goes
    /// through the real cartridge.
    pub script: Option<Vec<Value>>,
    /// What to call this seat in output: `sha256:1a3f…` says nothing about which model lost.
    pub label: String,
}

impl MatchFile {
    pub fn load(path: &Path) -> Result<MatchFile, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let doc: Value =
            serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        // Paths inside the file resolve against the file's own directory, so a match file and the
        // models it names travel together.
        let base: PathBuf = path.parent().unwrap_or(Path::new(".")).to_path_buf();

        let rows_json = doc
            .get("rows")
            .and_then(|v| v.as_array())
            .ok_or_else(|| format!("{}: no `rows` array", path.display()))?;
        if rows_json.is_empty() {
            return Err(format!("{}: `rows` is empty -- a wave of no matches", path.display()));
        }

        let mut rows = Vec::with_capacity(rows_json.len());
        for (i, r) in rows_json.iter().enumerate() {
            rows.push(Row::parse(r, i, &base)?);
        }

        Ok(MatchFile {
            game: doc.get("game").and_then(Value::as_str).unwrap_or("ants").to_string(),
            engine_digest: doc.get("engine_digest").and_then(Value::as_str).map(str::to_string),
            vars: doc.get("vars").cloned().unwrap_or(Value::Null),
            rows,
            base,
        })
    }

    /// Every row's board, whole, from whatever the file named it by -- and each one checked to
    /// seat the row's seats. A board's seat count is the board's own, so a row that
    /// disagrees is refused here rather than by `worldgen`, where it would name no row.
    pub fn resolve_boards(&mut self, game: &Game) -> Result<(), String> {
        for row in &mut self.rows {
            let board = game
                .resolve_board(&row.map, &self.base)
                .map_err(|e| format!("row '{}': {e}", row.id))?;
            let players = board.get("players").and_then(Value::as_u64);
            if players != Some(row.seat_count) {
                return Err(format!(
                    "row '{}': board '{}' seats {}, and the row names {} seats",
                    row.id,
                    board.get("id").and_then(Value::as_str).unwrap_or("?"),
                    players.map_or("an unknown number".to_string(), |p| p.to_string()),
                    row.seat_count
                ));
            }
            row.map = board;
        }
        Ok(())
    }

    /// A tuning number from the file, falling back to the game's own manifest.
    pub fn var(&self, key: &str, dflt: u64) -> u64 {
        self.vars.get(key).and_then(Value::as_u64).unwrap_or(dflt)
    }
}

impl Row {
    fn parse(r: &Value, index: usize, base: &Path) -> Result<Row, String> {
        // `m` is the index and `id` only names the output, so both default here; in the database
        // they are a column and a uuid.
        let id = r
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| format!("match-{index}"));
        let seed = r
            .get("seed")
            .and_then(Value::as_u64)
            .ok_or_else(|| format!("row '{id}': no `seed`"))?;
        // Refused by name rather than ignored: an older file names a preset and relied on the
        // seed to choose a board from its pool, and there is no pool any more.
        if r.get("preset").is_some() {
            return Err(format!(
                "row '{id}': `preset` is gone -- the engine carries no boards to pool.\n\
                 Name the board with `map`: an id from `tinybrains maps`, a path to a board's \
                 .json, or the board itself."
            ));
        }
        let map = r.get("map").cloned().unwrap_or(Value::Null);
        if map.is_null() {
            return Err(format!(
                "row '{id}': no `map` -- an id from `tinybrains maps`, a path to a board's .json, \
                 or the board itself"
            ));
        }

        let seats_json = r
            .get("seats")
            .and_then(|v| v.as_array())
            .ok_or_else(|| format!("row '{id}': no `seats` array"))?;
        let mut seats = Vec::with_capacity(seats_json.len());
        for (i, s) in seats_json.iter().enumerate() {
            seats.push(Seat::parse(s, i, &id, base)?);
        }

        let seat_count = r.get("seat_count").and_then(Value::as_u64).unwrap_or(seats.len() as u64);
        if seat_count as usize != seats.len() {
            return Err(format!(
                "row '{id}': seat_count is {seat_count} but {} seats are named",
                seats.len()
            ));
        }

        Ok(Row { id, seed, seat_count, map, seats })
    }
}

impl Seat {
    fn parse(s: &Value, index: usize, row: &str, base: &Path) -> Result<Seat, String> {
        let seat = s.get("seat").and_then(Value::as_u64).unwrap_or(index as u64);
        let here = format!("row '{row}' seat {seat}");
        let label = |dflt: &str| s.get("label").and_then(Value::as_str).unwrap_or(dflt).to_string();

        // A scripted seat names no model, so it must not be asked for one.
        if let Some(script) = s.get("script").and_then(|v| v.as_array()) {
            return Ok(Seat {
                seat,
                weights_hash: String::new(),
                manifest_hash: String::new(),
                script: Some(script.clone()),
                label: label("scripted"),
            });
        }

        // The production form: a seat naming hashes is already what the database holds.
        if let (Some(w), Some(a)) = (
            s.get("weights_hash").and_then(Value::as_str),
            s.get("manifest_hash").and_then(Value::as_str),
        ) {
            return Ok(Seat {
                seat,
                weights_hash: w.to_string(),
                manifest_hash: a.to_string(),
                script: None,
                label: label(store::short(w)),
            });
        }

        // The local superset: a path or a URL, read and hashed into the store.
        let w = s.get("weights").and_then(Value::as_str).ok_or_else(|| {
            format!("{here}: needs `weights_hash` + `manifest_hash`, `weights` + `manifest`, or a `script`")
        })?;
        let a = s
            .get("manifest")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{here}: has `weights` but no `manifest`"))?;

        let load = |kind, spec: &str| -> Result<String, String> {
            let bytes = store::bytes_of(spec, base).map_err(|e| format!("{here}: {e}"))?;
            store::put(kind, &bytes).map_err(|e| format!("{here}: {e}"))
        };

        Ok(Seat {
            seat,
            weights_hash: load(store::Kind::Weights, w)?,
            manifest_hash: load(store::Kind::Manifest, a)?,
            script: None,
            label: label(&name_of(w)),
        })
    }
}

/// A seat's default name: the file it was loaded from, without the extension.
fn name_of(spec: &str) -> String {
    Path::new(spec).file_stem().and_then(|s| s.to_str()).unwrap_or(spec).to_string()
}
