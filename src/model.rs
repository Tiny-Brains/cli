//! A submitted model, run here the way a node runs it.
//!
//! It is the same two libraries a node uses and in the same order — **datalogic** evaluates the manifest's adapters, **tract**
//! runs the ONNX graph — so `tinybrains check` answers the question admission will answer, rather
//! than a local approximation of it. The evaluator is not merely the same library: it is built the
//! way a node builds it, a dataflow-rs engine whose datalogic engine is borrowed (see
//! [`evaluator`]), so templating, the `$` key escape and the operator families are dataflow-rs's
//! own settings rather than a copy of them.
//!
//! ONE GAP IS LEFT, AND IT REFUSES RATHER THAN DIVERGES. A node also registers Orion's own
//! operators ([`ORION_OPERATORS`]) and screens three keys out of every adapter
//! ([`FORBIDDEN_OPERATORS`]). Neither lives anywhere this binary can link yet, and in templating
//! mode an operator this engine lacks is not an error but data -- so an adapter calling `join`
//! would build one tensor here and another on the ladder. [`screen`] refuses those keys instead,
//! until the operators and the screen are shared with Orion rather than listed here.
//!
//! What this deliberately is NOT: a second dialect, a second budget accountant, or a second
//! definition of what a manifest may contain. The manifest is Orion's `orion:model@1.0.0`, the
//! budget is datalogic's `evaluate_metered`, and the shapes are checked the way
//! `orion-server`'s model handler checks them — declared dtype and shape per input, a named
//! dimension bound on first sight and equal at every later one.
//!
//! The one thing here that IS the platform's rather than Orion's is [`decode`]: the head is read
//! by the platform, because a `result` expression's root is the output tensors alone
//! and so cannot reach the observation the gather needs. Kalam does it in JSONLogic and this does
//! it in Rust; `tinybrains conform` is what keeps the two honest.

use std::collections::{BTreeMap, BTreeSet};

use dataflow_rs::datalogic_rs::{self, DataValue, Engine, Logic};
use serde_json::{Value, json};
use tract_onnx::prelude::*;

/// The evaluator this binary links, printed by `tinybrains games` and written into every replay it
/// produces. An adapter is priced by datalogic on a node too, so a skew here is a skew in what a
/// local `check` promises.
pub const DATALOGIC_VERSION: &str = "5.5";

/// Orion's own operators, as orion-server 1.9.0 registers them on every engine a node evaluates an
/// adapter on (`engine/operators.rs`, `all()`). This build does not have them, and templating mode
/// reads an unknown operator as data, so an adapter naming one is refused rather than evaluated
/// differently. INTERIM: the list goes when the operators are shared with Orion, and until then it
/// changes with an Orion upgrade, like `Cargo.toml`'s `dataflow-rs`.
const ORION_OPERATORS: [&str; 10] = [
    "base64_encode",
    "base64_decode",
    "base64url_encode",
    "base64url_decode",
    "hex_encode",
    "hex_decode",
    "random",
    "url_encode",
    "url_decode",
    "join",
];

/// How many inferences admission's probe runs, and its verdict is their median: orion-server's
/// `model/admission.rs`, `PROBE_RUNS`.
pub const PROBE_RUNS: usize = 5;

/// The keys orion-server 1.9.0 refuses in any adapter at upload, with its reasons
/// (`model/manifest.rs`, `FORBIDDEN_OPERATORS`). Checked before [`ORION_OPERATORS`], so `random`
/// is refused for the reason a node gives.
const FORBIDDEN_OPERATORS: [(&str, &str); 3] = [
    (
        "secret",
        "an adapter may not read the secret store: a manifest is authored by the model's \
         owner, and the secrets are the deployment's",
    ),
    (
        "now",
        "an adapter may not read the clock: a replay of a traced inference must reproduce the \
         same tensors",
    ),
    (
        "random",
        "an adapter may not draw randomness: a replay of a traced inference must reproduce the \
         same tensors",
    ),
];

