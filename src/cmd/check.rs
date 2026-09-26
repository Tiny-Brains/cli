//! What admission will do, done here first: read the graph, run the manifest's adapters over the
//! game's reference observations under the season's budget, and check the head is one the platform
//! can read.
//!
//! Necessary and not sufficient. There is no download allowlist here, the size class is *reported*
//! rather than decided because that table is the season's, and a node re-hashes the object it
//! fetches — so a pass here says the submission is well formed, not that it was admitted.
//!
//! A model that declares a memory output is priced the way the admit clock prices it, and judged
//! against a weight class when `--memory-flat-bytes` and `--memory-cell-bytes` name one: the
//! class table is the season's, so without them the bytes are reported and nothing is decided.

use serde_json::{Value, json};

use crate::cmd::{game_and_rest, open_game, reference_observations};
use crate::memory::{self, Carry, Price, Verdict};
use crate::model::Model;
use crate::store;

/// A weight class's two memory numbers, as `--memory-flat-bytes` and `--memory-cell-bytes` gave
/// them. Absent means 0, as it does in a season's class table.
struct Class {
    flat: u64,
    cell: u64,
}

pub fn run(args: &[String]) -> Result<(), String> {
    // `--json` and the class numbers before the shared parser sees them: `game_and_rest` refuses an
    // unknown option, which is the behaviour every other command wants.
    let json_out = args.iter().any(|a| a == "--json");
    let (class, args) = class_of(args)?;
    let (slug, files) = game_and_rest(&args)?;
    if files.len() != 2 {
        return Err(
            "which model?\n\n  tinybrains check out/model.onnx out/manifest.json".to_string()
        );
    }
    let game = open_game(slug.as_deref())?;
    let observations = reference_observations(&game)?;

    let cwd = std::path::PathBuf::from(".");
    let wb = store::bytes_of(&files[0], &cwd)?;
    let mb = store::bytes_of(&files[1], &cwd)?;
    let weights = store::put(store::Kind::Weights, &wb)?;
    let manifest_hash = store::put(store::Kind::Manifest, &mb)?;
    let manifest: Value =
        serde_json::from_slice(&mb).map_err(|e| format!("{}: not JSON: {e}", files[1]))?;

    // The size metric: the bytes the node measures against a digest it re-hashes, plus the
    // document the submission forwards. Both terms are unforgeable, which a metric over
    // compressed initializers would not be: a graph can carry its weights somewhere else.
    let size_metric = wb.len() + mb.len();

    if !json_out {
        println!("{} against {}'s reference set", files[0], game.slug);
        println!("    weights          {weights}");
        println!("    manifest         {manifest_hash}");
        println!();
    }

    let budget = game.budget("adapter_ops_max", 1_000_000);
    // Stricter than admission, which allows more per observation than a turn does.
    let deadline = game.limit("turn_ms", 1000);

    let stats = crate::onnx::stats(&wb)?;
    // Priced from the manifest alone, as the admit clock prices it: before the graph is loaded,
    // because a memory declaration a node would refuse is the likelier reason a load fails too.
    let price = memory::price(&manifest);
    let model = Model::load(&manifest, &wb).map_err(|e| match &price {
        Err((code, why)) => format!("{code}: {why}"),
        Ok(_) => e,
    })?;
    for w in &model.warnings {
        eprintln!("warning: {w}");
    }
    if !json_out {
        report_graph(&stats, &model, size_metric);
        println!();
        println!(
            "adapters  ({} reference observations, budget {budget}, turn {deadline} ms)",
            observations.len()
        );
    }

    // Every observation is played, as the admitting runner plays them, and for a model with memory
    // they are CHAINED: observation i is fed call i-1's memory outputs when observation i-1 was on
    // a board of the same size and its call answered, so a memory input that cannot take its own
    // output fails here rather than on turn 1 of every match. No observation is played twice.
    let chained = model.declares_memory();
    let mut round_trip = RoundTrip::default();
    let mut ops_max = 0u64;
    let mut infer_us_max = 0u64;
    let mut failure: Option<(usize, String, bool)> = None;
    let mut last: Option<(Carry, &Value)> = None;
    for (i, obs) in observations.iter().enumerate() {
        let fed = chained
            && last.as_ref().is_some_and(|(c, prev)| !c.is_empty() && prev["size"] == obs["size"]);
        let none = Carry::default();
        let memory = match (&last, fed) {
            (Some((carry, _)), true) => carry,
            _ => &none,
        };
        let answer = model.infer(obs, memory, budget);
        round_trip.checked += u64::from(fed);
        last = answer.as_ref().ok().map(|inf| {
            let mut c = Carry::default();
            c.after(Some(inf));
            (c, obs)
        });
        match answer {
            Ok(inf) => {
                ops_max = ops_max.max(inf.peak_ops);
                infer_us_max = infer_us_max.max(inf.infer_us);
                // The head has to be one the platform can gather from, so `check` reads it exactly
                // as `kalam-match` does rather than merely noting that something came back.
                if let Err(e) = head_reads(&inf, obs) {
                    failure.get_or_insert((i, e, false));
                }
            }
            // A call that was fed a memory and failed is the round trip's to report.
            Err(e) if fed => {
                round_trip.failed += 1;
                round_trip.first.get_or_insert((i, e));
            }
            Err(e) => {
                let over = e.contains("budget") || e.contains("Budget");
                failure.get_or_insert((i, e, over));
            }
        }
    }
    let memory = judge_memory(&game, &price, class.as_ref());
    // ADMISSION'S ONE TIMING GATE, run as a node runs it: PROBE_RUNS inferences over zero-filled
    // inputs at the manifest's probe_dims, their median inside the game's turn. The admitting
    // runner's models.max_probe_ms IS that turn_ms (web's configs.sh checks it), so this compares
    // against the same number with no copy of it. Measured on this machine: the runner measures
    // again, and a model slow there on every attempt expires PROBE_TOO_SLOW.
    let probe = model.probe();
    let probe_ok = matches!(&probe, Ok((ms, _)) if *ms <= deadline as f64);
    let memory_ok = memory.verdict.is_none() && round_trip.failed == 0;
    // THE ONNX SURFACE, as admission reads it off the document: an operator off the allowlist is
    // OP_NOT_ALLOWED there, whatever this binary's runtime executes, and an opset outside the
    // range is refused the same way.
    let op_not_allowed = crate::onnx::refused(&stats);
    let opset_ok = crate::onnx::opset_allowed(&stats);
    let surface_ok = op_not_allowed.is_empty() && opset_ok;
    let ok = failure.is_none() && probe_ok && memory_ok && surface_ok;

    if json_out {
        // Machine-readable, for a repository that automates this -- exporting a model into a
        // weight class is a loop of build, measure, resize, and parsing prose is how that loop
        // breaks on a wording change.
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "ok": ok,
                "game": game.slug,
                "engine_digest": game.engine_digest,
                "weights_hash": weights,
                "manifest_hash": manifest_hash,
                "budget_ops": budget,
                "deadline_ms": deadline,
                "observations": observations.len(),
                "graph": {
                    "parameters": stats.parameters,
                    "nodes": stats.nodes,
                    "operators": stats.operators,
                    "ir_version": stats.ir_version,
                    "opset": stats.opset,
                },
                "size_metric_bytes": size_metric,
                "artifact_bytes": wb.len(),
                "manifest_bytes": mb.len(),
                "ops_max": ops_max,
                "infer_us_max": infer_us_max,
                "failing_case": failure.as_ref().map(|(i, ..)| *i),
                "reason": failure.as_ref().map(|(_, e, _)| e.clone()),
                "over_budget": failure.as_ref().map(|(.., o)| *o),
                "probe": {
                    "ok": probe_ok,
                    "runs": crate::model::PROBE_RUNS,
                    "median_ms": probe.as_ref().ok().map(|(ms, _)| *ms),
                    "limit_ms": deadline,
                    "dims": probe.as_ref().ok().map(|(_, dims)| dims.clone()),
                    "error": probe.as_ref().err(),
                },
                "memory": memory.json(chained.then_some(&round_trip)),
                "surface": {
                    "ok": surface_ok,
                    "op_not_allowed": op_not_allowed,
                    "opset_ok": opset_ok,
                    "opset_range": [crate::onnx::OPSET_MIN, crate::onnx::OPSET_MAX],
                },
            }))
            .map_err(|e| e.to_string())?
        );
    } else {
        match &failure {
            None => {
                let pct = ops_max * 100 / budget.max(1);
                println!("    PASSED");
                println!("    worst case       {ops_max} operations, {pct}% of the budget");
                if pct > 80 {
                    println!(
                        "    little headroom -- a busier board than any of these would exceed it"
                    );
                }
                // Reported, never a gate: no weight class caps compute, and the timing admission
                // does judge is the probe below, at probe_dims rather than at these boards.
                println!(
                    "    slowest graph    {:.2} ms of inference  (measured here, not a threshold: \
                     the probe below is admission's timing gate)",
                    infer_us_max as f64 / 1000.0
                );
            }
            Some((i, e, over)) => {
                println!("    FAILED  {e}");
                println!("    failing case     observation {i} of {}", observations.len());
                // Too expensive is not the same as wrong; collapsing them sends someone to
                // re-read the expression reference for a budget problem.
                if *over {
                    println!("    the adapter is too expensive, not incorrect");
                } else {
                    println!("    the adapter is incorrect, not merely expensive");
                }
            }
        }
        println!();
        if !op_not_allowed.is_empty() {
            println!(
                "    FAILED  OP_NOT_ALLOWED: {} -- admission refuses an operator off its allowlist, \
                 which the book's Format page lists; this runtime executing it is beside the point",
                op_not_allowed.join(", ")
            );
        }
        if !opset_ok {
            println!(
                "    FAILED  OPSET: the graph declares opset {}, and admission takes {} to {}",
                stats.opset,
                crate::onnx::OPSET_MIN,
                crate::onnx::OPSET_MAX
            );
        }
        report_probe(&probe, deadline);
        if memory.declared() {
            println!();
            memory.report(chained.then_some(&round_trip));
        }
        println!();
        println!(
            "This is not admission. It has no download allowlist and does not decide a size class,"
        );
        println!("so a pass here is necessary and not sufficient.");
    }
    if !ok {
        return Err("check failed".to_string());
    }
    Ok(())
}

