//! The wave: claim-free, lease-free, and otherwise Kalam's.
//!
//! The one part of the CLI that is a **second implementation**. Kalam expresses this loop as an
//! Orion workflow of JSONLogic; this expresses it as Rust. `tinybrains conform` is what keeps the
//! copy honest. Five behaviours are Kalam's and not the engine's -- each drawn from
//! `kalam/scripts/gen-kalam.py`, and each silently wrong if copied wrongly:
//!
//! 1. `actions` uses the explicit `{m, seat, action}` form. The positional form only aligns while
//!    every live seat is played, and a forfeited seat is not sent at all.
//! 2. A forfeited seat is omitted from the play call entirely -- omission *is* the no-op. So is a
//!    seat with no ants: from three seats up an eliminated colony stays in the match with an empty
//!    `mine`, has nothing to order and so nothing to miss, and is asked again the turn its hive
//!    spawns one (Kalam's `seat_plays`).
//! 3. Strikes are cumulative across the match, not consecutive, so a seat cannot game the rule by
//!    hiccupping every fourth turn.
//! 4. A forfeited seat's rank is `engine_rank + seat_count`, so two forfeits cannot tie with a seat
//!    that played. The engine's own ranks go into the envelope untouched.
//! 5. `refs` are a flat list, each carrying its own `m` and `seat`, echoed and never inspected.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use crate::cartridge::{Cartridge, Fault};
use crate::cmd::Models;
use crate::matchfile::{MatchFile, Row};
use crate::registry::Game;

pub struct Outcome {
    pub id: String,
    pub reason: String,
    pub turns: u64,
    pub ranks: Vec<i64>,
    pub scores: Vec<i64>,
    pub strikes: Vec<u64>,
    pub map_id: String,
    pub envelope: Value,
}

/// One seat, as it travels: out through `observe`, onto the play row, back on the loader's echoed
/// `ref`, and into the next turn. Neither the engine nor the loader looks inside one.
#[derive(Clone)]
struct Ref {
    m: usize,
    seat: u64,
    weights_hash: String,
    manifest_hash: String,
    strikes: u64,
    forfeited: bool,
    script: Option<Vec<Value>>,
    /// Per-seat inference cost, accumulated across the match. Held in Rust rather than in the wire
    /// `ref` (see `to_json`): Kalam carries `strikes` in its ref only because a workflow has
    /// nowhere else to keep it, and this loop has a struct.
    infer_us_total: u64,
    infer_us_max: u64,
    /// Turns this seat was actually played, so a mean survives a seat that forfeited early.
    seat_turns: u64,
}

impl Ref {
    fn to_json(&self) -> Value {
        json!({
            "m": self.m, "seat": self.seat,
            "weights_hash": self.weights_hash, "manifest_hash": self.manifest_hash,
            "strikes": self.strikes, "forfeited": self.forfeited,
        })
        // Deliberately NOT the timing fields: this shape is Kalam's ref, and the loader echoes it
        // verbatim onto every row. Widening it here would put the two implementations' wire
        // payloads out of step for a number this side already has in memory.
    }
}

pub struct Report {
    pub outcomes: Vec<Outcome>,
    pub turns_played: u64,
    pub play_calls: u64,
    /// One model evaluated for one seat on one turn: the unit a budget is spent in, and not the
    /// same as a play call -- a wave of eight matches asks for sixteen at once.
    pub seat_turns: u64,
    pub total_ops: u64,
    /// Summed `infer_us`, which is each row's share of its own group's inference. NOT summed
    /// `elapsed_ms`: that is a latency running from a row entering the call to leaving it, so every
    /// row reports roughly the whole call and a sum over 64 rows overstates the cost 64-fold.
    pub total_infer_us: u64,
    /// The most expensive single seat-turn, which is what a turn deadline is actually spent against.
    pub max_infer_us: u64,
}

