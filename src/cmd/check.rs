//! What admission will do, done here first: read the graph, run the manifest's adapters over the
//! game's reference observations under the season's budget, and check the head is one the platform
//! can read.
//!
//! Necessary and not sufficient. There is no download allowlist here, the size class is *reported*
//! rather than decided because that table is the season's, and a node re-hashes the object it
//! fetches — so a pass here says the submission is well formed, not that it was admitted.

use serde_json::{Value, json};

use crate::cmd::{game_and_rest, open_game, reference_observations};
use crate::model::Model;
use crate::store;

pub fn run(args: &[String]) -> Result<(), String> {
    // `--json` before the shared parser sees it: `game_and_rest` refuses an unknown option, which
    // is the behaviour every other command wants.
    let json_out = args.iter().any(|a| a == "--json");
    let args: Vec<String> = args.iter().filter(|a| *a != "--json").cloned().collect();
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
    let model = Model::load(&manifest, &wb)?;
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

    let mut ops_max = 0u64;
    let mut infer_us_max = 0u64;
    let mut failure: Option<(usize, String, bool)> = None;
    for (i, obs) in observations.iter().enumerate() {
        match model.infer(obs, budget) {
            Ok(inf) => {
                ops_max = ops_max.max(inf.peak_ops);
                infer_us_max = infer_us_max.max(inf.infer_us);
                // The head has to be one the platform can gather from, so `check` reads it exactly
                // as `kalam-match` does rather than merely noting that something came back.
                if let Err(e) = head_reads(&inf, obs) {
                    failure = Some((i, e, false));
                    break;
                }
            }
            Err(e) => {
                let over = e.contains("budget") || e.contains("Budget");
                failure = Some((i, e, over));
                break;
            }
        }
    }
    // ADMISSION'S ONE TIMING GATE, run as a node runs it: PROBE_RUNS inferences over zero-filled
    // inputs at the manifest's probe_dims, their median inside the game's turn. The admitting
    // runner's models.max_probe_ms IS that turn_ms (web's configs.sh checks it), so this compares
    // against the same number with no copy of it. Measured on this machine: the runner measures
    // again, and a model slow there on every attempt expires PROBE_TOO_SLOW.
    let probe = model.probe();
    let probe_ok = matches!(&probe, Ok((ms, _)) if *ms <= deadline as f64);
    let ok = failure.is_none() && probe_ok;

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
        report_probe(&probe, deadline);
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
