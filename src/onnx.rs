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
    "Abs", "Add", "And", "ArgMax", "ArgMin", "AveragePool", "BatchNormalization", "Cast", "Ceil",
    "Clip", "Concat", "Constant", "ConstantOfShape", "Conv", "Div", "Elu", "Equal", "Erf", "Exp",
    "Expand", "Flatten", "Floor", "Gather", "GatherElements", "Gemm", "GlobalAveragePool",
    "GlobalMaxPool", "Greater", "HardSigmoid", "Identity", "InstanceNormalization",
    "LayerNormalization", "LeakyRelu", "Less", "Log", "LogSoftmax", "MatMul", "Max", "MaxPool",
    "Mean", "Min", "Mul", "Neg", "Not", "Or", "Pad", "Pow", "PRelu", "Range", "Reciprocal",
    "ReduceMax", "ReduceMean", "ReduceMin", "ReduceSum", "Relu", "Reshape", "Resize", "Selu",
    "Shape", "Sigmoid", "Sign", "Slice", "Softmax", "Softplus", "Split", "Sqrt", "Squeeze", "Sub",
    "Sum", "Tanh", "Tile", "Transpose", "Unsqueeze", "Where",
];
pub const OPSET_MIN: i64 = 13;
pub const OPSET_MAX: i64 = 19;

/// The operators a graph uses that admission refuses: anything off [`OP_ALLOWLIST`], an operator
/// of another domain included, since the list names the default domain's.
pub fn refused(stats: &Stats) -> Vec<String> {
    stats
        .operators
        .iter()
        .filter(|op| !OP_ALLOWLIST.contains(&op.as_str()))
        .cloned()
        .collect()
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
        let domain = n.domain.as_str();
        self.ops.insert(if domain.is_empty() || domain == "ai.onnx" {
            n.op_type.clone()
        } else {
            format!("{domain}.{}", n.op_type)
        });
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
        assert_eq!(refused(&s), vec!["GreaterOrEqual".to_string(), "ai.onnx.ml.TreeEnsemble".to_string()]);
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
