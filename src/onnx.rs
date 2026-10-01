//! What a graph says about itself, read from the protobuf and not from a runtime.
//!
//! Two reasons this exists rather than asking tract. The names: tract names an output after the
//! node that produces it, which an exporter names freely (`/fc2/Gemm`), while `graph.output`
//! carries the tensor name a manifest speaks — so a manifest checked against tract's names would
//! be checked against the wrong strings. And the counts: `parameters` is a fairness number, so it
//! must be what the *document* carries rather than what a runtime happens to keep after
//! decluttering.
//!
//! The counting rule is Orion 1.8.1's, copied deliberately (`model/onnx.rs`): **every value the
//! document carries, wherever it carries it** — initializers, the tensors and repeated scalar
//! lists a node holds in its attributes, and the bodies of `If`, `Loop` and `Scan`. A graph that
//! moves its weights into `Constant` nodes, or an `ai.onnx.ml` model whose whole forest travels in
//! attributes, counts the same as the honest exporter's. Counting initializers alone would let a
//! graph hide its weights.

use std::collections::BTreeSet;

use prost::Message;
use tract_onnx::pb::{
    AttributeProto, GraphProto, ModelProto, NodeProto, attribute_proto::AttributeType,
};

pub struct Stats {
    pub parameters: u64,
    pub nodes: u64,
    /// Distinct operators, sorted, each qualified by its domain unless that is the default one.
    pub operators: Vec<String>,
    pub ir_version: i64,
    pub opset: i64,
}

/// The ONNX surface admission allows: the deployment's `op_allowlist`, `opset_min` and
/// `opset_max` (soma's `docker/soma.toml.tmpl`), which the book publishes on its *Format* page
/// and web's `scripts/check/configs.sh` holds equal to this copy. An operator off the list is
/// `OP_NOT_ALLOWED` at admission, whatever this binary's runtime can execute -- `GreaterOrEqual`
/// runs here and is refused there -- so `check` refuses it first. INTERIM like Orion's operator
/// list in `model.rs`: a copy until a node's policy can be read by a binary that links no node.
pub const OP_ALLOWLIST: &[&str] = &[
    "Abs",
    "Add",
    "And",
    "ArgMax",
    "ArgMin",
    "AveragePool",
    "BatchNormalization",
    "Cast",
    "Ceil",
    "Clip",
    "Concat",
    "Constant",
    "ConstantOfShape",
    "Conv",
    "Div",
    "Elu",
    "Equal",
    "Erf",
    "Exp",
    "Expand",
    "Flatten",
    "Floor",
    "Gather",
    "GatherElements",
    "Gemm",
    "GlobalAveragePool",
    "GlobalMaxPool",
    "Greater",
    "HardSigmoid",
    "Identity",
    "InstanceNormalization",
    "LayerNormalization",
    "LeakyRelu",
    "Less",
    "Log",
    "LogSoftmax",
    "MatMul",
    "Max",
    "MaxPool",
    "Mean",
    "Min",
    "Mul",
    "Neg",
    "Not",
    "Or",
    "Pad",
    "Pow",
    "PRelu",
    "Range",
    "Reciprocal",
    "ReduceMax",
    "ReduceMean",
    "ReduceMin",
    "ReduceSum",
    "Relu",
    "Reshape",
    "Resize",
    "Selu",
    "Shape",
    "Sigmoid",
    "Sign",
    "Slice",
    "Softmax",
    "Softplus",
    "Split",
    "Sqrt",
    "Squeeze",
    "Sub",
    "Sum",
    "Tanh",
    "Tile",
    "Transpose",
    "Unsqueeze",
    "Where",
];
pub const OPSET_MIN: i64 = 13;
pub const OPSET_MAX: i64 = 19;

/// The operators a graph uses that admission refuses: anything off [`OP_ALLOWLIST`], an operator
/// of another domain included, since the list names the default domain's.
pub fn refused(stats: &Stats) -> Vec<String> {
    stats.operators.iter().filter(|op| !OP_ALLOWLIST.contains(&op.as_str())).cloned().collect()
}

/// Whether the graph's opset is one admission accepts.
pub fn opset_allowed(stats: &Stats) -> bool {
    (OPSET_MIN..=OPSET_MAX).contains(&stats.opset)
}

fn parse(bytes: &[u8]) -> Result<ModelProto, String> {
    <ModelProto as Message>::decode(bytes).map_err(|e| format!("not an ONNX document: {e}"))
}

