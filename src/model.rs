//! A submitted model, run here the way a node runs it.
//!
//! This is the half of the CLI that used to be `axon` as a library. It is the same two libraries a
//! node uses and in the same order — **datalogic** evaluates the manifest's adapters, **tract**
//! runs the ONNX graph — so `tinybrains check` answers the question admission will answer, rather
//! than a local approximation of it.
//!
//! What this deliberately is NOT: a second dialect, a second budget accountant, or a second
//! definition of what a manifest may contain. The manifest is Orion's `orion:model@1.0.0`, the
//! budget is datalogic's `evaluate_metered`, and the shapes are checked the way
//! `orion-server`'s model handler checks them — declared dtype and shape per input, a named
//! dimension bound on first sight and equal at every later one.
//!
//! The one thing here that IS the platform's rather than Orion's is [`decode`]: the head is read
//! by the platform (decision R3), because a `result` expression's root is the output tensors alone
//! and so cannot reach the observation the gather needs. Kalam does it in JSONLogic and this does
//! it in Rust; `tinybrains conform` is what keeps the two honest.

use std::collections::BTreeMap;

use datalogic_rs::{DataValue, Engine, Logic};
use serde_json::{Value, json};
use tract_onnx::prelude::*;

/// The evaluator this binary links, printed by `tinybrains games` and written into every replay it
/// produces. An adapter is priced by datalogic on a node too, so a skew here is a skew in what a
/// local `check` promises.
pub const DATALOGIC_VERSION: &str = "5.5";

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
    engine: Engine,
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
                        head (decision R3), so a result would be ignored at play and is refused here"
                .to_string());
        }
        let name = manifest["name"].as_str().unwrap_or("model").to_string();
        let inputs = decls(&manifest["inputs"], "inputs")?;
        let outputs = decls(&manifest["outputs"], "outputs")?;

        // The adapters, compiled on one engine — the same engine every evaluation runs on, because
        // a compiled program belongs to the engine that compiled it.
        let engine = Engine::new();
        let mut adapters = Vec::with_capacity(inputs.len());
        for (i, decl) in inputs.iter().enumerate() {
            let logic = manifest["inputs"][i]["adapter"].clone();
            let logic = if logic.is_null() {
                // The default adapter: the input's own name, read off the message.
                json!({ "var": decl.name })
            } else {
                logic
            };
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
            let metered = self
                .engine
                .evaluate_metered(&self.adapters[i], observation, &arena, budget)
                .map_err(|e| format!("adapter for input '{}' failed: {e}", decl.name))?;
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
        let out = self
            .plan
            .run(tensors.into_iter().collect::<TVec<_>>())
            .map_err(|e| format!("the graph failed to run: {e}"))?;
        let infer_us = started.elapsed().as_micros() as u64;

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

/// THE PLATFORM READS THE HEAD (decision R3), and this is that rule in Rust.
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
fn input_fact(symbols: &SymbolScope, decl: &Decl) -> Result<InferenceFact, String> {
    let dt = match decl.dtype.to_ascii_lowercase().as_str() {
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
        other => {
            return Err(format!("input '{}': dtype '{other}' is not one tract takes", decl.name));
        }
    };
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
    let dt = match t.dtype().name() {
        "f32" => DatumType::F32,
        "f64" => DatumType::F64,
        "i8" => DatumType::I8,
        "u8" => DatumType::U8,
        "i16" => DatumType::I16,
        "u16" => DatumType::U16,
        "i32" => DatumType::I32,
        "u32" => DatumType::U32,
        "i64" => DatumType::I64,
        "u64" => DatumType::U64,
        "bool" => DatumType::Bool,
        other => return Err(format!("dtype '{other}' is not one a graph takes here")),
    };
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
