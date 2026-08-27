//! openfoto — a local-first photo organizer. Your folders are the database.
//!
//! Nothing mutating happens without `--apply`. Every command that can change the
//! library prints its plan first.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use openfoto_core::{dedupe, journal::Journal, rename, scan, Library};
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
        Cmd::Dedupe { rmse, dest, apply } => {
            let mut lib = open(&cli)?;
            let opt = dedupe::Options { rmse: *rmse, dest: dest.clone(), ..Default::default() };
            let n = dedupe::ensure_signatures(&lib)?;
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
