//! Where a match's wall clock went.
//!
//! A global, lock-free set of accumulators, written from the four places a match spends time --
//! the cartridge host, the manifest evaluator, the graph runner and the loop between them -- and
//! read once at the end. It is always recording (two atomics and an `Instant` per span, tens of
//! nanoseconds against milliseconds of work) and only printed when a command asks.
//!
//! **The two groups are not one list.** `Registry`..`Write` are disjoint and sum to the run;
//! `Instantiate`..`DecodeOut` decompose the four cartridge calls among them and so double-count on
//! purpose. Adding a phase to the first group without subtracting it from another is how a
//! breakdown starts summing to more than the clock it came from.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::time::Instant;

#[derive(Copy, Clone, Debug)]
pub enum P {
    Registry = 0,
    CartOpen,
    ModelLoad,
    Worldgen,
    Observe,
    Adapter,
    Graph,
    Head,
    Step,
    Finish,
    Write,
    Instantiate,
    EncodeIn,
    WasmCall,
    DecodeOut,
}

pub const N: usize = 15;
/// The disjoint group: these, plus what is left over, are the whole run.
pub const DISJOINT: usize = 11;

pub const NAMES: [&str; N] = [
    "registry + cache",
    "cartridge compile",
    "model load",
    "worldgen",
    "observe",
    "adapter (datalogic)",
    "graph (tract)",
    "head decode",
    "step",
    "finish",
    "replay write",
    "  wasm instantiate",
    "  json encode in",
    "  wasm call",
    "  json decode out",
];

static NANOS: [AtomicU64; N] = [const { AtomicU64::new(0) }; N];
static CALLS: [AtomicU64; N] = [const { AtomicU64::new(0) }; N];
static BYTES: [AtomicU64; N] = [const { AtomicU64::new(0) }; N];
static TURNS: Mutex<Vec<[u64; N]>> = Mutex::new(Vec::new());
/// Per graph node, when `TINYBRAINS_PROFILE_NODES` asks: the op as tract named it, what it cost, and how
/// many times it ran. A map rather than a slot per op because the graph is the model's, not ours.
static NODES: Mutex<Vec<(String, u64, u64)>> = Mutex::new(Vec::new());

/// Whether to take the per-node path, which trades `plan.run` for a state machine and a timer a
/// node. Read once: a graph is run six hundred times and an env lookup a node is not free.
pub fn profiling_nodes() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("TINYBRAINS_PROFILE_NODES").is_some())
}

pub fn node(op: &str, ns: u64) {
    if let Ok(mut v) = NODES.lock() {
        match v.iter_mut().find(|(n, ..)| n == op) {
            Some(e) => {
                e.1 += ns;
                e.2 += 1;
            }
            None => v.push((op.to_string(), ns, 1)),
        }
    }
}

pub fn nodes() -> Vec<(String, u64, u64)> {
    NODES.lock().map(|v| v.clone()).unwrap_or_default()
}

/// The graph, node by node -- printed only when the run asked for it.
pub fn report_nodes() {
    let mut v = nodes();
    if v.is_empty() {
        return;
    }
    v.sort_by_key(|(_, ns, _)| std::cmp::Reverse(*ns));
    let total: u64 = v.iter().map(|(_, ns, _)| ns).sum();
    println!();
    println!("inside the graph ({:.0} ms over {} runs)", total as f64 / 1e6, v[0].2);
    println!("{:<44} {:>9} {:>7} {:>11}", "node", "total ms", "share", "mean us");
    println!("{}", "-".repeat(73));
    for (op, ns, calls) in v.iter().take(20) {
        println!(
            "{:<44} {:>9.1} {:>6.1}% {:>11.1}",
            &op[..op.len().min(44)],
            *ns as f64 / 1e6,
            *ns as f64 * 100.0 / total.max(1) as f64,
            *ns as f64 / 1000.0 / (*calls).max(1) as f64
        );
    }
}

pub fn start() -> Instant {
    Instant::now()
}