/// `--memory-flat-bytes N` and `--memory-cell-bytes N` out of the arguments, and what is left. Either
/// one names a class, and the other is then 0; neither leaves the memory unjudged.
fn class_of(args: &[String]) -> Result<(Option<Class>, Vec<String>), String> {
    let mut rest = Vec::new();
    let (mut flat, mut cell) = (None, None);
    let mut i = 0;
    while i < args.len() {
        let slot = match args[i].as_str() {
            "--json" => None,
            "--memory-flat-bytes" => Some((&mut flat, memory::FLAT_BYTES_MAX)),
            "--memory-cell-bytes" => Some((&mut cell, memory::CELL_BYTES_MAX)),
            other => {
                rest.push(other.to_string());
                None
            }
        };
        if let Some((slot, max)) = slot {
            let flag = &args[i];
            i += 1;
            let n = args
                .get(i)
                .and_then(|v| v.parse::<u64>().ok())
                .filter(|n| *n <= max)
                .ok_or_else(|| format!("{flag} takes a whole number from 0 to {max}"))?;
            *slot = Some(n);
        }
        i += 1;
    }
    let class = (flat.is_some() || cell.is_some())
        .then(|| Class { flat: flat.unwrap_or(0), cell: cell.unwrap_or(0) });
    Ok((class, rest))
}

