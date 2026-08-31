//! blinkview — a local-first photo organizer. Your folders are the database.
//!
//! Nothing mutating happens without `--apply`. Every command that can change the
//! library prints its plan first.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
mod web;

use blinkview_core::faces::{assign, pipeline, review};
use blinkview_core::{dedupe, journal::Journal, rename, scan, scenery, Library};

/// Human-readable size of a cache directory, for `cache list`.
///
/// One level deep, which is where a cache keeps its bulk (`thumbs/`, `derived/`,
/// `faces/`); deeper traversal would make `list` walk every thumbnail.
fn dir_size(dir: &std::path::Path) -> String {
    let bytes = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| match e.metadata() {
                    Ok(m) if m.is_dir() => std::fs::read_dir(e.path())
                        .map(|sub| {
                            sub.flatten()
                                .filter_map(|s| s.metadata().ok())
                                .map(|m| m.len())
                                .sum::<u64>()
                        })
                        .unwrap_or(0),
                    Ok(m) => m.len(),
                    Err(_) => 0,
                })
                .sum::<u64>()
        })
        .unwrap_or(0);
    for (unit, factor) in [("GB", 1 << 30), ("MB", 1 << 20), ("KB", 1 << 10)] {
        if bytes >= factor {
            return format!("{:.1} {unit}", bytes as f64 / factor as f64);
        }
    }
    format!("{bytes} B")
}
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "blinkview",
    version,
    about = "Local-first photo organizer. Your folders are the database."
)]
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
    /// Build the thumbnail cache.
    Thumbs,
    /// Do everything a photograph needs from one decode: thumbnail, faces, embedding.
    Analyze {
        /// Skip face detection.
        #[arg(long)]
        no_faces: bool,
        /// Skip semantic embedding.
        #[arg(long)]
        no_semantic: bool,
    },
    /// Search photos by what is in them.
    Find {
        /// A phrase, e.g. "a dog on a beach".
        query: Vec<String>,
        #[arg(long, default_value_t = blinkview_core::semantic::DEFAULT_THRESHOLD)]
        threshold: f32,
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// Embed any photos that do not have an embedding yet, first.
        #[arg(long)]
        index: bool,
    },
    /// Download the face-detection models.
    Models {
        #[command(subcommand)]
        cmd: ModelsCmd,
    },
    /// The derived caches, held outside your photo folders since ADR-0019.
    Cache {
        #[command(subcommand)]
        cmd: CacheCmd,
    },
}

