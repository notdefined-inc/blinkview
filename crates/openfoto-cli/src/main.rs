//! openfoto — a local-first photo organizer. Your folders are the database.
//!
//! Nothing mutating happens without `--apply`. Every command that can change the
//! library prints its plan first.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
mod web;

use openfoto_core::faces::{assign, people::People, pipeline, review};
use openfoto_core::{dedupe, journal::Journal, rename, scan, scenery, Library};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "openfoto", version, about = "Local-first photo organizer. Your folders are the database.")]
struct Cli {
    /// Library root (defaults to the current directory).
    #[arg(short = 'C', long, global = true)]
    library: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Index the library. Never modifies photos.
    Scan {
        /// Re-hash every file instead of trusting size+mtime.
        #[arg(long)]
        rehash: bool,
    },
    /// What the index knows, and how it differs from the disk.
    Status,
    /// Detect people, review clusters, and file photos by person.
    Faces {
        #[command(subcommand)]
        cmd: FacesCmd,
    },
    /// Set aside photos with no close-up person.
    Scenery {
        /// Largest face, as a fraction of image width, still counted as scenery.
        #[arg(long, default_value_t = scenery::DEFAULT_MAX_FACE)]
        max_face: f32,
        #[arg(long, default_value = scenery::DEFAULT_DEST)]
        dest: String,
        #[arg(long)]
        apply: bool,
    },
    /// Find burst near-duplicates and set them aside.
    Dedupe {
        /// Pixel-difference threshold. Lower is stricter. 0.20/0.30/0.45 moved
        /// 134/284/549 photos on the reference library.
        #[arg(long, default_value_t = dedupe::DEFAULT_RMSE)]
        rmse: f32,
        /// Folder the duplicates go to.
        #[arg(long, default_value = dedupe::DEFAULT_DEST)]
        dest: String,
        #[arg(long)]
        apply: bool,
    },
    /// Give every file a date-time filename.
    Rename {
        #[arg(long, default_value = rename::DEFAULT_FORMAT)]
        format: String,
        /// Actually move files. Without this, prints the plan and exits.
        #[arg(long)]
        apply: bool,
    },
    /// Reverse an applied operation (the most recent, unless one is named).
    Undo {
        id: Option<String>,
        #[arg(long)]
        apply: bool,
    },
    /// List applied operations.
    History,
}

#[derive(Subcommand)]
enum FacesCmd {
    /// Detect and embed faces. Reads only; never moves a photo.
    Analyze {
        #[arg(long, default_value_t = pipeline::DEFAULT_SCORE)]
        score: f32,
    },
    /// Open the review page to name the people found.
    Review {
        /// Maximum cosine distance for two faces to share a cluster.
        #[arg(long, default_value_t = 0.55)]
        distance: f32,
        /// Write the page to a file instead of serving it. For design work.
        #[arg(long)]
        dump: Option<PathBuf>,
    },
    /// List known people and their reference counts.
    People,
    /// Move photos into a folder per person.
    File {
        #[arg(long)]
        apply: bool,
    },
}

/// A single rewritten line, so a long run visibly advances without scrolling.
fn cli_progress(label: &'static str) -> impl Fn(usize, usize) + Sync {
    use std::io::Write;
    move |done, total| {
        if total == 0 {
            return;
        }
        let width = 24usize;
        let filled = done * width / total.max(1);
        eprint!(
            "\r  {label} [{}{}] {done}/{total}",
            "#".repeat(filled),
            " ".repeat(width - filled)
        );
        if done == total {
            eprintln!();
        }
        let _ = std::io::stderr().flush();
    }
}

