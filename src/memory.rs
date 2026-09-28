//! A seat's memory: the outputs a runner hands back on the seat's next view, and what they cost.
//!
//! A manifest may declare an output named `memory`, one named `ant_memory`, both or neither. The
//! runner keeps each seat's last value of each and puts it on that seat's next view under the same
//! key, and an adapter reads it back with `{"tensor": [{"var": "memory"}]}`. Like a node, this hands
//! the adapter a LIVE tensor: the view is built into the evaluator's arena and each memory goes in
//! as a `DataValue::Tensor`, so `var` on it yields the tensor itself and never an object with a
//! `tensor.dtype` inside. Only a file a trainer writes spells a memory in JSON, and [`Carry::load`]
//! decodes it on the way in.
//!
//! [`Carry`] is the rule, one per seat per match, used by `wave.rs` and by `check`'s round trip.
//! [`price`] and [`judge`] are the admit clock's verdicts on a declaration, computed from the
//! manifest and a weight class with no model run, as Soma computes them.

use std::collections::BTreeMap;

use dataflow_rs::datalogic_rs::bumpalo::{Bump, collections::Vec as BumpVec};
use dataflow_rs::datalogic_rs::datavalue::{DType, DataTensor, DataValue};
use serde_json::Value;

use crate::model::Inference;

/// The output names a runner carries, each under its own key on the next view.
pub const OUTPUTS: [&str; 2] = ["memory", "ant_memory"];

/// A weight class's two memory numbers are bounded by `weight_classes_ok()` in Soma's migration.
pub const FLAT_BYTES_MAX: u64 = 262_144;
pub const CELL_BYTES_MAX: u64 = 16;

/// One held tensor: an output exactly as the graph produced it.
#[derive(Clone, Debug, PartialEq)]
struct Held {
    dtype: DType,
    shape: Vec<usize>,
    bytes: Vec<u8>,
}