#[derive(Subcommand)]
enum CacheCmd {
    /// Every cached library: its id, the folder it last served, and its size.
    List,
    /// Delete caches whose library folder no longer exists. Prints what it removed.
    ///
    /// Removing a source in the app already takes its cache with it; this catches the
    /// rest — a folder deleted in Finder, a library renamed while the app was closed.
    Prune,
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

#[derive(Subcommand)]
enum ModelsCmd {
    /// Show which models are installed and where they are looked for.
    Status,
    /// Download any missing models into the user cache.
    Fetch,
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
                *by_dir
                    .entry(r.path.rsplit_once('/').map(|(d, _)| d).unwrap_or("(root)"))
                    .or_default() += 1;
            }
            println!("library: {}", lib.root().display());
            println!("indexed: {} files", rows.len());
            for (d, n) in &by_dir {
                println!("  {d:<24} {n:>6}");
            }
            if missing > 0 {
                println!("\n{missing} indexed files are no longer on disk — run `blinkview scan`.");
            }
            let sources = rows.iter().filter_map(|r| r.taken_src.as_deref()).fold(
                std::collections::BTreeMap::<&str, usize>::new(),
                |mut m, s| {
                    *m.entry(s).or_default() += 1;
                    m
                },
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
                let people = lib.people()?;
                if people.is_empty() {
                    println!("no people yet — run `blinkview faces review`");
                }
                for p in &people.people {
                    println!("  {:<20} {} reference faces", p.name, p.references.len());
                }
            }
            FacesCmd::File { apply } => {
                let mut lib = open(&cli)?;
                let people = lib.people()?;
                if people.is_empty() {
                    println!("no people known yet — run `blinkview faces review` first.");
                    return Ok(());
                }
                let out =
                    blinkview_core::faces::file::plan(&lib, &people, &assign::Options::default())?;
                println!(
                    "{} photos to file, {} shared between people (left in place), {} unclaimed",
                    out.plan.len(),
                    out.shared.len(),
                    out.unclaimed
                );
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
                println!("\napplied. undo with:  blinkview undo {} --apply", j.id);
            }
            FacesCmd::Review { distance, dump } => {
                let lib = open(&cli)?;
                let mut people = lib.people()?;
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
                lib.save_people(&people)?;
                println!(
                    "\nlearned {learned} reference faces across {} people.",
                    result.assignments.len()
                );
                println!("run `blinkview faces people` to see them.");
            }
        },
        Cmd::Scenery {
            max_face,
            dest,
            apply,
        } => {
            let mut lib = open(&cli)?;
            let opt = scenery::Options {
                max_face: *max_face,
                dest: dest.clone(),
            };
            let split = scenery::split(&lib, &opt)?;
            if split.unanalysed > 0 {
                println!(
                    "{} photos have no face data yet — run `blinkview faces analyze` first.",
                    split.unanalysed
                );
            }
            println!(
                "{} photos with no close-up person, {} with someone at {:.0}% of frame or more",
                split.scenery.len(),
                split.people,
                max_face * 100.0
            );
            if !apply {
                println!("\ndry run — nothing changed. Re-run with --apply to commit.");
                return Ok(());
            }
            std::fs::create_dir_all(lib.abs(dest))?;
            let plan = scenery::plan(&lib, &opt)?;
            let j = plan.apply(&mut lib)?;
            println!("applied. undo with:  blinkview undo {} --apply", j.id);
        }
        Cmd::Dedupe { rmse, dest, apply } => {
            let mut lib = open(&cli)?;
            let opt = dedupe::Options {
                rmse: *rmse,
                dest: dest.clone(),
                ..Default::default()
            };
            let n = dedupe::ensure_signatures_with_progress(&lib, &cli_progress("analysing"))?;
            if n > 0 {
                println!("analysed {n} photos");
            }
            let groups = dedupe::find_groups(&lib, &opt)?;
            let moves: usize = groups.iter().map(|g| g.duplicates.len()).sum();
            println!(
                "{} groups, {} photos, {} would move (keeping the sharpest)",
                groups.len(),
                moves + groups.len(),
                moves
            );
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
            println!("\napplied. undo with:  blinkview undo {} --apply", j.id);
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
            println!("\napplied. undo with:  blinkview undo {} --apply", j.id);
        }
        Cmd::Find {
            query,
            threshold,
            limit,
            index,
        } => {
            use blinkview_core::semantic;
            let lib = open(&cli)?;
            if !semantic::Encoder::available() {
                println!("the search models are not installed — run `blinkview models fetch`");
                return Ok(());
            }
            if *index {
                let st = semantic::analyze(&lib, &cli_progress("understanding"))?;
                println!(
                    "embedded {} photos ({} already done)",
                    st.embedded, st.skipped
                );
                for e in st.errors.iter().take(3) {
                    eprintln!("  error: {e}");
                }
            }
            let q = query.join(" ");
            if q.trim().is_empty() {
                println!("nothing to search for");
                return Ok(());
            }
            let indexed = lib.index.clip_count()?;
            if indexed == 0 {
                println!(
                    "no photos have been understood yet — run `blinkview find --index <query>`"
                );
                return Ok(());
            }
            let hits = semantic::search(&lib, &q, *threshold, *limit)?;
            let by_hash: std::collections::BTreeMap<_, _> = lib
                .index
                .all()?
                .into_iter()
                .map(|r| (r.hash, r.path))
                .collect();
            println!("{} of {indexed} photos match {q:?}", hits.len());
            for h in &hits {
                println!(
                    "  {:.3}  {}",
                    h.score,
                    by_hash.get(&h.hash).cloned().unwrap_or_default()
                );
            }
            if hits.is_empty() {
                println!(
                    "  (nothing above {threshold:.2} — the model is not confident enough to guess)"
                );
            }
        }
        Cmd::Thumbs => {
            let mut lib = open(&cli)?;
            let st = blinkview_core::analyze::run_with_progress(
                &mut lib,
                blinkview_core::analyze::Stages::only_thumbs(),
                &cli_progress("thumbnails"),
            )?;
            println!(
                "built {} thumbnails ({} from an embedded preview)",
                st.thumbs, st.from_preview
            );
        }
        Cmd::Analyze {
            no_faces,
            no_semantic,
        } => {
            let mut lib = open(&cli)?;
            let stages = blinkview_core::analyze::Stages {
                thumbs: true,
                faces: !no_faces,
                semantic: !no_semantic,
            };
            let st = blinkview_core::analyze::run_with_progress(
                &mut lib,
                stages,
                &cli_progress("analysing"),
            )?;
            println!(
                "{} photographs · {} decoded ({} from a preview) · {} thumbnails · {} faces · {} understood",
                st.considered, st.decoded, st.from_preview, st.thumbs, st.faces, st.embedded
            );
            for e in st.errors.iter().take(5) {
                eprintln!("  {e}");
            }
        }
        Cmd::Models { cmd } => {
            use blinkview_core::faces::{fetch, models};
            match cmd {
                ModelsCmd::Status => {
                    println!("looked for in:");
                    for d in models::search_paths() {
                        println!("  {}", d.display());
                    }
                    println!();
                    for spec in fetch::specs() {
                        let ok = fetch::is_present(&spec);
                        let where_ = models::find(spec.name)
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|_| "not found".into());
                        println!(
                            "  {:<12} {}  {}",
                            spec.name,
                            if ok { "ok     " } else { "MISSING" },
                            where_
                        );
                    }
                }
                ModelsCmd::Fetch => {
                    println!("cache: {}", fetch::cache_dir()?.display());
                    let sink = |name: &str, done: usize, total: usize| {
                        use std::io::Write;
                        let width = 24usize;
                        let filled = if total > 0 { done * width / total } else { 0 };
                        eprint!(
                            "\r  {name:<12} [{}{}] {:>3}%",
                            "#".repeat(filled),
                            " ".repeat(width - filled),
                            if total > 0 { done * 100 / total } else { 0 }
                        );
                        if done == total {
                            eprintln!();
                        }
                        let _ = std::io::stderr().flush();
                    };
                    let got = fetch::fetch_missing(&sink)?;
                    if got.is_empty() {
                        println!("all models already installed and verified.");
                    } else {
                        println!("installed: {}", got.join(", "));
                    }
                }
            }
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
        Cmd::Cache { cmd } => match cmd {
            CacheCmd::List => {
                let known = blinkview_core::cache::known();
                if known.is_empty() {
                    println!("no caches at {}", blinkview_core::cache::root().display());
                    return Ok(());
                }
                for (vault, path) in &known {
                    let size = dir_size(vault);
                    let where_ = match path {
                        Some(p) if p.exists() => p.display().to_string(),
                        Some(p) => format!("{} (gone — `blinkview cache prune`)", p.display()),
                        None => "unknown".to_string(),
                    };
                    let id = vault.file_name().unwrap().to_string_lossy();
                    println!("  {:<8} {:>9}  {}", &id[..8], size, where_);
                }
                println!("\ncache root: {}", blinkview_core::cache::root().display());
            }
            CacheCmd::Prune => {
                let mut removed = 0usize;
                for (vault, path) in blinkview_core::cache::known() {
                    // Only a cache naming a *vanished* folder is junk. One with no
                    // breadcrumb is left alone: unknown is not the same as gone.
                    let Some(gone) = path.filter(|p| !p.exists()) else {
                        continue;
                    };
                    if std::fs::remove_dir_all(&vault).is_ok() {
                        println!(
                            "  removed {}  (was {})",
                            &vault.file_name().unwrap().to_string_lossy()[..8],
                            gone.display()
                        );
                        removed += 1;
                    }
                }
                if removed == 0 {
                    println!("nothing to prune");
                }
            }
        },
    }
    Ok(())
}
