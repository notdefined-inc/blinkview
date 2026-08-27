//! Ground-truth evaluation of face assignment.
//!
//! The library's folder names are the labels. Seeds `people.json` from the first N
//! faces of each folder, then assigns the rest and reports accuracy. Used to confirm
//! the ADR-0003 thresholds still hold after the port.
use openfoto_core::faces::{assign, people::People, pipeline};
use openfoto_core::{scan, Library};
use std::collections::BTreeMap;

fn folder(path: &str) -> String {
    path.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or("(root)".into())
}

fn main() -> anyhow::Result<()> {
    let root = std::env::args().nth(1).expect("usage: eval_faces <library> [seeds]");
    let seeds: usize = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(5);
    let mut lib = Library::open(&root)?;
    scan::scan(&mut lib, false)?;
    let st = pipeline::analyze(&lib, pipeline::DEFAULT_SCORE)?;
    println!(
        "analysed {} photos ({} cached): {} faces, {} too small to embed, {} errors",
        st.photos, st.skipped_cached, st.faces, st.too_small, st.errors.len()
    );

    let hash_to_folder: BTreeMap<String, String> =
        lib.index.all()?.into_iter().map(|r| (r.hash, folder(&r.path))).collect();

    // Seed identities from the first few faces of each folder.
    let mut people = People::default();
    let mut used: BTreeMap<String, usize> = BTreeMap::new();
    // Ground truth is only trustworthy for solo shots: a group photo filed under
    // "Me" also contains other people's faces, and counting those as mislabels
    // measures the fixture, not the matcher.
    let all = lib.all_faces()?;
    let mut per_photo: BTreeMap<String, usize> = BTreeMap::new();
    for f in &all {
        *per_photo.entry(f.hash.clone()).or_default() += 1;
    }
    let faces: Vec<_> = all
        .into_iter()
        .filter(|f| f.embedding.is_some() && per_photo[&f.hash] == 1)
        .collect();
    println!("using {} solo-shot faces as ground truth", faces.len());
    let mut held_out = Vec::new();
    for f in &faces {
        let label = hash_to_folder.get(&f.hash).cloned().unwrap_or_default();
        let n = used.entry(label.clone()).or_default();
        if *n < seeds {
            people.add_references(&label, vec![f.embedding.clone().unwrap()]);
            *n += 1;
        } else {
            held_out.push((label, f.embedding.clone().unwrap()));
        }
    }
    println!("\nseeded {} people with {seeds} references each", people.people.len());

    let opt = assign::Options::default();
    let (mut right, mut wrong, mut amb, mut unk) = (0, 0, 0, 0);
    let mut confusion: BTreeMap<(String, String), usize> = BTreeMap::new();
    for (truth, e) in &held_out {
        match assign::assign(e, &people, &opt) {
            assign::Assignment::Person { name, .. } => {
                if &name == truth { right += 1 } else { wrong += 1 }
                *confusion.entry((truth.clone(), name)).or_default() += 1;
            }
            assign::Assignment::Ambiguous { .. } => amb += 1,
            assign::Assignment::Unknown { .. } => unk += 1,
        }
    }
    let total = held_out.len().max(1);
    println!("\nheld-out faces: {}", held_out.len());
    println!("  correct   {right:>4}  ({:.0}%)", 100.0 * right as f32 / total as f32);
    println!("  WRONG     {wrong:>4}  ({:.0}%)", 100.0 * wrong as f32 / total as f32);
    println!("  ambiguous {amb:>4}  (left in place by design)");
    println!("  unknown   {unk:>4}  (left in place by design)");
    if wrong > 0 {
        println!("\nmisassignments:");
        for ((t, g), n) in confusion.iter().filter(|((t, g), _)| t != g) {
            println!("  {t} -> {g}: {n}");
        }
    }
    Ok(())
}