/// How the chained observations went for a model with memory: the calls that were fed their
/// predecessor's memory, and of those the ones that failed, which the admitting runner reports as
/// `probe.round_trip` and Soma refuses as `MEMORY_ROUND_TRIP`.
#[derive(Default)]
struct RoundTrip {
    checked: u64,
    failed: u64,
    first: Option<(usize, String)>,
}

/// The memory's price and, when a class was named, its verdict.
struct MemoryReport {
    price: Result<Option<Price>, Verdict>,
    /// The smallest and largest board the envelope allows, in cells, when the release declares one.
    cells: Option<(u64, u64)>,
    class: Option<(u64, u64)>,
    verdict: Option<Verdict>,
}

fn judge_memory(
    game: &crate::registry::Game,
    price: &Result<Option<Price>, Verdict>,
    class: Option<&Class>,
) -> MemoryReport {
    let cells = game.envelope().and_then(|env| {
        let side = env.get("sides")?.as_array()?.first()?.as_u64()?;
        Some((side * side, env.get("cells_max")?.as_u64()?))
    });
    let verdict = match (price, class, cells) {
        (Err(v), ..) => Some(v.clone()),
        (Ok(Some(p)), Some(c), Some((lo, hi))) => memory::judge(p, c.flat, c.cell, lo, hi).err(),
        // A release with no limits.boards has no smallest or largest board to price at.
        _ => None,
    };
    MemoryReport { price: price.clone(), cells, class: class.map(|c| (c.flat, c.cell)), verdict }
}

impl MemoryReport {
    fn declared(&self) -> bool {
        !matches!(self.price, Ok(None))
    }

    fn json(&self, round_trip: Option<&RoundTrip>) -> Value {
        if !self.declared() {
            return Value::Null;
        }
        let p = self.price.as_ref().ok().copied().flatten();
        json!({
            "fixed_bytes": p.map(|p| p.fixed),
            "cell_bytes": p.map(|p| p.per_cell),
            "cells_min": self.cells.map(|c| c.0),
            "cells_max": self.cells.map(|c| c.1),
            "bytes_at_min": p.zip(self.cells).map(|(p, c)| p.bytes(c.0)),
            "bytes_at_max": p.zip(self.cells).map(|(p, c)| p.bytes(c.1)),
            "class": self.class.map(|(f, c)| json!({"memory_flat_bytes": f, "memory_cell_bytes": c})),
            "verdict": self.verdict.as_ref().map(|v| v.0),
            "reason": self.verdict.as_ref().map(|v| v.1.clone()),
            "round_trip": round_trip.map(|r| json!({
                "checked": r.checked,
                "failed": r.failed,
                "failing_case": r.first.as_ref().map(|f| f.0),
                "reason": r.first.as_ref().map(|f| f.1.clone()),
            })),
        })
    }