/// The graph's declared input and output names, in order.
pub fn io_names(bytes: &[u8]) -> Result<(Vec<String>, Vec<String>), String> {
    let m = parse(bytes)?;
    let g = m.graph.ok_or_else(|| "the document carries no graph".to_string())?;
    Ok((
        g.input.iter().map(|v| v.name.clone()).collect(),
        g.output.iter().map(|v| v.name.clone()).collect(),
    ))
}

pub fn stats(bytes: &[u8]) -> Result<Stats, String> {
    let m = parse(bytes)?;
    let opset = m
        .opset_import
        .iter()
        .find(|o| o.domain.is_empty() || o.domain == "ai.onnx")
        .map(|o| o.version)
        .unwrap_or(0);
    let g = m.graph.ok_or_else(|| "the document carries no graph".to_string())?;
    let mut acc = Acc::default();
    acc.graph(&g);
    for f in &m.functions {
        acc.nodes += f.node.len() as u64;
        for n in &f.node {
            acc.node(n);
        }
    }
    Ok(Stats {
        parameters: acc.params,
        nodes: acc.nodes,
        operators: acc.ops.into_iter().collect(),
        ir_version: m.ir_version,
        opset,
    })
}

/// How a graph's multiply-accumulate work divides between convolutions with a spatial kernel and
/// plain matrix products, per output position -- Orion 1.12.0's `model::conv_work`, copied
/// deliberately, because it is what a node reads to decide whether a graph runs on a plan per
/// concrete shape (`model.rs`, [`crate::model::specialising_pays`]), and a match played here has to
/// run the plan a node runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ConvWork {
    /// Per output position, in convolutions whose kernel covers more than one position:
    /// `out × in/group × kernel`.
    pub spatial: u64,
    /// The same for 1x1 convolutions and for `MatMul`/`Gemm` against a stored weight: `out × in`.
    pub pointwise: u64,
}

/// The operator a node asks for, qualified by its domain unless that is the default one.
fn operator(n: &NodeProto) -> String {
    if n.domain.is_empty() || n.domain == "ai.onnx" {
        n.op_type.clone()
    } else {
        format!("{}.{}", n.domain, n.op_type)
    }
}

/// Where a matrix-product operator's weight operand is, or `None` for an operator this accounting
/// does not know.
fn weight_operand(op: &str) -> Option<usize> {
    match op {
        "Conv" | "ConvInteger" | "MatMul" | "Gemm" | "MatMulInteger" => Some(1),
        "QLinearConv" | "QLinearMatMul" => Some(3),
        _ => None,
    }
}

/// Every operator that multiplies two operands as a matrix product. One whose weight this reader
/// cannot find makes the graph's work unknown, rather than silently small.
fn is_matrix_product(op: &str) -> bool {
    weight_operand(op).is_some()
        || matches!(op, "Einsum" | "ConvTranspose" | "FusedMatMul" | "Attention")
}

/// How `bytes`' convolution and matrix-product work divides, per output position. `None` when some
/// matrix product's weight is not a tensor the document stores, or sits in a subgraph or a
/// model-local function: the division is unknown, and the caller takes the general plan.
pub fn conv_work(bytes: &[u8]) -> Result<Option<ConvWork>, String> {
    let m = parse(bytes)?;
    let g = m.graph.as_ref().ok_or_else(|| "the document carries no graph".to_string())?;
    let nested = |n: &NodeProto| n.attribute.iter().any(|a| a.g.is_some() || !a.graphs.is_empty());
    if g.node.iter().any(nested)
        || m.functions.iter().flat_map(|f| &f.node).any(|n| is_matrix_product(&operator(n)))
    {
        return Ok(None);
    }

    // The shape of every stored tensor, by the name a node reads it under: the initializers, and
    // what a `Constant` node writes.
    let mut stored: std::collections::HashMap<&str, &[i64]> =
        g.initializer.iter().map(|t| (t.name.as_str(), t.dims.as_slice())).collect();
    for n in &g.node {
        if operator(n) == "Constant"
            && let (Some(out), Some(t)) =
                (n.output.first(), n.attribute.iter().find_map(|a| a.t.as_ref()))
        {
            stored.insert(out.as_str(), t.dims.as_slice());
        }
    }

    let product = |dims: &[i64]| {
        dims.iter().try_fold(1u64, |n, d| u64::try_from(*d).ok().map(|d| n.saturating_mul(d)))
    };
    let mut work = ConvWork::default();
    for n in &g.node {
        let op = operator(n);
        if !is_matrix_product(&op) {
            continue;
        }
        let Some(dims) = weight_operand(&op)
            .and_then(|at| n.input.get(at))
            .and_then(|name| stored.get(name.as_str()))
        else {
            return Ok(None);
        };
        let Some(total) = product(dims) else {
            return Ok(None);
        };
        // A convolution's weight is [out, in/group, k1, k2, …]: more than one kernel position
        // makes it spatial. A matrix product's is its [in, out] -- pointwise by definition.
        if op.contains("Conv") && dims.len() > 2 && product(&dims[2..]) > Some(1) {
            work.spatial = work.spatial.saturating_add(total);
        } else {
            work.pointwise = work.pointwise.saturating_add(total);
        }
    }
    Ok(Some(work))
}