/// The evaluator a node evaluates an adapter on, built the way the node builds it: a dataflow-rs
/// engine with no workflows, and the datalogic engine it holds. orion-server does the same
/// (`model/handler.rs` evaluates on `generation.engine.datalogic()`), so templating, the `$` key
/// escape, the operator families and the `secret` operator are the node's by construction for as
/// long as `Cargo.toml` pins the dataflow-rs orion-server links. Orion's own operators are what
/// this lacks; [`screen`] covers them.
fn evaluator() -> Result<(std::sync::Arc<Engine>, BTreeSet<String>), String> {
    let engine = dataflow_rs::Engine::builder()
        .build()
        .map_err(|e| format!("the adapter evaluator did not build: {e}"))?;
    let vocabulary = engine.operator_names().map(String::from).collect();
    Ok((std::sync::Arc::clone(engine.datalogic()), vocabulary))
}

/// Walk an adapter the way a node's screen does, structurally: every single-key object, wherever
/// it sits. `Err` for a key a node refuses or evaluates with an operator this build lacks; a
/// warning for a key that is no operator at all, which a node reads as data -- a misspelt
/// `{"scattr": …}` is a literal object, and `{"if": [{"=": …}, …]}` takes the THEN branch, because
/// an object is truthy. Nothing refuses that on the ladder, so the warning is the only place a
/// competitor hears of it. A `$`-escaped key is data by intent and never warned about.
fn screen(
    logic: &Value,
    path: &str,
    vocabulary: &BTreeSet<String>,
    warnings: &mut Vec<String>,
) -> Result<(), String> {
    match logic {
        Value::Object(map) => {
            if map.len() == 1 {
                let key = map.keys().next().expect("one member");
                if let Some((_, reason)) = FORBIDDEN_OPERATORS.iter().find(|(op, _)| op == key) {
                    return Err(format!("{path} uses {{\"{key}\": …}}: {reason}"));
                }
                if ORION_OPERATORS.contains(&key.as_str()) {
                    return Err(format!(
                        "{path} uses {{\"{key}\": …}}, one of Orion's own operators: a node \
                         evaluates it and this build of tinybrains cannot, so it refuses the \
                         adapter rather than read the call as data"
                    ));
                }
                if !key.starts_with('$') && !vocabulary.contains(key) {
                    warnings.push(format!(
                        "{path}: {{\"{key}\": …}} names no operator, so a node reads it as data \
                         rather than calling anything. If it is a misspelt operator, fix it; if \
                         the object is data, write {{\"${key}\": …}} to say so"
                    ));
                }
            }
            for (key, value) in map {
                screen(value, &format!("{path}.{key}"), vocabulary, warnings)?;
            }
        }
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                screen(item, &format!("{path}[{i}]"), vocabulary, warnings)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// One dimension of a declared shape: a count, or a name bound per call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Dim {
    Fixed(usize),
    Named(String),
}

impl Dim {
    fn parse(v: &Value) -> Result<Dim, String> {
        match v {
            Value::Number(n) => n
                .as_u64()
                .filter(|n| *n > 0)
                .map(|n| Dim::Fixed(n as usize))
                .ok_or_else(|| format!("a fixed dimension must be a positive integer, got {n}")),
            Value::String(s) if is_dim_name(s) => Ok(Dim::Named(s.clone())),
            other => Err(format!(
                "a dimension is a positive integer or a name matching [A-Za-z_][A-Za-z0-9_]*, got {other}"
            )),
        }
    }

    fn render(&self) -> String {
        match self {
            Dim::Fixed(n) => n.to_string(),
            Dim::Named(s) => s.clone(),
        }
    }
}

fn is_dim_name(s: &str) -> bool {
    let mut cs = s.chars();
    matches!(cs.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && cs.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// What a call has bound each named dimension to. One of these lives for one inference: every
/// input is checked through it in order, then every output, so a name the inputs bound is what the
/// outputs are held to — exactly the rule `orion-server`'s `Bindings` applies.
#[derive(Default)]
pub struct Bindings(BTreeMap<String, usize>);

impl Bindings {
    pub fn check(&mut self, declared: &[Dim], actual: &[usize], what: &str) -> Result<(), String> {
        if declared.len() != actual.len() {
            return Err(format!(
                "{what}: expected {} dimension(s), got {}",
                declared.len(),
                actual.len()
            ));
        }
        for (axis, (dim, actual)) in declared.iter().zip(actual).enumerate() {
            match dim {
                Dim::Fixed(n) if n == actual => {}
                Dim::Fixed(n) => {
                    return Err(format!("{what}: axis {axis} must be {n}, got {actual}"));
                }
                Dim::Named(name) => match self.0.get(name) {
                    Some(bound) if bound != actual => {
                        return Err(format!(
                            "{what}: axis {axis} is '{name}', already {bound} in this call, got {actual}"
                        ));
                    }
                    Some(_) => {}
                    None => {
                        self.0.insert(name.clone(), *actual);
                    }
                },
            }
        }
        Ok(())
    }
}

struct Decl {
    name: String,
    dtype: String,
    shape: Vec<Dim>,
}

fn decls(v: &Value, what: &str) -> Result<Vec<Decl>, String> {
    let list = v.as_array().ok_or_else(|| format!("manifest.{what} must be an array"))?;
    if list.is_empty() {
        return Err(format!("manifest.{what} declares nothing"));
    }
    list.iter()
        .map(|d| {
            let name =
                d["name"].as_str().ok_or_else(|| format!("{what}: a declaration needs a name"))?;
            let dtype = d["dtype"]
                .as_str()
                .ok_or_else(|| format!("{what} '{name}': a declaration needs a dtype"))?;
            let shape = d["shape"]
                .as_array()
                .ok_or_else(|| format!("{what} '{name}': a declaration needs a shape"))?
                .iter()
                .map(Dim::parse)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("{what} '{name}': {e}"))?;
            Ok(Decl { name: name.to_string(), dtype: dtype.to_string(), shape })
        })
        .collect()
}

/// One input adapter's answer: its name, dtype, shape, bytes, and what evaluating it charged.
pub type AdaptedInput = (String, String, Vec<usize>, Vec<u8>, u64);

/// What one inference cost and what it produced.
///
/// `ops` and `peak_ops` are the two numbers `stats_output` reports on a node and they answer
/// different questions: `ops` is what the message cost, `peak_ops` is what a per-evaluation
/// ceiling has to clear.
#[allow(dead_code)]
pub struct Inference {
    /// Output tensors in manifest order, as `(name, dtype, shape, bytes)`.
    pub outputs: Vec<(String, String, Vec<usize>, Vec<u8>)>,
    /// Operations the input adapters charged, summed — the number
    /// `stats_output.ops` reports on a node.
    pub ops: u64,
    /// The heaviest single adapter, which is what a per-evaluation ceiling has to clear.
    pub peak_ops: u64,
    pub infer_us: u64,
}

impl Inference {
    /// The output named `name`, as a plain f32 vector with its shape.
    pub fn f32_output(&self, name: &str) -> Option<(&[usize], Vec<f32>)> {
        let (_, dtype, shape, bytes) = self.outputs.iter().find(|(n, ..)| n == name)?;
        if dtype != "f32" {
            return None;
        }
        let vals = bytes.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)).collect();
        Some((shape.as_slice(), vals))
    }
}

