//! `tinybrains adapt` — run a manifest's input adapters and write out the tensors they produced.
//!
//! **This exists because an adapter is half of what plays, and until now nothing could show you
//! what it actually produced.** `check` reports shapes; a competitor training in Python encodes
//! observations twice — once in `adapter.json` for the ladder and once in numpy for the trainer —
//! observations twice — once in the manifest's adapters for the ladder and once in numpy for the
//! trainer — and two implementations of one encoding is the classic way to ship a model that
//! scores worse in the arena than it did in training. This is the tool that lets the two be
//! *diffed* rather than believed: dump the ladder's own answer, and assert your encoder equals it.
//!
//! `.npy` because that is the one array format every trainer already reads, and because writing it
//! needs no dependency: a header and the bytes the tensor already carries.
//!
//! It knows no game. The adapters are the manifest's, the observations are the cartridge's — and
//! they are evaluated by datalogic, which is the evaluator a node runs them on.

use std::path::PathBuf;

use serde_json::{Value, json};

use crate::cmd::{open_game, reference_observations};

pub fn run(args: &[String]) -> Result<(), String> {
    let mut obs_file: Option<PathBuf> = None;
    let mut out = PathBuf::from("tensors");
    let mut rest: Vec<String> = Vec::new();
    let mut slug: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--obs" => {
                i += 1;
                obs_file = Some(PathBuf::from(args.get(i).ok_or("--obs needs a file")?));
            }
            "--out" => {
                i += 1;
                out = PathBuf::from(args.get(i).ok_or("--out needs a directory")?);
            }
            "--game" => {
                i += 1;
                slug = Some(args.get(i).ok_or("--game needs a slug")?.clone());
            }
            other if !other.starts_with('-') => rest.push(other.to_string()),
            other => return Err(format!("unknown option '{other}'\n\n{}", crate::USAGE)),
        }
        i += 1;
    }
    if rest.len() != 2 {
        return Err(
            "which model?\n\n  tinybrains adapt model.onnx manifest.json [--obs FILE] [--out DIR]"
                .to_string(),
        );
    }

    let onnx = std::fs::read(&rest[0]).map_err(|e| format!("{}: {e}", rest[0]))?;
    let mbytes = std::fs::read(&rest[1]).map_err(|e| format!("{}: {e}", rest[1]))?;
    let manifest: Value =
        serde_json::from_slice(&mbytes).map_err(|e| format!("{}: not JSON: {e}", rest[1]))?;
    // The graph is loaded too, though nothing is run through it: a manifest whose declared shapes
    // the graph refuses is wrong in a way the tensors alone would not show.
    let model = crate::model::Model::load(&manifest, &onnx)?;

    let game = open_game(slug.as_deref())?;
    let (observations, source) = match &obs_file {
        Some(p) => (load_observations(p)?, p.display().to_string()),
        None => (reference_observations(&game)?, format!("{}'s reference set", game.slug)),
    };
    let budget = game.budget("adapter_ops_max", 1_000_000);

    println!(
        "{} against {} ({} observation{})",
        rest[0],
        source,
        observations.len(),
        if observations.len() == 1 { "" } else { "s" }
    );
    println!("    manifest         {}", crate::store::digest(&mbytes));
    println!("    inputs           {}", model.input_names().join(" "));
    println!();

    std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    let mut cases = Vec::new();
    for (i, obs) in observations.iter().enumerate() {
        let ports = model.adapt(obs, budget).map_err(|e| format!("observation {i}: {e}"))?;
        let ops = ports.iter().map(|(.., ops)| *ops).max().unwrap_or(0);

        let dir = out.join(format!("case-{i}"));
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let mut wrote = Vec::new();
        for (name, dtype, shape, bytes, _) in &ports {
            let path = dir.join(format!("{name}.npy"));
            std::fs::write(&path, npy(dtype, shape, bytes)?)
                .map_err(|e| format!("{}: {e}", path.display()))?;
            wrote.push(json!({
                "name": name,
                "dtype": dtype,
                "shape": shape,
                "file": path.display().to_string(),
            }));
        }
        // The observation beside its tensors: an encoder under test needs the input that produced
        // them, and a caller who passed --obs should not have to split the file again themselves.
        std::fs::write(
            dir.join("observation.json"),
            serde_json::to_vec(obs).map_err(|e| e.to_string())?,
        )
        .map_err(|e| format!("{}: {e}", dir.display()))?;

        println!(
            "    case {i:<3} {:>9} ops ({:>3}% of budget)  {}",
            ops,
            ops * 100 / budget.max(1),
            ports
                .iter()
                .map(|(n, dt, shape, ..)| format!("{n}{shape:?}:{dt}"))
                .collect::<Vec<_>>()
                .join(" ")
        );
        cases.push(json!({ "case": i, "ops_in": ops, "tensors": wrote }));
    }

    let index = json!({
        "artifact": rest[0],
        "manifest": rest[1],
        "manifest_hash": crate::store::digest(&mbytes),
        "game": game.slug,
        "engine_digest": game.engine_digest,
        "budget_ops": budget,
        "source": source,
        "cases": cases,
    });
    let mpath = out.join("index.json");
    std::fs::write(&mpath, serde_json::to_vec_pretty(&index).map_err(|e| e.to_string())?)
        .map_err(|e| format!("{}: {e}", mpath.display()))?;

    println!();
    println!(
        "wrote {} case{} to {}",
        cases.len(),
        if cases.len() == 1 { "" } else { "s" },
        out.display()
    );
    println!("these are the ladder's own tensors -- diff your trainer's encoder against them");
    Ok(())
}