pub fn stop(p: P, t: Instant) {
    let ns = t.elapsed().as_nanos() as u64;
    NANOS[p as usize].fetch_add(ns, Relaxed);
    CALLS[p as usize].fetch_add(1, Relaxed);
}

/// Bytes that crossed a boundary in this phase -- a JSON payload in or out of the sandbox. Size is
/// the lever for the encode/decode phases, so the breakdown reports it beside the time.
pub fn bytes(p: P, n: usize) {
    BYTES[p as usize].fetch_add(n as u64, Relaxed);
}

/// The running totals, called at each turn boundary so the shape of the match over time survives
/// into the report: an observation grows with the colony, and a flat mean hides that.
pub fn mark_turn() {
    let mut row = [0u64; N];
    for (i, cell) in row.iter_mut().enumerate() {
        *cell = NANOS[i].load(Relaxed);
    }
    if let Ok(mut t) = TURNS.lock() {
        t.push(row);
    }
}

pub struct Phase {
    pub name: &'static str,
    pub nanos: u64,
    pub calls: u64,
    pub bytes: u64,
}

pub fn read() -> Vec<Phase> {
    (0..N)
        .map(|i| Phase {
            name: NAMES[i],
            nanos: NANOS[i].load(Relaxed),
            calls: CALLS[i].load(Relaxed),
            bytes: BYTES[i].load(Relaxed),
        })
        .collect()
}

pub fn per_turn() -> Vec<[u64; N]> {
    TURNS.lock().map(|t| t.clone()).unwrap_or_default()
}

/// The breakdown, printed against the wall clock the caller measured -- so the row that is not a
/// phase at all ("unaccounted") is visible rather than quietly folded into the last one.
pub fn report(total: std::time::Duration) {
    let ph = read();
    let wall = total.as_nanos() as u64;
    let named: u64 = ph[..DISJOINT].iter().map(|p| p.nanos).sum();

    println!();
    println!("where the {:.0} ms went", wall as f64 / 1e6);
    println!(
        "{:<22} {:>9} {:>7} {:>9} {:>11} {:>10}",
        "phase", "total ms", "share", "calls", "mean us", "MB moved"
    );
    println!("{}", "-".repeat(73));
    let mut rows: Vec<&Phase> = ph[..DISJOINT].iter().filter(|p| p.calls > 0).collect();
    rows.sort_by_key(|p| std::cmp::Reverse(p.nanos));
    for p in rows {
        line(p, wall);
    }
    let rest = wall.saturating_sub(named);
    println!(
        "{:<22} {:>9.1} {:>6.1}% {:>9} {:>11} {:>10}",
        "unaccounted",
        rest as f64 / 1e6,
        rest as f64 * 100.0 / wall.max(1) as f64,
        "-",
        "-",
        "-"
    );
    println!();
    println!("inside the {} cartridge calls", ph[P::WasmCall as usize].calls);
    for p in &ph[DISJOINT..] {
        if p.calls > 0 {
            line(p, wall);
        }
    }
}

fn line(p: &Phase, wall: u64) {
    println!(
        "{:<22} {:>9.1} {:>6.1}% {:>9} {:>11.1} {:>10}",
        p.name,
        p.nanos as f64 / 1e6,
        p.nanos as f64 * 100.0 / wall.max(1) as f64,
        p.calls,
        p.nanos as f64 / 1000.0 / p.calls.max(1) as f64,
        if p.bytes > 0 { format!("{:.1}", p.bytes as f64 / 1e6) } else { "-".to_string() }
    );
}

/// The per-turn curve as CSV, for anything that wants to plot it.
pub fn write_csv(path: &str) -> std::io::Result<()> {
    let rows = per_turn();
    let mut s = String::from("turn");
    for n in NAMES {
        s.push(',');
        s.push_str(n.trim());
    }
    s.push('\n');
    let mut prev = [0u64; N];
    for (t, row) in rows.iter().enumerate() {
        s.push_str(&t.to_string());
        for i in 0..N {
            s.push(',');
            s.push_str(&(row[i].saturating_sub(prev[i])).to_string());
        }
        s.push('\n');
        prev = *row;
    }
    std::fs::write(path, s)
}