/// A manifest and its graph, loaded and ready to answer.
///
/// `name`, `digest` and `artifact_bytes` are the identity a node records on a row, kept here so a
/// caller that reports them does not have to re-read the file to find them.
#[allow(dead_code)]
pub struct Model {
    pub name: String,
    pub digest: String,
    pub artifact_bytes: usize,
    inputs: Vec<Decl>,
    outputs: Vec<Decl>,
    /// What [`screen`] found that a node accepts but most likely does not mean: each a sentence
    /// naming the input and the key. Printed by the caller, to stderr.
    pub warnings: Vec<String>,
    /// The manifest's `probe_dims`: what admission's probe binds each named dimension to.
    probe_dims: BTreeMap<String, usize>,
    engine: std::sync::Arc<Engine>,
    adapters: Vec<Logic>,
    plan: std::sync::Arc<TypedSimplePlan>,
    /// The graph index each manifest input feeds, in manifest order.
    in_order: Vec<usize>,
    /// Where each manifest output sits in the plan's output list.
    out_order: Vec<usize>,
}

impl Model {
    /// Load a manifest and its artifact. The graph is bound to the manifest's declared dtypes and
    /// shapes, a named dimension going in as its name — which is the spec string tract's own
    /// parser takes, and what lets one session serve every board size.
    pub fn load(manifest: &Value, onnx: &[u8]) -> Result<Model, String> {
        let abi = manifest["abi"].as_str().unwrap_or_default();
        if !abi.starts_with("orion:model@") {
            return Err(format!("manifest abi must be orion:model@<version>, got '{abi}'"));
        }
        if !manifest["result"].is_null() {
            return Err("a manifest may not carry a `result` expression: the platform reads the \
                        head, so a result would be ignored at play and is refused here"
                .to_string());
        }
        let name = manifest["name"].as_str().unwrap_or("model").to_string();
        let inputs = decls(&manifest["inputs"], "inputs")?;
        let outputs = decls(&manifest["outputs"], "outputs")?;
        let probe_dims = match &manifest["probe_dims"] {
            Value::Null => BTreeMap::new(),
            Value::Object(m) => m
                .iter()
                .map(|(name, v)| {
                    v.as_u64().filter(|n| *n > 0).map(|n| (name.clone(), n as usize)).ok_or_else(
                        || format!("probe_dims.{name} must be a positive integer, got {v}"),
                    )
                })
                .collect::<Result<_, _>>()?,
            other => return Err(format!("manifest.probe_dims must be an object, got {other}")),
        };

        // The adapters, compiled on one engine — the same engine every evaluation runs on, because
        // a compiled program belongs to the engine that compiled it — after the screen.
        let (engine, vocabulary) = evaluator()?;
        let mut adapters = Vec::with_capacity(inputs.len());
        let mut warnings = Vec::new();
        for (i, decl) in inputs.iter().enumerate() {
            let logic = manifest["inputs"][i]["adapter"].clone();
            let logic = if logic.is_null() {
                // The default adapter: the input's own name, read off the message.
                json!({ "var": decl.name })
            } else {
                logic
            };
            screen(&logic, &format!("input '{}': adapter", decl.name), &vocabulary, &mut warnings)?;
            adapters.push(engine.compile(&logic).map_err(|e| {
                format!("input '{}': the adapter does not compile: {e}", decl.name)
            })?);
        }

        let mut model = tract_onnx::onnx()
            .model_for_read(&mut std::io::Cursor::new(onnx))
            .map_err(|e| format!("tract could not read the graph: {e}"))?;
        // THE MODEL'S OWN SCOPE, not a fresh one. A symbol belongs to the scope that minted it,
        // and a fact built in another scope panics inside tract's dimension prover the moment the
        // two meet -- `ProofCacheSession scope_id mismatch`, which says nothing about manifests.
        let symbols = model.symbols.clone();
        let graph_inputs = graph_input_names(onnx)?;
        let graph_outputs = graph_output_names(onnx)?;

        let mut in_order = Vec::with_capacity(inputs.len());
        for decl in &inputs {
            let index = graph_inputs
                .iter()
                .position(|n| *n == decl.name)
                .ok_or_else(|| format!("the graph has no input named '{}'", decl.name))?;
            let fact = input_fact(&symbols, decl)?;
            model.set_input_fact(index, fact).map_err(|e| format!("input '{}': {e}", decl.name))?;
            in_order.push(index);
        }
        let mut out_order = Vec::with_capacity(outputs.len());
        for decl in &outputs {
            out_order.push(
                graph_outputs
                    .iter()
                    .position(|n| *n == decl.name)
                    .ok_or_else(|| format!("the graph has no output named '{}'", decl.name))?,
            );
        }

        model.analyse(true).map_err(|e| {
            format!("the graph does not type-check with the manifest's inputs: {e}")
        })?;
        let plan = model
            .into_typed()
            .and_then(|m| m.into_optimized())
            .and_then(|m| m.into_runnable())
            .map_err(|e| format!("the graph could not be prepared: {e}"))?;

        Ok(Model {
            name,
            digest: crate::store::digest(onnx),
            artifact_bytes: onnx.len(),
            inputs,
            outputs,
            warnings,
            probe_dims,
            engine,
            adapters,
            plan,
            in_order,
            out_order,
        })
    }

