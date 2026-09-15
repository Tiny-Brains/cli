use crate::registry::{Game, Registry};

pub fn run() -> Result<(), String> {
    let path = Registry::find()?;
    let reg = Registry::load(&path)?;
    println!("registry {}", path.display());
    // The evaluator, named by the library that IS it. An adapter is priced and run by datalogic
    // on a node and by datalogic here; a version skew between the two is what makes a local pass
    // and a remote refusal disagree, so the number is printed rather than assumed.
    println!("evaluator datalogic {}", crate::model::DATALOGIC_VERSION);
    println!();
    for slug in reg.games.keys() {
        match reg.resolve(slug, &path) {
            Ok(g) => describe(slug, &g),
            Err(e) => println!("{slug}  UNRESOLVED: {e}"),
        }
    }
    Ok(())
}

fn describe(slug: &str, g: &Game) {
    let presets: Vec<String> = g
        .manifest
        .get("presets")
        .and_then(|p| p.as_array())
        .map(|a| {
            a.iter()
                .map(|p| {
                    format!(
                        "{} ({} seats, {} boards)",
                        p["name"].as_str().unwrap_or("?"),
                        p["players"].as_u64().unwrap_or(0),
                        p["maps"].as_u64().unwrap_or(0)
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    println!("{}  {}", slug, g.name);
    println!("    engine  {}", g.engine_digest);
    println!("    from    {}", g.source);
    println!("    presets {}", presets.join(", "));
    println!("    boards  {}", g.catalogue().len());
}