pub fn run(
    game: &Game,
    cart: &Cartridge,
    mf: &MatchFile,
    models: &Models,
    verbose: bool,
) -> Result<Report, String> {
    let max_turns = mf.var("max_turns", game.limit("max_turns", 1000));
    let turn_ms = mf.var("turn_ms", game.limit("turn_ms", 1000));
    let budget_ops = mf.var("budget_ops", game.budget("adapter_ops_max", 1_000_000));
    let strike_ceiling = mf.var("strike_ceiling", 5);

    // Every model the match file names, loaded before a turn is played -- a graph that will not
    // load is a different failure from one that plays badly, and finding out on turn 300 tells you
    // less than finding out now. This is the local shape of what a replica's roster clock does.
    for (w, m) in distinct_models(mf) {
        models.get(&w, &m)?;
    }

    // One worldgen with every seed: that is what makes this a wave and not a loop over matches, so
    // one batched play call per turn serves every match a model is in. Each row's board goes whole,
    // as the claim hands it to a runner: the component carries none to look one up in, and
    // `MatchFile::resolve_boards` has already turned every name into a board.
    let world = json!({
        "seeds": mf.rows.iter().map(|r| r.seed).collect::<Vec<_>>(),
        "maps": mf.rows.iter().map(|r| r.map.clone()).collect::<Vec<_>>(),
        "players": mf.rows[0].seat_count,
        "max_turns": max_turns,
    });
    let opened = cart.invoke(&f(game, "worldgen"), &world).map_err(fault)?;
    let mut state = opened["wave_state"].clone();
    let map_ids: Vec<String> = opened["map_ids"]
        .as_array()
        .map(|a| a.iter().map(|v| v.as_str().unwrap_or("").to_string()).collect())
        .unwrap_or_default();

    let mut refs: Vec<Ref> = mf
        .rows
        .iter()
        .enumerate()
        .flat_map(|(m, row)| {
            row.seats.iter().map(move |s| Ref {
                m,
                seat: s.seat,
                weights_hash: s.weights_hash.clone(),
                manifest_hash: s.manifest_hash.clone(),
                strikes: 0,
                infer_us_total: 0,
                infer_us_max: 0,
                seat_turns: 0,
                forfeited: false,
                script: s.script.clone(),
            })
        })
        .collect();

    let mut deltas: BTreeMap<usize, Vec<Value>> = BTreeMap::new();
    let mut report = Report {
        outcomes: Vec::new(),
        turns_played: 0,
        play_calls: 0,
        seat_turns: 0,
        total_ops: 0,
        total_infer_us: 0,
        max_infer_us: 0,
    };

    for turn_index in 0.. {
        let obs = cart
            .invoke(
                &f(game, "observe"),
                &json!({
                    "wave_state": state,
                    "refs": refs.iter().map(Ref::to_json).collect::<Vec<_>>(),
                }),
            )
            .map_err(fault)?;
        let views = obs["views"].as_array().cloned().unwrap_or_default();
        if views.is_empty() {
            break;
        }

        let mut acts: Vec<Value> = Vec::new();
        let mut playing: Vec<&Value> = Vec::new();
        // Rule 2: a forfeited seat is not sent at all.
        for v in views.iter().filter(|v| !forfeited(v)) {
            let m = v["ref"]["m"].as_u64().unwrap_or(0) as usize;
            let seat = v["ref"]["seat"].as_u64().unwrap_or(0);
            match find(&refs, m, seat).and_then(|x| x.script.as_ref()) {
                // A scripted seat's orders are read, not inferred, and never reach the loader.
                Some(script) => {
                    let ants = v["view"]["mine"].as_array().map(|a| a.len()).unwrap_or(0);
                    acts.push(json!({
                        "m": m, "seat": seat,
                        "action": scripted_orders(script, turn_index, ants),
                    }));
                }
                // Rule 2 again: no ants, nothing to ask. Kalam does not infer such a seat, so it
                // neither strikes it nor charges it a seat-turn, and neither does this.
                None if v["view"]["mine"].as_array().is_some_and(|a| a.is_empty()) => {}
                None => playing.push(v),
            }
        }

        if !playing.is_empty() {
            report.play_calls += 1;
        }
        for v in &playing {
            report.seat_turns += 1;
            let m = v["ref"]["m"].as_u64().unwrap_or(0) as usize;
            let seat = v["ref"]["seat"].as_u64().unwrap_or(0);
            let (weights, manifest) = match find(&refs, m, seat) {
                Some(r) => (r.weights_hash.clone(), r.manifest_hash.clone()),
                None => continue,
            };
            let model = models.get(&weights, &manifest)?;

            // ONE SEAT, ONE INFERENCE -- the shape `tb-match` has. The failure is the
            // competitor's and not the run's: an adapter that throws, a graph that will not run, a
            // head the platform cannot read all leave `action` null, which is a strike and a
            // no-op, exactly as a node would score it.
            let mut ops = 0;
            let mut infer_us = 0;
            let action = match model.infer(&v["view"], budget_ops) {
                Ok(inf) => {
                    ops = inf.peak_ops;
                    infer_us = inf.infer_us;
                    let t = crate::timing::start();
                    let head = read_head(&inf, &v["view"]).unwrap_or(Value::Null);
                    crate::timing::stop(crate::timing::P::Head, t);
                    head
                }
                Err(e) => {
                    if verbose {
                        eprintln!("    seat {m}/{seat}: {e}");
                    }
                    Value::Null
                }
            };
            report.total_ops += ops;
            report.total_infer_us += infer_us;
            report.max_infer_us = report.max_infer_us.max(infer_us);
            // The turn deadline is a node's to enforce and this loop cannot preempt an inference,
            // so it is reported rather than applied: a seat that takes longer than a turn here
            // would strike THERE, and knowing that before submitting is the whole point.
            if verbose && infer_us > turn_ms * 1000 {
                eprintln!(
                    "  match {m} seat {seat}: {:.1} ms is over the {turn_ms} ms turn -- this \
                     would strike on the ladder",
                    infer_us as f64 / 1000.0
                );
            }

            // Before the action check below, which `continue`s on the common path: a seat's cost
            // is charged whether or not it produced a move.
            if let Some(rf) = refs.iter_mut().find(|x| x.m == m && x.seat == seat) {
                rf.infer_us_total += infer_us;
                rf.infer_us_max = rf.infer_us_max.max(infer_us);
                rf.seat_turns += 1;
            }

            // Rule 1: the explicit form. A seat that is simply absent plays the no-op.
            if !action.is_null() {
                acts.push(json!({ "m": m, "seat": seat, "action": action }));
                continue;
            }
            // Rule 3: cumulative, and the ceiling forfeits the seat for the rest of the match.
            if let Some(rf) = refs.iter_mut().find(|x| x.m == m && x.seat == seat) {
                rf.strikes += 1;
                rf.forfeited |= rf.strikes >= strike_ceiling;
                if verbose {
                    eprintln!(
                        "  match {m} seat {seat}: no action -- strike {}{}",
                        rf.strikes,
                        if rf.forfeited { ", forfeited" } else { "" }
                    );
                }
            }
        }

        let stepped = cart
            .invoke(&f(game, "step"), &json!({ "wave_state": state, "actions": acts }))
            .map_err(fault)?;
        state = stepped["wave_state"].clone();
        report.turns_played += 1;
        crate::timing::mark_turn();
        for d in stepped["replay_delta"].as_array().cloned().unwrap_or_default() {
            deltas.entry(d["m"].as_u64().unwrap_or(0) as usize).or_default().push(d);
        }
    }

    // Every match has ended, so one `finish` answers the whole wave.
    let fin = cart.invoke(&f(game, "finish"), &json!({ "wave_state": state })).map_err(fault)?;
    let results = fin["results"].as_array().cloned().unwrap_or_default();

    for (m, row) in mf.rows.iter().enumerate() {
        let r = results
            .iter()
            .find(|r| r["m"].as_u64() == Some(m as u64))
            .ok_or_else(|| format!("the engine returned no result for match {m}"))?;
        let engine_ranks = ints(&r["ranks"]);
        let scores = ints(&r["scores"]);

        // Rule 4: forfeits rank last, and not all at the same last.
        let mut ranks = engine_ranks.clone();
        let mut strikes = vec![0u64; row.seats.len()];
        let mut timing = vec![(0u64, 0u64, 0u64); row.seats.len()];
        for rf in refs.iter().filter(|x| x.m == m) {
            let i = rf.seat as usize;
            if rf.forfeited && i < ranks.len() {
                ranks[i] = engine_ranks[i] + row.seat_count as i64;
            }
            if i < strikes.len() {
                strikes[i] = rf.strikes;
            }
            if i < timing.len() {
                timing[i] = (rf.infer_us_total, rf.infer_us_max, rf.seat_turns);
            }
        }

        let envelope = json!({
            "match_id": row.id,
            "seed": row.seed,
            "map_id": r["map_id"],
            "map": r["map"],
            "max_turns": max_turns,
            "engine_digest": game.engine_digest,
            // What ran the adapters. A local run is not a node, so it says so rather than claiming
            // a version it is not: `conform` compares the MATCH, and a replay written here that
            // claimed to be Orion's would make a skew invisible.
            "orion_version": format!("tinybrains-cli/datalogic-{}", crate::model::DATALOGIC_VERSION),
            "engine_ranks": engine_ranks,
            "scores": scores,
            "reason": r["reason"],
            "turns": r["turns"],
            // Local only, and absent from Kalam's envelope, which joins it from `match_seats`: on a
            // laptop there is no row to join to, and an unattributable replay teaches nothing.
            // `infer_us_*` is what a model COST, not how long the call took: each row's share of
            // its own group's inference, summed over the turns this seat played. Both seats of a
            // match are rows of one call, on one machine, at one instant -- so within a replay these
            // numbers are directly comparable, which is the only honest cost comparison there is.
            // `conform` does not read `seats`, so carrying a non-reproducible number here cannot
            // make a deterministic replay fail to conform.
            "seats": row.seats.iter().enumerate().map(|(i, s)| {
                let (total, max, turns) = timing.get(i).copied().unwrap_or((0, 0, 0));
                json!({
                    "seat": s.seat, "label": s.label,
                    "weights_hash": s.weights_hash, "manifest_hash": s.manifest_hash,
                    "infer_us_total": total, "infer_us_max": max, "seat_turns": turns,
                })
            }).collect::<Vec<_>>(),
            "deltas": deltas.get(&m).cloned().unwrap_or_default(),
        });

        report.outcomes.push(Outcome {
            id: row.id.clone(),
            reason: r["reason"].as_str().unwrap_or("?").to_string(),
            turns: r["turns"].as_u64().unwrap_or(0),
            ranks,
            scores,
            strikes,
            map_id: map_ids.get(m).cloned().unwrap_or_default(),
            envelope,
        });
    }

    Ok(report)
}