    /// One inference over one observation: every input adapter evaluated under `budget`, the graph
    /// run, the outputs checked against the manifest through the call's own bindings.
    pub fn infer(&self, observation: &Value, budget: u64) -> Result<Inference, String> {
        let arena = datalogic_rs::bumpalo::Bump::new();
        let mut bindings = Bindings::default();
        let mut tensors: Vec<TValue> = vec![TValue::from(Tensor::default()); self.in_order.len()];
        let mut ops = 0u64;
        let mut peak = 0u64;

        for (i, decl) in self.inputs.iter().enumerate() {
            let t = crate::timing::start();
            let metered = self
                .engine
                .evaluate_metered(&self.adapters[i], observation, &arena, budget)
                .map_err(|e| format!("adapter for input '{}' failed: {e}", decl.name))?;
            crate::timing::stop(crate::timing::P::Adapter, t);
            ops += metered.ops;
            peak = peak.max(metered.ops);
            let t = as_tensor(metered.value)
                .ok_or_else(|| format!("adapter for input '{}' produced no tensor", decl.name))?;
            let dtype = t.dtype().name();
            if dtype != decl.dtype {
                return Err(format!(
                    "input '{}' is {dtype}, the manifest declares {}",
                    decl.name, decl.dtype
                ));
            }
            bindings.check(&decl.shape, t.shape(), &format!("input '{}'", decl.name))?;
            tensors[self.in_order[i]] = to_tract(t)?.into();
        }

        let started = std::time::Instant::now();
        let inputs = tensors.into_iter().collect::<TVec<_>>();
        let out = if crate::timing::profiling_nodes() {
            self.run_profiled(inputs)?
        } else {
            self.plan.run(inputs).map_err(|e| format!("the graph failed to run: {e}"))?
        };
        let infer_us = started.elapsed().as_micros() as u64;
        crate::timing::stop(crate::timing::P::Graph, started);

        let mut outputs = Vec::with_capacity(self.outputs.len());
        for (i, decl) in self.outputs.iter().enumerate() {
            let t = &out[self.out_order[i]];
            let shape = t.shape().to_vec();
            let dtype = dtype_name(t.datum_type()).ok_or_else(|| {
                format!("output '{}' has a dtype this build cannot carry", decl.name)
            })?;
            if dtype != decl.dtype {
                return Err(format!(
                    "output '{}' is {dtype}, the manifest declares {}",
                    decl.name, decl.dtype
                ));
            }
            bindings.check(&decl.shape, &shape, &format!("output '{}'", decl.name))?;
            outputs.push((decl.name.clone(), dtype.to_string(), shape, t.as_bytes().to_vec()));
        }
        Ok(Inference { outputs, ops, peak_ops: peak, infer_us })
    }

