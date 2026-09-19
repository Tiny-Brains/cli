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
    println!("{}  {}", slug, g.name);
    println!("    engine  {}", g.engine_digest);
    println!("    from    {}", g.source);
    println!("    boards  {} shipped (`tinybrains maps`)", g.catalogue().len());
    // A season's boards are uploaded, not shipped, and must fit what the release's own boards
    // span; an older release declares nothing, and says so by saying nothing.
    if let Some(e) = crate::cmd::maps::envelope_line(g) {
        println!("    season  a board may be {e}");
    }
}