/// Every distinct model the match file needs. A scripted seat has none, and loading an empty hash
/// would fail a teaching example that never needed ONNX at all.
fn distinct_models(mf: &MatchFile) -> Vec<(String, String)> {
    let mut seen = BTreeSet::new();
    mf.rows
        .iter()
        .flat_map(|row| &row.seats)
        .filter(|s| s.script.is_none())
        .filter(|s| seen.insert((s.weights_hash.clone(), s.manifest_hash.clone())))
        .map(|s| (s.weights_hash.clone(), s.manifest_hash.clone()))
        .collect()
}

/// The head, read the way the platform reads it (`model::decode`).
fn read_head(inf: &crate::model::Inference, view: &Value) -> Option<Value> {
    let (shape, values) = inf.f32_output("policy")?;
    let mine: Vec<(usize, usize)> = view["mine"]
        .as_array()?
        .iter()
        .map(|p| (p[0].as_u64().unwrap_or(0) as usize, p[1].as_u64().unwrap_or(0) as usize))
        .collect();
    let cols = view["size"][1].as_u64().unwrap_or(0) as usize;
    crate::model::decode(shape, &values, &mine, cols).ok().map(Value::from)
}

fn find(refs: &[Ref], m: usize, seat: u64) -> Option<&Ref> {
    refs.iter().find(|x| x.m == m && x.seat == seat)
}