    /// The same run, one node at a time, so a slow graph can say which of its nodes is slow.
    /// `TINYBRAINS_PROFILE_NODES` only: this is the state-machine path rather than `plan.run`, and the
    /// timer per node makes the total a little larger than the run it is explaining.
    fn run_profiled(&self, inputs: TVec<TValue>) -> Result<TVec<TValue>, String> {
        let mut state = tract_onnx::tract_core::plan::SimpleState::new(&self.plan)
            .map_err(|e| format!("the graph could not be stepped: {e}"))?;
        state
            .run_plan_with_eval(inputs, |ctx, op_state, node, input| {
                let t = std::time::Instant::now();
                let r = tract_onnx::tract_core::plan::eval(ctx, op_state, node, input);
                crate::timing::node(&node.op().name(), t.elapsed().as_nanos() as u64);
                r
            })
            .map_err(|e| format!("the graph failed to run: {e}"))
    }

    /// Admission's probe, run the way a node runs it (orion-server's `model/admission.rs`,
    /// `probe_runs`): [`PROBE_RUNS`] inferences over zero-filled inputs, a named dimension at the
    /// manifest's `probe_dims` or 1 when it names none, each run timed alone and no warm-up, since a
    /// node takes none. The median in milliseconds, and what each name was bound to.
    pub fn probe(&self) -> Result<(f64, BTreeMap<String, usize>), String> {
        let mut bound = BTreeMap::new();
        let mut tensors: Vec<TValue> = vec![TValue::from(Tensor::default()); self.in_order.len()];
        for (i, decl) in self.inputs.iter().enumerate() {
            let dt = datum_type(&decl.dtype).ok_or_else(|| {
                format!("input '{}': dtype '{}' is not one tract takes", decl.name, decl.dtype)
            })?;
            let shape: Vec<usize> = decl
                .shape
                .iter()
                .map(|d| match d {
                    Dim::Fixed(n) => *n,
                    Dim::Named(name) => {
                        let n = self.probe_dims.get(name).copied().unwrap_or(1);
                        bound.insert(name.clone(), n);
                        n
                    }
                })
                .collect();
            let zeros =
                Tensor::zero_dt(dt, &shape).map_err(|e| format!("input '{}': {e}", decl.name))?;
            tensors[self.in_order[i]] = zeros.into();
        }
        let inputs = tensors.into_iter().collect::<TVec<_>>();
        let mut times = Vec::with_capacity(PROBE_RUNS);
        for _ in 0..PROBE_RUNS {
            let started = std::time::Instant::now();
            self.plan
                .run(inputs.clone())
                .map_err(|e| format!("the probe inference failed: {e}"))?;
            times.push(started.elapsed().as_secs_f64() * 1000.0);
        }
        times.sort_by(f64::total_cmp);
        Ok((times[PROBE_RUNS / 2], bound))
    }