/// One seat's memory in one match. Seats never share one, even two seats of one model.
///
/// | When                   | the view's `memory` (and `ant_memory`)           |
/// |------------------------|--------------------------------------------------|
/// | turn 0                 | absent                                           |
/// | after an answered call | that call's output of the same name              |
/// | after a struck call    | unchanged: the last value the model wrote         |
///
/// "Answered" is Kalam's: the model call returned. A head the platform cannot read is a strike
/// but the call answered, so its memory is kept, exactly as `kalam-match-run` stores
/// `p{seat}.memory` before it decodes the head.
#[derive(Default, Clone, Debug, PartialEq)]
pub struct Carry(BTreeMap<&'static str, Held>);

impl Carry {
    /// The evaluator's input: the engine's view, built into `arena`, with each memory this seat
    /// holds as a live tensor under its own key. A key it holds none of is absent, never null,
    /// whatever the view came with.
    pub fn input<'a>(&self, view: &Value, arena: &'a Bump) -> Result<&'a DataValue<'a>, String> {
        let tree = DataValue::from_serde_value_in(view, arena);
        let DataValue::Object(pairs) = tree else { return Ok(arena.alloc(tree)) };
        let mut out = BumpVec::with_capacity_in(pairs.len() + self.0.len(), arena);
        out.extend(pairs.iter().filter(|(k, _)| !OUTPUTS.contains(k)).copied());
        for (name, h) in &self.0 {
            let t = DataTensor::from_bytes_in(h.dtype, &h.shape, &h.bytes, arena)
                .map_err(|e| format!("`{name}`: {e}"))?;
            out.push((*name, DataValue::Tensor(arena.alloc(t))));
        }
        Ok(arena.alloc(DataValue::Object(out.into_bump_slice())))
    }

    /// After the seat's call: an answered call's outputs of those names replace what was held; a
    /// struck one (`None`) leaves everything as it was. An answered call that lacks one of the
    /// outputs keeps the last value of it, which is Kalam's `??`.
    pub fn after(&mut self, answered: Option<&Inference>) {
        let Some(inf) = answered else { return };
        for name in OUTPUTS {
            let Some((_, dtype, shape, bytes)) = inf.outputs.iter().find(|(n, ..)| n == name)
            else {
                continue;
            };
            if let Some(dtype) = DType::from_name(dtype) {
                self.0.insert(name, Held { dtype, shape: shape.clone(), bytes: bytes.clone() });
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// A trainer's observation, from a file, split into the view and the memory it carries. A
    /// memory may be written in the tensor wire form (`{"tensor": {"dtype", "shape", "data"}}`, or
    /// its body alone), or as nested arrays, which are decoded into the dtype the manifest declares
    /// for the output of that name, since a runner hands back a tensor of that dtype. Either way
    /// the adapter is handed a live tensor, as on a node. Nested arrays under a name the manifest
    /// declares no output for stay in the view as JSON.
    pub fn load(
        obs: &Value,
        declared: &dyn Fn(&str) -> Option<String>,
    ) -> Result<(Value, Carry), String> {
        let mut view = obs.clone();
        let mut carry = Carry::default();
        let Some(obj) = view.as_object_mut() else { return Ok((view, carry)) };
        for name in OUTPUTS {
            let Some(v) = obj.get(name) else { continue };
            let arena = Bump::new();
            let dv = DataValue::from_serde_value_in(v, &arena);
            let t = match v {
                Value::Object(o) if o.contains_key(DataTensor::JSON_TAG) => {
                    DataTensor::from_json_value_in(&dv, &arena)
                }
                Value::Object(_) => {
                    let tagged = arena.alloc([(DataTensor::JSON_TAG, dv)]);
                    DataTensor::from_json_value_in(&DataValue::Object(&tagged[..]), &arena)
                }
                _ => {
                    let Some(dtype) = declared(name) else { continue };
                    let dt = DType::from_name(&dtype)
                        .ok_or_else(|| format!("no tensor dtype '{dtype}'"))?;
                    DataTensor::from_nested_in(&dv, dt, &arena)
                }
            }
            .map_err(|e| format!("`{name}` is not a tensor: {e}"))?;
            carry.0.insert(
                name,
                Held { dtype: t.dtype(), shape: t.shape().to_vec(), bytes: t.data().to_vec() },
            );
            obj.remove(name);
        }
        Ok((view, carry))
    }
}

/// What a manifest's memory costs: `bytes(cells) = fixed + per_cell × cells`, both outputs summed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Price {
    pub fixed: u64,
    pub per_cell: u64,
}

impl Price {
    pub fn bytes(&self, cells: u64) -> u64 {
        self.fixed.saturating_add(self.per_cell.saturating_mul(cells))
    }
}

/// A verdict the admit clock returns, with the sentence that explains it.
pub type Verdict = (&'static str, String);

/// Price a manifest's memory declaration, or `None` when it declares no memory output.
///
/// An output's numeric dimensions multiply. `memory` may name at most two dimensions and
/// `ant_memory` at most one, and no name twice within a shape. An output with any named dimension
/// is per cell (the product of its numeric dimensions, times the board's cells); one with none is
/// fixed. Anything else is `MEMORY_SHAPE`.
pub fn price(manifest: &Value) -> Result<Option<Price>, Verdict> {
    let shape_err = |m: String| ("MEMORY_SHAPE", m);
    let outputs = manifest["outputs"].as_array().map(Vec::as_slice).unwrap_or_default();
    let mut total: Option<Price> = None;
    for (name, names_max) in [("memory", 2usize), ("ant_memory", 1usize)] {
        let mut decls = outputs.iter().filter(|o| o["name"] == name);
        let Some(decl) = decls.next() else { continue };
        // Soma's `bad := bad OR (o ->> 'name') = ANY (seen)`: a name declared twice is a shape
        // refusal, not the first declaration priced and the rest ignored.
        if decls.next().is_some() {
            return Err(shape_err(format!("the `{name}` output is declared more than once")));
        }
        let dtype = decl["dtype"].as_str().ok_or_else(|| {
            shape_err(format!("the `{name}` output is not declared with a dtype"))
        })?;
        let width = width(dtype).ok_or_else(|| {
            shape_err(format!("the `{name}` output's dtype '{dtype}' has no width"))
        })?;
        let dims = decl["shape"].as_array().ok_or_else(|| {
            shape_err(format!("the `{name}` output is not declared with a shape"))
        })?;
        let mut elems = 1u64;
        let mut named: Vec<&str> = Vec::new();
        for d in dims {
            match d {
                Value::Number(n) => match n.as_u64().filter(|n| *n > 0) {
                    Some(n) => elems = elems.saturating_mul(n),
                    None => {
                        return Err(shape_err(format!(
                            "the `{name}` output has a dimension {n}, not a positive integer"
                        )));
                    }
                },
                Value::String(s) => {
                    if named.contains(&s.as_str()) {
                        return Err(shape_err(format!(
                            "the `{name}` output names '{s}' twice in one shape"
                        )));
                    }
                    named.push(s);
                }
                other => {
                    return Err(shape_err(format!(
                        "the `{name}` output has a dimension {other}, neither a count nor a name"
                    )));
                }
            }
        }
        if named.len() > names_max {
            return Err(shape_err(format!(
                "the `{name}` output names {} dimensions ({}); it may name at most {names_max}",
                named.len(),
                named.join(", ")
            )));
        }
        let bytes = elems.saturating_mul(width);
        let p = total.get_or_insert(Price { fixed: 0, per_cell: 0 });
        if named.is_empty() {
            p.fixed = p.fixed.saturating_add(bytes);
        } else {
            p.per_cell = p.per_cell.saturating_add(bytes);
        }
    }
    Ok(total)
}

/// A class of 0 flat and 0 a cell against a manifest that declares a memory at all.
///
/// `memory_price()` reaches this BEFORE it judges the shape, so a caller holding a shape refusal
/// has to ask this first or it reports `MEMORY_SHAPE` where the admit clock reports this.
pub fn not_allowed() -> Verdict {
    (
        "MEMORY_NOT_ALLOWED",
        "the manifest declares a memory output and this class allows none".to_string(),
    )
}

/// The admit clock's cap on a priced memory: `flat + cell × cells`, checked at the smallest and
/// the largest board the envelope allows, which covers every board between because both sides
/// are linear in the cell count.
pub fn judge(
    price: &Price,
    flat: u64,
    cell: u64,
    cells_min: u64,
    cells_max: u64,
) -> Result<(), Verdict> {
    if flat == 0 && cell == 0 {
        return Err(not_allowed());
    }
    for cells in [cells_min, cells_max] {
        let (bytes, cap) = (price.bytes(cells), flat.saturating_add(cell.saturating_mul(cells)));
        if bytes > cap {
            return Err((
                "MEMORY_TOO_LARGE",
                format!("{bytes} bytes at {cells} cells, over this class's cap of {cap}"),
            ));
        }
    }
    Ok(())
}

/// Bytes per element, by the dtype names a manifest writes.
fn width(dtype: &str) -> Option<u64> {
    Some(match dtype {
        "bool" | "i8" | "u8" => 1,
        "f16" | "i16" | "u16" => 2,
        "f32" | "i32" | "u32" => 4,
        "f64" | "i64" | "u64" => 8,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn inference(outputs: &[(&str, &str, &[usize], Vec<u8>)]) -> Inference {
        Inference {
            outputs: outputs
                .iter()
                .map(|(n, d, s, b)| (n.to_string(), d.to_string(), s.to_vec(), b.clone()))
                .collect(),
            ops: 0,
            peak_ops: 0,
            infer_us: 0,
        }
    }

    fn f32s(v: &[f32]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_le_bytes()).collect()
    }

    /// What `logic` evaluates to on this seat's input, as JSON: a tensor comes out in its wire form.
    fn eval(logic: Value, view: Value, carry: &Carry) -> Value {
        crate::model::eval(&logic, &view, carry).unwrap()
    }

    fn wire(dtype: &str, shape: &[usize], bytes: &[u8]) -> Value {
        let dt = DType::from_name(dtype).unwrap();
        let t = dataflow_rs::datalogic_rs::datavalue::OwnedDataTensor::from_bytes(
            dt,
            shape.to_vec(),
            bytes,
        )
        .unwrap();
        serde_json::to_value(&t).unwrap()
    }

    #[test]
    fn a_view_carries_no_memory_on_turn_zero() {
        let c = Carry::default();
        assert_eq!(eval(json!({"var": "memory"}), json!({"mine": []}), &c), Value::Null);
        assert_eq!(eval(json!({"var": "mine"}), json!({"mine": []}), &c), json!([]));
    }

    #[test]
    fn a_key_the_seat_holds_nothing_of_is_absent_whatever_the_view_came_with() {
        let view = json!({"mine": [], "memory": [1, 2]});
        assert_eq!(eval(json!({"var": "memory"}), view, &Carry::default()), Value::Null);
    }

    #[test]
    fn an_answered_call_hands_back_its_own_outputs_of_those_names() {
        let mut c = Carry::default();
        c.after(Some(&inference(&[
            ("policy", "f32", &[1, 5], f32s(&[0.0; 5])),
            ("memory", "f32", &[1, 2], f32s(&[1.0, 2.0])),
            ("ant_memory", "u8", &[1, 3, 1], vec![7, 8, 9]),
        ])));
        let read = |name: &str| eval(json!({"tensor": [{"var": name}]}), json!({"mine": []}), &c);
        assert_eq!(read("memory"), wire("f32", &[1, 2], &f32s(&[1.0, 2.0])));
        assert_eq!(read("ant_memory"), wire("u8", &[1, 3, 1], &[7, 8, 9]));
        assert_eq!(eval(json!({"var": "policy"}), json!({}), &c), Value::Null);
    }

    #[test]
    fn the_adapter_is_handed_a_live_tensor_and_not_its_wire_form() {
        let mut c = Carry::default();
        c.after(Some(&inference(&[("ant_memory", "f32", &[1, 1, 2], f32s(&[4.0, 5.0]))])));
        // A node's context holds the output tensor, so there is no `tensor.dtype` to read in it.
        assert_eq!(eval(json!({"var": "ant_memory.tensor.dtype"}), json!({}), &c), Value::Null);
        assert_eq!(eval(json!({"var": "ant_memory.tensor"}), json!({}), &c), Value::Null);
        let shape = eval(json!({"shape": [{"var": "ant_memory"}]}), json!({}), &c);
        assert_eq!(shape, json!([1, 1, 2]));
    }

    #[test]
    fn a_struck_call_keeps_the_last_memory_written() {
        let mut c = Carry::default();
        c.after(Some(&inference(&[("memory", "f32", &[1], f32s(&[3.0]))])));
        let before = c.clone();
        c.after(None);
        c.after(None);
        assert_eq!(c, before);
    }

    #[test]
    fn an_answered_call_without_the_output_keeps_the_last_value() {
        let mut c = Carry::default();
        c.after(Some(&inference(&[("memory", "f32", &[1], f32s(&[3.0]))])));
        c.after(Some(&inference(&[("policy", "f32", &[1, 5], f32s(&[0.0; 5]))])));
        let got = eval(json!({"tensor": [{"var": "memory"}]}), json!({}), &c);
        assert_eq!(got, wire("f32", &[1], &f32s(&[3.0])));
    }

    #[test]
    fn seats_never_share_a_memory() {
        let (mut a, b) = (Carry::default(), Carry::default());
        a.after(Some(&inference(&[("memory", "f32", &[1], f32s(&[1.0]))])));
        assert!(b.is_empty() && !a.is_empty());
    }

    #[test]
    fn a_memory_in_a_file_is_loaded_as_a_live_tensor() {
        let declared = |n: &str| (n == "memory").then(|| "u8".to_string());
        // Nested arrays, decoded into the dtype the manifest declares for the output.
        let obs = json!({"mine": [], "memory": [[1, 2], [3, 4]], "ant_memory": [[1.5]]});
        let (view, c) = Carry::load(&obs, &declared).unwrap();
        let got = eval(json!({"tensor": [{"var": "memory"}]}), view.clone(), &c);
        assert_eq!(got, wire("u8", &[2, 2], &[1, 2, 3, 4]));
        assert_eq!(eval(json!({"var": "memory.0"}), view.clone(), &c), Value::Null);
        // Declared by no output: left in the view for the adapter to decode with a dtype of its own.
        assert_eq!(view["ant_memory"], json!([[1.5]]));
        // The wire form, and its body alone.
        let w = wire("f32", &[1, 2], &f32s(&[7.0, 8.0]));
        for m in [w.clone(), w["tensor"].clone()] {
            let (view, c) = Carry::load(&json!({"ant_memory": m}), &declared).unwrap();
            let got = eval(json!({"tensor": [{"var": "ant_memory"}]}), view.clone(), &c);
            assert_eq!(got, w);
            assert_eq!(eval(json!({"var": "ant_memory.tensor"}), view, &c), Value::Null);
        }
    }

    fn manifest(outputs: Value) -> Value {
        json!({"outputs": outputs})
    }

    #[test]
    fn a_manifest_without_memory_is_not_priced() {
        let m = manifest(json!([{"name": "policy", "dtype": "f32", "shape": [1, 5, "H", "W"]}]));
        assert_eq!(price(&m), Ok(None));
    }

    #[test]
    fn named_dimensions_are_per_cell_and_counts_are_fixed() {
        let m = manifest(json!([
            {"name": "memory", "dtype": "f32", "shape": [1, 2, "H", "W"]},
            {"name": "ant_memory", "dtype": "i16", "shape": [1, "N", 4]},
        ]));
        assert_eq!(price(&m), Ok(Some(Price { fixed: 0, per_cell: 2 * 4 + 4 * 2 })));
        let m = manifest(json!([{"name": "memory", "dtype": "u8", "shape": [1, 8]}]));
        assert_eq!(price(&m), Ok(Some(Price { fixed: 8, per_cell: 0 })));
    }

    #[test]
    fn a_shape_the_rules_refuse_is_memory_shape() {
        for outputs in [
            json!([{"name": "memory", "dtype": "f32", "shape": ["A", "B", "C"]}]),
            json!([{"name": "ant_memory", "dtype": "f32", "shape": ["N", "M"]}]),
            json!([{"name": "memory", "dtype": "f32", "shape": ["H", "H"]}]),
            json!([{"name": "memory", "dtype": "bf16", "shape": [1]}]),
            json!([{"name": "memory", "dtype": "f32"}]),
            json!([{"name": "memory", "shape": [1]}]),
        ] {
            assert_eq!(price(&manifest(outputs.clone())).map_err(|e| e.0), Err("MEMORY_SHAPE"));
        }
    }

    #[test]
    fn one_name_declared_twice_is_memory_shape() {
        // Soma prices the whole outputs array and refuses a repeated name; taking the first
        // declaration and ignoring the rest priced a manifest it refuses.
        let outputs = json!([
            {"name": "memory", "dtype": "f32", "shape": [4]},
            {"name": "memory", "dtype": "f32", "shape": [1024, "H", "W"]},
        ]);
        assert_eq!(price(&manifest(outputs)).map_err(|e| e.0), Err("MEMORY_SHAPE"));
    }

    #[test]
    fn a_class_with_no_memory_refuses_any() {
        let p = Price { fixed: 1, per_cell: 0 };
        assert_eq!(judge(&p, 0, 0, 576, 14_880).map_err(|e| e.0), Err("MEMORY_NOT_ALLOWED"));
    }

    #[test]
    fn the_cap_is_checked_at_both_ends_of_the_envelope() {
        // 1 KiB flat and 1 byte a cell: a u8 plane per cell plus 1 KiB fits exactly everywhere.
        assert_eq!(judge(&Price { fixed: 1024, per_cell: 1 }, 1024, 1, 576, 14_880), Ok(()));
        // Over at the smallest board only: a large fixed part against a cap that is mostly cells.
        let p = Price { fixed: 2000, per_cell: 0 };
        let e = judge(&p, 0, 2, 576, 14_880).unwrap_err();
        assert_eq!(e.0, "MEMORY_TOO_LARGE");
        assert!(e.1.contains("576 cells"));
        // Over at the largest board only: more per cell than the class, under its flat part.
        let p = Price { fixed: 0, per_cell: 2 };
        let e = judge(&p, 4096, 1, 576, 14_880).unwrap_err();
        assert_eq!(e.0, "MEMORY_TOO_LARGE");
        assert!(e.1.contains("14880 cells"));
    }
}
