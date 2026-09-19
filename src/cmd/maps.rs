//! `tinybrains maps`: the boards a release ships, written out -- and a board checked the way the
//! platform checks an upload.
//!
//! **A season's boards are in no release** (N28). An admin uploads them to Soma, which refuses one
//! outside the cartridge's `limits.boards` and asks the engine's own `worldgen` whether it can be
//! played. `maps check` asks the same two things of a file on a laptop, so a folder of season boards
//! is known good before anyone uploads it -- and the envelope is the cartridge's, read from
//! `cartridge.json`, never a number typed into this binary.

use std::path::PathBuf;

use serde_json::{Value, json};

use crate::cartridge::Cartridge;
use crate::cmd::open_game;
use crate::registry::{Game, read_board};
use crate::store;

pub fn run(args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str) {
        Some("export") => export(&args[1..]),
        Some("check") => check(&args[1..]),
        _ => list(args),
    }
}

fn list(args: &[String]) -> Result<(), String> {
    let slug = args.first().filter(|s| !s.starts_with('-')).map(String::as_str);
    let game = open_game(slug)?;
    println!("{}  {} boards", game.slug, game.catalogue().len());
    for m in game.catalogue() {
        println!(
            "  {:<18} {:>3}x{:<3} {} seats  food {:<3} {}",
            m["id"].as_str().unwrap_or("?"),
            m["rows"].as_u64().unwrap_or(0),
            m["cols"].as_u64().unwrap_or(0),
            m["players"].as_u64().unwrap_or(0),
            m["food_target"].as_u64().unwrap_or(0),
            store::short(m["sha256"].as_str().unwrap_or("")),
        );
    }
    if let Some(e) = envelope_line(&game) {
        println!("  a season's board may be {e}");
    }
    Ok(())
}

fn export(args: &[String]) -> Result<(), String> {
    let slug = args.first().filter(|s| !s.starts_with('-')).map(String::as_str);
    let game = open_game(slug)?;
    let dir = PathBuf::from(args.get(1).cloned().unwrap_or_else(|| "maps".to_string()));
    let src = game
        .maps_dir
        .as_ref()
        .ok_or("this game's boards are not available as files from where it resolved")?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;

    let mut n = 0;
    for m in game.catalogue() {
        let id = m["id"].as_str().unwrap_or_default();
        let want = m["sha256"].as_str().unwrap_or_default();
        let from = src.join(format!("{id}.json"));
        let bytes = std::fs::read(&from).map_err(|e| format!("{}: {e}", from.display()))?;
        // The catalogue's digest is over the file as committed, so a mismatch is a stale checkout.
        let got = store::digest(&bytes);
        if got != want {
            return Err(format!(
                "{}\n  catalogue says {want}\n  the file is    {got}\n\
                 the checkout and its manifest disagree -- rebuild the cartridge",
                from.display()
            ));
        }
        std::fs::write(dir.join(format!("{id}.json")), &bytes)
            .map_err(|e| format!("{}: {e}", dir.display()))?;
        n += 1;
    }
    println!("wrote {n} boards to {}", dir.display());
    Ok(())
}

/// `maps check [--game SLUG] FILE...`: each board against the envelope, then through the engine.
fn check(args: &[String]) -> Result<(), String> {
    let (slug, files) = crate::cmd::game_and_rest(args)?;
    if files.is_empty() {
        return Err("which boards?\n\n  tinybrains maps check ../maps/*.json".to_string());
    }
    let game = open_game(slug.as_deref())?;
    let cart = Cartridge::open(&game.component).map_err(|e| e.to_string())?;
    match envelope_line(&game) {
        Some(e) => println!("{}: a season's board may be {e}", game.slug),
        None => println!(
            "{}: this release declares no limits.boards, so only the engine's own checks apply",
            game.slug
        ),
    }

    let mut refused = 0;
    for f in &files {
        let verdict = read_board(&PathBuf::from(f)).and_then(|b| judge(&game, &cart, &b));
        match verdict {
            Ok(line) => println!("  ok       {f}  {line}"),
            Err(e) => {
                refused += 1;
                println!("  REFUSED  {f}  {e}");
            }
        }
    }
    println!();
    if refused > 0 {
        return Err(format!("{refused} of {} boards would be refused", files.len()));
    }
    println!("all {} boards would be accepted", files.len());
    Ok(())
}

/// What Soma's upload asks, in its order: a header of the right shape, inside the envelope, and a
/// board the engine will open. The last is the engine's own verdict in its own words.
fn judge(game: &Game, cart: &Cartridge, board: &Value) -> Result<String, String> {
    let num = |k: &str| board.get(k).and_then(Value::as_u64);
    let id = board.get("id").and_then(Value::as_str).unwrap_or("");
    let (Some(players), Some(rows), Some(cols)) = (num("players"), num("rows"), num("cols")) else {
        return Err("a board needs whole numbers for players, rows and cols".to_string());
    };
    if id.is_empty()
        || !id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err(format!("'{id}' is not a board id: lowercase letters, digits and '-'"));
    }
    if let Some(env) = game.envelope() {
        let pair = |k: &str| -> Option<(u64, u64)> {
            let a = env.get(k)?.as_array()?;
            Some((a.first()?.as_u64()?, a.get(1)?.as_u64()?))
        };
        if let Some((lo, hi)) = pair("players")
            && !(lo..=hi).contains(&players)
        {
            return Err(format!("{players} seats, outside {lo} to {hi}"));
        }
        if let Some((lo, hi)) = pair("sides") {
            for (name, side) in [("rows", rows), ("cols", cols)] {
                if !(lo..=hi).contains(&side) {
                    return Err(format!("{side} {name}, outside {lo} to {hi} a side"));
                }
            }
        }
        if let Some(max) = env.get("cells_max").and_then(Value::as_u64)
            && rows * cols > max
        {
            return Err(format!("{} cells, above the {max} admission was proved on", rows * cols));
        }
    }
    let req = json!({ "seeds": [0], "map": board, "players": players, "max_turns": 1 });
    cart.invoke(&format!("tb.{}.worldgen", game.slug), &req)
        .map_err(|e| format!("the engine refuses it: {e}"))?;
    Ok(format!("{id}  {rows}x{cols}  {players} seats"))
}

/// `limits.boards` in words, or nothing for a release from before N28.
pub fn envelope_line(game: &Game) -> Option<String> {
    let env = game.envelope()?;
    let pair = |k: &str| -> String {
        env.get(k)
            .and_then(Value::as_array)
            .map(|a| a.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(" to "))
            .unwrap_or_else(|| "?".to_string())
    };
    Some(format!(
        "{} seats, {} a side, at most {} cells",
        pair("players"),
        pair("sides"),
        env.get("cells_max").map_or("?".to_string(), |v| v.to_string())
    ))
}