    fn report(&self, round_trip: Option<&RoundTrip>) {
        println!("memory");
        if let Ok(Some(p)) = &self.price {
            println!("    fixed            {} bytes", p.fixed);
            println!("    per cell         {} bytes", p.per_cell);
            if let Some((lo, hi)) = self.cells {
                println!("    at {lo:<5} cells   {} bytes  (the smallest board)", p.bytes(lo));
                println!("    at {hi:<5} cells   {} bytes  (the largest board)", p.bytes(hi));
            }
        }
        match (&self.class, &self.verdict) {
            (_, Some((code, why))) => println!("    FAILED  {code}: {why}"),
            (Some(_), None) if self.cells.is_none() => println!(
                "    not judged: this release declares no limits.boards, so there is no board to \
                 price it at"
            ),
            (Some((f, c)), None) => {
                println!("    PASSED  under a class of {f} bytes flat and {c} a cell");
            }
            (None, None) => println!(
                "    not judged: name the season's class with --memory-flat-bytes and \
                 --memory-cell-bytes"
            ),
        }
        if let Some(r) = round_trip {
            println!(
                "    round trip       {} observations fed their predecessor's memory, {} failed",
                r.checked, r.failed
            );
            if let Some((i, e)) = &r.first {
                println!("    FAILED  MEMORY_ROUND_TRIP: observation {i}: {e}");
                println!(
                    "    the memory input does not take the model's own memory output: every \
                     match would strike from turn 1"
                );
            }
        }
    }
}

/// Read the head the way `kalam-match` reads it, and say why if it cannot.
fn head_reads(inf: &crate::model::Inference, obs: &Value) -> Result<(), String> {
    let (shape, values) = inf
        .f32_output("policy")
        .ok_or_else(|| "the manifest declares no f32 output named `policy`".to_string())?;
    let mine: Vec<(usize, usize)> = obs["mine"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|p| (p[0].as_u64().unwrap_or(0) as usize, p[1].as_u64().unwrap_or(0) as usize))
                .collect()
        })
        .unwrap_or_default();
    let cols = obs["size"][1].as_u64().unwrap_or(0) as usize;
    let acts = crate::model::decode(shape, &values, &mine, cols)?;
    if acts.len() != mine.len() {
        return Err(format!("the head decoded {} actions for {} ants", acts.len(), mine.len()));
    }
    Ok(())
}

/// Admission's probe as `check` measured it, against the game's turn.
fn report_probe(
    probe: &Result<(f64, std::collections::BTreeMap<String, usize>), String>,
    deadline: u64,
) {
    match probe {
        Ok((ms, dims)) => {
            let at = if dims.is_empty() {
                "its declared shapes".to_string()
            } else {
                dims.iter().map(|(n, v)| format!("{n} = {v}")).collect::<Vec<_>>().join(", ")
            };
            println!(
                "probe  ({} zero-filled inferences at {at}, as admission runs them, turn {deadline} ms)",
                crate::model::PROBE_RUNS
            );
            if *ms <= deadline as f64 {
                println!("    PASSED");
                println!(
                    "    median           {ms:.2} ms  (measured here; the admitting runner measures again)"
                );
            } else {
                println!("    FAILED  the median took {ms:.2} ms, over the {deadline} ms turn");
                println!(
                    "    admission refuses a model this slow on every attempt as PROBE_TOO_SLOW: make \
                     it faster at its probe_dims"
                );
            }
        }
        Err(e) => {
            println!("probe");
            println!("    FAILED  {e}");
            println!(
                "    admission refuses this as PROBE_FAILED: the graph does not run at its probe_dims"
            );
        }
    }
}

fn report_graph(stats: &crate::onnx::Stats, model: &Model, size_metric: usize) {
    println!("graph");
    println!("    opset            {}", stats.opset);
    println!("    parameters       {}", stats.parameters);
    println!("    nodes            {}", stats.nodes);
    println!(
        "    size metric      {size_metric} bytes  (artifact + manifest -- the platform classifies \
         it, this does not)"
    );
    println!("    operators        {}", stats.operators.join(", "));
    println!("    inputs           {}", model.input_names().join(" "));
}
