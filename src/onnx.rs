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
//! attributes, counts the same as the honest exporter's. Counting initializers alone is exactly
//! the hole `axon` had.

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