    /// The inputs a manifest declares, for `tinybrains adapt`.
    pub fn input_names(&self) -> Vec<&str> {
        self.inputs.iter().map(|d| d.name.as_str()).collect()
    }

    /// The tensor one adapter produces, without running the graph.
    pub fn adapt(&self, observation: &Value, budget: u64) -> Result<Vec<AdaptedInput>, String> {
        let arena = datalogic_rs::bumpalo::Bump::new();
        let mut out = Vec::new();
        for (i, decl) in self.inputs.iter().enumerate() {
            let metered = self
                .engine
                .evaluate_metered(&self.adapters[i], observation, &arena, budget)
                .map_err(|e| format!("adapter for input '{}' failed: {e}", decl.name))?;
            let t = as_tensor(metered.value)
                .ok_or_else(|| format!("adapter for input '{}' produced no tensor", decl.name))?;
            out.push((
                decl.name.clone(),
                t.dtype().name().to_string(),
                t.shape().to_vec(),
                t.data().to_vec(),
                metered.ops,
            ));
        }
        Ok(out)
    }
}

/// THE PLATFORM READS THE HEAD, and this is that rule in Rust. A manifest's `result` expression
/// sees only the output tensors, never the observation the gather needs.
///
///   `[1, 5, H, W]`  per-cell: gather the ants' flat indices out of the channel-major plane and
///                   take the argmax over the five channels.
///   `[n, 5]`        per-ant: already in `mine` order, because the adapter fed the coordinates in
///                   that order. Argmax and nothing else.
///
/// `mine` is the observation's own list and the action array is positionally aligned with it, so
/// an empty list is an empty action array — which is always valid.
pub fn decode(
    shape: &[usize],
    values: &[f32],
    mine: &[(usize, usize)],
    cols: usize,
) -> Result<Vec<String>, String> {
    const DIRS: [&str; 5] = ["N", "E", "S", "W", "-"];
    let pick = |row: &[f32]| -> &'static str {
        let mut best = 0usize;
        for (i, v) in row.iter().enumerate() {
            if *v > row[best] {
                best = i;
            }
            let _ = i;
        }
        DIRS[best.min(DIRS.len() - 1)]
    };
    match shape.len() {
        4 => {
            let (c, h, w) = (shape[1], shape[2], shape[3]);
            if c != DIRS.len() {
                return Err(format!("a per-cell head must have {} channels, got {c}", DIRS.len()));
            }
            if w != cols {
                return Err(format!("the head is {w} columns wide and the board is {cols}"));
            }
            let cells = h * w;
            Ok(mine
                .iter()
                .map(|(r, col)| {
                    let flat = r * w + col;
                    let row: Vec<f32> = (0..c).map(|ch| values[ch * cells + flat]).collect();
                    pick(&row).to_string()
                })
                .collect())
        }
        2 => {
            let (n, c) = (shape[0], shape[1]);
            if c != DIRS.len() {
                return Err(format!("a per-ant head must have {} columns, got {c}", DIRS.len()));
            }
            if n != mine.len() {
                return Err(format!(
                    "the head has {n} rows and the observation has {} ants",
                    mine.len()
                ));
            }
            Ok((0..n).map(|i| pick(&values[i * c..(i + 1) * c]).to_string()).collect())
        }
        other => Err(format!(
            "a head is [1, 5, H, W] per cell or [n, 5] per ant; this one has {other} dimensions"
        )),
    }
}