/// NPY v1.0. The header is padded so the data starts 64-byte aligned, which is what numpy itself
/// writes and what `np.load` with `mmap_mode` expects.
fn npy(dtype: &str, shape: &[usize], data: &[u8]) -> Result<Vec<u8>, String> {
    // The datavalue dtype names, which are what a manifest writes.
    let descr = match dtype {
        "i8" => "|i1",
        "u8" => "|u1",
        "bool" => "|b1",
        "i16" => "<i2",
        "u16" => "<u2",
        "i32" => "<i4",
        "u32" => "<u4",
        "i64" => "<i8",
        "u64" => "<u8",
        "f32" => "<f4",
        "f64" => "<f8",
        other => return Err(format!("no numpy descriptor for dtype '{other}'")),
    };
    // A rank-1 shape needs the trailing comma or Python reads a parenthesised expression.
    let shape = match shape.len() {
        0 => "()".to_string(),
        1 => format!("({},)", shape[0]),
        _ => format!("({})", shape.iter().map(|d| d.to_string()).collect::<Vec<_>>().join(", ")),
    };
    let head = format!("{{'descr': '{descr}', 'fortran_order': False, 'shape': {shape}, }}");
    let mut pad = 64 - ((10 + head.len() + 1) % 64);
    if pad == 64 {
        pad = 0;
    }
    let header = format!("{head}{}\n", " ".repeat(pad));

    let mut out = Vec::with_capacity(10 + header.len() + data.len());
    out.extend_from_slice(b"\x93NUMPY\x01\x00");
    out.extend_from_slice(&(header.len() as u16).to_le_bytes());
    out.extend_from_slice(header.as_bytes());
    out.extend_from_slice(data);
    Ok(out)
}

/// One observation, a bare array of them, or the cartridge's `{"observations": [...]}` envelope.
fn load_observations(path: &std::path::Path) -> Result<Vec<Value>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let doc: Value = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(match doc {
        Value::Array(a) => a,
        Value::Object(ref o) if o.contains_key("observations") => {
            doc["observations"].as_array().cloned().unwrap_or_default()
        }
        other => vec![other],
    })
}