fn open(cli: &Cli) -> Result<Library> {
    let root = cli
        .library
        .clone()
        .unwrap_or(std::env::current_dir().context("resolving current directory")?);
    Library::open(root)
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match &cli.cmd {
        Cmd::Scan { rehash } => {
            let mut lib = open(&cli)?;
            let st = scan::scan(&mut lib, *rehash)?;
            println!(
                "scanned {} files  ({} hashed, {} unchanged, {} moved externally, {} removed)",
                st.seen, st.hashed, st.unchanged, st.moved, st.removed
            );
            for e in st.errors.iter().take(10) {
                eprintln!("  error: {e}");
            }
            if st.errors.len() > 10 {
                eprintln!("  ... and {} more errors", st.errors.len() - 10);
            }
        }
        Cmd::Status => {
            let lib = open(&cli)?;
            let rows = lib.index.all()?;
            let missing = rows.iter().filter(|r| !lib.abs(&r.path).exists()).count();
            let mut by_dir: std::collections::BTreeMap<&str, usize> = Default::default();
            for r in &rows {
                *by_dir.entry(r.path.rsplit_once('/').map(|(d, _)| d).unwrap_or("(root)")).or_default() += 1;
            }
            println!("library: {}", lib.root().display());
            println!("indexed: {} files", rows.len());
            for (d, n) in &by_dir {
                println!("  {d:<24} {n:>6}");
            }
            if missing > 0 {
                println!("\n{missing} indexed files are no longer on disk — run `openfoto scan`.");
            }
            let sources = rows.iter().filter_map(|r| r.taken_src.as_deref()).fold(
                std::collections::BTreeMap::<&str, usize>::new(),
                |mut m, s| { *m.entry(s).or_default() += 1; m },
            );
            if !sources.is_empty() {
                println!("\ncapture time from: {sources:?}");
            }
        }
        Cmd::Faces { cmd } => match cmd {
            FacesCmd::Analyze { score } => {
                let lib = open(&cli)?;
                let st = pipeline::analyze_with_progress(&lib, *score, &cli_progress("analysing"))?;
                println!(
                    "analysed {} photos ({} already done): {} faces, {} too small to embed",
                    st.photos, st.skipped_cached, st.faces, st.too_small
                );
                for e in st.errors.iter().take(5) {
                    eprintln!("  error: {e}");
                }
            }
            FacesCmd::People => {
                let lib = open(&cli)?;
                let people = People::load(&lib.vault())?;
                if people.is_empty() {
                    println!("no people yet — run `openfoto faces review`");
                }
                for p in &people.people {
                    println!("  {:<20} {} reference faces", p.name, p.references.len());
                }
            }
            FacesCmd::File { apply } => {
                let mut lib = open(&cli)?;
                let people = People::load(&lib.vault())?;
                if people.is_empty() {
                    println!("no people known yet — run `openfoto faces review` first.");
                    return Ok(());
                }
                let out = openfoto_core::faces::file::plan(&lib, &people, &assign::Options::default())?;
                println!("{} photos to file, {} shared between people (left in place), {} unclaimed",
                         out.plan.len(), out.shared.len(), out.unclaimed);
                for op in out.plan.ops.iter().take(8) {
                    println!("  {}  ->  {}", op.from(), op.to());
                }
                if out.plan.len() > 8 {
                    println!("  ... and {} more", out.plan.len() - 8);
                }
                for (p, why) in out.plan.skipped.iter().take(5) {
                    println!("  keeping {p} — {why}");
                }
                if !apply {
                    println!("\ndry run — nothing changed. Re-run with --apply to commit.");
                    return Ok(());
                }
                for name in people.people.iter().map(|p| &p.name) {
                    std::fs::create_dir_all(lib.abs(name))?;
                }
                let j = out.plan.apply(&mut lib)?;
                println!("\napplied. undo with:  openfoto undo {} --apply", j.id);
            }
            FacesCmd::Review { distance, dump } => {
                let lib = open(&cli)?;
                let mut people = People::load(&lib.vault())?;
                let opt = assign::Options::default();
                println!("building review…");
                let payload = review::build(&lib, &people, &opt, *distance)?;
                if payload.clusters.is_empty() {
                    println!("no unassigned faces to review.");
                    return Ok(());
                }
                println!(
                    "{} clusters, {} unassigned faces",
                    payload.clusters.len(),
                    payload.unassigned_faces
                );
                let json = serde_json::to_string(&payload)?;
                if let Some(path) = dump {
                    std::fs::write(path, web::render_page(&json))?;
                    println!("wrote {}", path.display());
                    return Ok(());
                }
                let body = web::serve_review(&json)?;
                let result: review::ReviewResult = serde_json::from_str(&body)?;

                // Naming a cluster teaches the person that cluster's faces.
                let groups = pipeline::cluster_unassigned(&lib, &people, &opt, *distance)?;
                let mut learned = 0;
                for (id, name) in &result.assignments {
                    if let Some(g) = groups.get(*id) {
                        let refs: Vec<Vec<f32>> =
                            g.iter().filter_map(|f| f.embedding.clone()).collect();
                        learned += refs.len();
                        people.add_references(name, refs);
                    }
                }
                people.save(&lib.vault())?;
                println!(
                    "\nlearned {learned} reference faces across {} people.",
                    result.assignments.len()
                );
                println!("run `openfoto faces people` to see them.");
            }
        },
        Cmd::Scenery { max_face, dest, apply } => {
            let mut lib = open(&cli)?;
            let opt = scenery::Options { max_face: *max_face, dest: dest.clone() };
            let split = scenery::split(&lib, &opt)?;
            if split.unanalysed > 0 {
                println!("{} photos have no face data yet — run `openfoto faces analyze` first.",
                         split.unanalysed);
            }
            println!("{} photos with no close-up person, {} with someone at {:.0}% of frame or more",
                     split.scenery.len(), split.people, max_face * 100.0);
            if !apply {
                println!("\ndry run — nothing changed. Re-run with --apply to commit.");
                return Ok(());
            }
            std::fs::create_dir_all(lib.abs(dest))?;
            let plan = scenery::plan(&lib, &opt)?;
            let j = plan.apply(&mut lib)?;
            println!("applied. undo with:  openfoto undo {} --apply", j.id);
        }
        Cmd::Dedupe { rmse, dest, apply } => {
            let mut lib = open(&cli)?;
            let opt = dedupe::Options { rmse: *rmse, dest: dest.clone(), ..Default::default() };
            let n = dedupe::ensure_signatures_with_progress(&lib, &cli_progress("analysing"))?;
            if n > 0 {
                println!("analysed {n} photos");
            }
            let groups = dedupe::find_groups(&lib, &opt)?;
            let moves: usize = groups.iter().map(|g| g.duplicates.len()).sum();
            println!("{} groups, {} photos, {} would move (keeping the sharpest)",
                     groups.len(), moves + groups.len(), moves);
            for g in groups.iter().take(5) {
                println!("  keep {}", g.keep.path);
                for d in g.duplicates.iter().take(4) {
                    println!("       {}", d.path);
                }
                if g.duplicates.len() > 4 {
                    println!("       ... and {} more", g.duplicates.len() - 4);
                }
            }
            if groups.len() > 5 {
                println!("  ... and {} more groups", groups.len() - 5);
            }
            if !apply {
                println!("\ndry run — nothing changed. Re-run with --apply to commit.");
                return Ok(());
            }
            std::fs::create_dir_all(lib.abs(dest))?;
            let plan = dedupe::plan(&lib, &opt)?;
            let j = plan.apply(&mut lib)?;
            println!("\napplied. undo with:  openfoto undo {} --apply", j.id);
        }
        Cmd::Rename { format, apply } => {
            let mut lib = open(&cli)?;
            let plan = rename::plan(&lib, format)?;
            if plan.is_empty() {
                println!("nothing to rename — every file already matches the format.");
                return Ok(());
            }
            println!("{} renames planned:", plan.len());
            for op in plan.ops.iter().take(10) {
                println!("  {}  ->  {}", op.from(), op.to());
            }
            if plan.len() > 10 {
                println!("  ... and {} more", plan.len() - 10);
            }
            if !apply {
                println!("\ndry run — nothing changed. Re-run with --apply to commit.");
                return Ok(());
            }
            let j = plan.apply(&mut lib)?;
            println!("\napplied. undo with:  openfoto undo {} --apply", j.id);
        }
        Cmd::History => {
            let lib = open(&cli)?;
            let ids = Journal::list(&lib)?;
            if ids.is_empty() {
                println!("no operations recorded.");
            }
            for id in ids {
                let j = Journal::load(&lib, &id)?;
                println!("  {}  {} ops", j.id, j.ops.len());
            }
        }
        Cmd::Undo { id, apply } => {
            let mut lib = open(&cli)?;
            let ids = Journal::list(&lib)?;
            let target = match id {
                Some(i) => i.clone(),
                None => ids.last().cloned().context("no operations to undo")?,
            };
            let j = Journal::load(&lib, &target)?;
            println!("undo {} — {} ops", j.id, j.ops.len());
            if !apply {
                println!("dry run — nothing changed. Re-run with --apply to commit.");
                return Ok(());
            }
            let n = j.undo(&mut lib)?;
            println!("reversed {n} ops.");
        }
    }
    Ok(())
}