// ---------------------------------------------------------------------------- plumbing

/// A declared input as the fact tract type-checks the graph against: its dtype, and its shape with
/// each named dimension a symbol of the model's own scope.
///
/// The same fact `tract_libcli::tensor::parse_spec` built from `"<dim>,...,<dtype>"`, whose dtype
/// table this copies -- without the crate, which is most of tract's command line. A dtype outside
/// the table is refused by name, where the spec parser would have read it as one more dimension.
/// A manifest's dtype name as tract's type: the one table the graph's inputs, the probe's zeros and
/// an adapter's tensors are all converted through.
fn datum_type(name: &str) -> Option<DatumType> {
    Some(match name.to_ascii_lowercase().as_str() {
        "bool" => DatumType::Bool,
        "f16" => DatumType::F16,
        "f32" => DatumType::F32,
        "f64" => DatumType::F64,
        "i8" => DatumType::I8,
        "i16" => DatumType::I16,
        "i32" => DatumType::I32,
        "i64" => DatumType::I64,
        "u8" => DatumType::U8,
        "u16" => DatumType::U16,
        "u32" => DatumType::U32,
        "u64" => DatumType::U64,
        _ => return None,
    })
}

fn input_fact(symbols: &SymbolScope, decl: &Decl) -> Result<InferenceFact, String> {
    let dt = datum_type(&decl.dtype).ok_or_else(|| {
        format!("input '{}': dtype '{}' is not one tract takes", decl.name, decl.dtype)
    })?;
    let shape = decl
        .shape
        .iter()
        .map(|d| symbols.parse_tdim(d.render()))
        .collect::<TractResult<Vec<TDim>>>()
        .map_err(|e| format!("input '{}': a dimension tract does not take: {e}", decl.name))?;
    Ok(InferenceFact::dt_shape(dt, shape))
}

fn as_tensor<'a>(v: &'a DataValue<'a>) -> Option<&'a datalogic_rs::datavalue::DataTensor<'a>> {
    match v {
        DataValue::Tensor(t) => Some(t),
        _ => None,
    }
}

fn to_tract(t: &datalogic_rs::datavalue::DataTensor<'_>) -> Result<Tensor, String> {
    let shape = t.shape();
    let bytes = t.data();
    let name = t.dtype().name();
    let dt = datum_type(name)
        .filter(|dt| *dt != DatumType::F16)
        .ok_or_else(|| format!("dtype '{name}' is not one a graph takes here"))?;
    unsafe { Tensor::from_raw_dt(dt, shape, bytes) }.map_err(|e| format!("tensor: {e}"))
}

fn dtype_name(dt: DatumType) -> Option<&'static str> {
    Some(match dt {
        DatumType::F32 => "f32",
        DatumType::F64 => "f64",
        DatumType::I8 => "i8",
        DatumType::U8 => "u8",
        DatumType::I16 => "i16",
        DatumType::U16 => "u16",
        DatumType::I32 => "i32",
        DatumType::U32 => "u32",
        DatumType::I64 => "i64",
        DatumType::U64 => "u64",
        DatumType::Bool => "bool",
        _ => return None,
    })
}

/// The graph's own input and output NAMES, read from the protobuf rather than from tract.
///
/// tract names an output after the node that produces it, which an exporter names freely
/// (`/fc2/Gemm`), while `graph.output` carries the tensor name a manifest speaks. Orion reads them
/// the same way and for the same reason.
fn graph_input_names(onnx: &[u8]) -> Result<Vec<String>, String> {
    crate::onnx::io_names(onnx).map(|(i, _)| i)
}

fn graph_output_names(onnx: &[u8]) -> Result<Vec<String>, String> {
    crate::onnx::io_names(onnx).map(|(_, o)| o)
}