fn forfeited(view: &Value) -> bool {
    view["ref"]["forfeited"].as_bool().unwrap_or(false)
}

fn ints(v: &Value) -> Vec<i64> {
    v.as_array().map(|a| a.iter().map(|x| x.as_i64().unwrap_or(0)).collect()).unwrap_or_default()
}

/// `tb.<slug>.<label>` -- the platform's namespace convention, not this binary's knowledge of any
/// particular game.
fn f(game: &Game, name: &str) -> String {
    format!("tb.{}.{}", game.slug, name)
}

fn fault(e: Fault) -> String {
    format!("the cartridge refused: {e}")
}

/// Rows in one file must share a seat count: `worldgen` checks every board of the wave against the
/// one `players` it is given. The boards themselves may differ -- each row carries its own.
pub fn check_uniform(rows: &[Row]) -> Result<(), String> {
    let first = &rows[0];
    for r in rows.iter().skip(1) {
        if r.seat_count != first.seat_count {
            return Err(format!(
                "a wave is one seat count: '{}' has {} and '{}' has {}",
                first.id, first.seat_count, r.id, r.seat_count
            ));
        }
    }
    Ok(())
}

/// One turn of a script, sized to the ants a seat actually has. Past the end of the script a seat
/// holds, which is how a scenario stops without a turn count written in two places.
fn scripted_orders(script: &[Value], turn: usize, ants: usize) -> Value {
    let orders: Vec<Value> = match script.get(turn) {
        Some(Value::String(one)) => vec![json!(one); ants],
        Some(Value::Array(per_ant)) => {
            (0..ants).map(|i| per_ant.get(i).cloned().unwrap_or(json!("-"))).collect()
        }
        _ => vec![json!("-"); ants],
    };
    Value::Array(orders)
}