#[derive(Default)]
struct Acc {
    params: u64,
    nodes: u64,
    ops: BTreeSet<String>,
}

impl Acc {
    fn graph(&mut self, g: &GraphProto) {
        for t in &g.initializer {
            self.params += t.dims.iter().map(|d| *d as u64).product::<u64>().max(1);
        }
        for t in &g.sparse_initializer {
            if let Some(v) = &t.values {
                self.params += v.dims.iter().map(|d| *d as u64).product::<u64>().max(1);
            }
        }
        self.nodes += g.node.len() as u64;
        for n in &g.node {
            self.node(n);
        }
    }

    fn node(&mut self, n: &NodeProto) {
        self.ops.insert(operator(n));
        for a in &n.attribute {
            self.attribute(a);
        }
    }

    /// A field that can hold an unbounded number of values counts. The rule is the carrier and not
    /// a table of operators, because a table is a thing an operator can be missing from.
    fn attribute(&mut self, a: &AttributeProto) {
        match AttributeType::try_from(a.r#type).unwrap_or(AttributeType::Undefined) {
            AttributeType::Tensor => {
                if let Some(t) = &a.t {
                    self.params += t.dims.iter().map(|d| *d as u64).product::<u64>().max(1);
                }
            }
            AttributeType::Tensors => {
                for t in &a.tensors {
                    self.params += t.dims.iter().map(|d| *d as u64).product::<u64>().max(1);
                }
            }
            AttributeType::SparseTensor => {
                if let Some(v) = a.sparse_tensor.as_ref().and_then(|s| s.values.as_ref()) {
                    self.params += v.dims.iter().map(|d| *d as u64).product::<u64>().max(1);
                }
            }
            AttributeType::Floats => self.params += a.floats.len() as u64,
            AttributeType::Ints => self.params += a.ints.len() as u64,
            AttributeType::Graph => {
                if let Some(g) = &a.g {
                    self.graph(g);
                }
            }
            AttributeType::Graphs => {
                for g in &a.graphs {
                    self.graph(g);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stats(ops: &[&str], opset: i64) -> Stats {
        Stats {
            parameters: 0,
            nodes: ops.len() as u64,
            operators: ops.iter().map(|s| s.to_string()).collect(),
            ir_version: 8,
            opset,
        }
    }

    #[test]
    fn an_operator_off_the_allowlist_is_named_and_one_on_it_is_not() {
        let s = stats(&["Conv", "GreaterOrEqual", "Relu", "ai.onnx.ml.TreeEnsemble"], 17);
        assert_eq!(
            refused(&s),
            vec!["GreaterOrEqual".to_string(), "ai.onnx.ml.TreeEnsemble".to_string()]
        );
        assert!(refused(&stats(&["Conv", "Greater", "Where", "Floor"], 17)).is_empty());
    }

    #[test]
    fn the_opset_must_sit_inside_the_deployments_range() {
        assert!(opset_allowed(&stats(&[], 13)) && opset_allowed(&stats(&[], 19)));
        assert!(!opset_allowed(&stats(&[], 12)) && !opset_allowed(&stats(&[], 20)));
    }

    #[test]
    fn the_allowlist_is_the_books_list_without_repeats() {
        let set: std::collections::BTreeSet<_> = OP_ALLOWLIST.iter().collect();
        assert_eq!(set.len(), OP_ALLOWLIST.len());
        assert!(!OP_ALLOWLIST.contains(&"GreaterOrEqual") && OP_ALLOWLIST.contains(&"Greater"));
    }
}
