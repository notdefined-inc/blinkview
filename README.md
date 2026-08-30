<div align="center">

<img src="assets/logo.png" width="120" alt="">

# OpenFoto

**Your photos are already organised. They're in folders. Why does no app believe you?**

[Install](#install) · [Why this exists](#the-problem-nobody-will-fix) · [What it does](#what-it-actually-does) · [Build from source](#build-it-yourself)

</div>

---

## The problem nobody will fix

You have a hard drive. On it, a folder called `Trip`. Inside that, `Greece Day 1`, `Greece Day 2`, `Swiss Day 1`. You made those folders. You know exactly what's in them.

Now try to look at them.

**Apple** — a company worth more than most countries' economies — ships an operating system whose photo app cannot do this. Photos wants to *import* your pictures into a library it controls, in a location it chooses, under a structure it invents. Your folders are not welcome. You can point Finder at the folder and press space, which is a file previewer wearing a photo app's coat: no faces, no search, no dates, no grid worth the name.

So you look elsewhere, and the market splits neatly in two.

**The free ones that aren't.** They install cheerfully. Then face recognition is a subscription. Then export is Pro. Then a modal appears every third launch, warmly explaining that your memories deserve better, for £39.99 a year. Your photos have become a recurring revenue line on someone's dashboard.

**The genuinely free ones that look it.** Some are magnificent — decades of engineering, real capability, given away by people who owed you nothing. And they were designed when Windows XP was current. Nested toolbars. Nine-step import wizards. A UI that makes a simple thing feel like filing a tax return.

Then the self-hosted generation arrived — and they're *good*. But they want a Docker daemon, a Postgres container, a reverse proxy, and a mental model where photos live inside a server. Wonderful, if you wanted to run a server. You wanted to look at your holiday.

Every one of them, free or paid, old or new, makes the same move: **it takes your photos and puts them somewhere it owns.**

## The idea

What Obsidian is to Markdown, OpenFoto is to your photo library.

**The folders are the database.** Not a metaphor. There is no library file, no import step, no proprietary container. Point OpenFoto at a folder and it reads what's there. Rename a folder in Finder mid-session and it keeps up, because photos are tracked by content hash, not by path.

Delete OpenFoto tomorrow and you lose *nothing*. Your folders sit exactly where they were, named exactly what you named them — plus a small readable `openfoto.json` beside each one holding the ratings and names you added, because those were yours and no machine can reproduce them.

Everything else — thumbnails, the index, face embeddings — lives in a `.openfoto/` folder that is **safe to delete**. Delete it and it rebuilds. That's not a caveat; it's the promise the whole design is built to keep.

## What it actually does

**Finds faces, offline.** Detection and recognition run on your machine. Nothing is uploaded. Nothing phones home. You name someone once and OpenFoto files the rest — and when it isn't sure, it says so and leaves the photo alone, because a confidently wrong answer is worse than an honest shrug.

**Searches by what's *in* the picture.** Type `a church` and get the church. Type `snowy mountains` and get the mountains. No tagging, no training — a vision model reads the pixels. Ask for `the sea` in a library with no sea and it returns nothing, on purpose. Below the confidence threshold it would rather say nothing than guess.

**Understands sentences.** `a church sam 18 august 2026` is three filters — a scene, a person, a date — and gives you two photographs. Not because there's a language model in the loop; because there's a grammar, and a grammar is faster, offline, and never invents a folder that doesn't exist.

**Does what you tell it.** `move all my august photos to Trip/Greece Day3` shows you exactly what will happen, then waits. Every change is journalled. ⌘Z undoes it — the file move *and* the ratings that travelled with it.

**Finds the duplicates you actually have.** Burst shots, near-identical, three seconds apart. Perceptual hashing to find candidates, real pixel comparison to confirm, and complete-linkage clustering so a chain of vaguely-similar photos never gets mistaken for a pile of duplicates.

**Edits without destroying.** Crop, straighten, rotate, exposure. The original moves to a visible `Originals/` folder by default, because "non-destructive" should mean you can see the thing that wasn't destroyed.

**Stays quick when it gets big.** The grid is virtualised: 200,000 photos render with about 55 cells alive. Thumbnails are produced as you scroll, and the camera's own embedded preview is used when it's there — reading 37 KB instead of decoding twelve megapixels. Analysis decodes each photo *once* and takes the thumbnail, the faces and the search embedding from that single pass.

## What it deliberately doesn't do

- **No cloud, no account, no sync.** There is nowhere to sign in.
- **No telemetry.** Nothing is measured, collected or sent. There is no analytics code to audit because there is none to write.
- **No paid tier.** There is no feature behind a wall, because there is no wall.
- **No library format.** If OpenFoto vanished tomorrow, your folders wouldn't notice.

## Install

Grab the installer for your platform from [**Releases**](https://github.com/notdefined-inc/openfoto/releases/latest).

| | |
|---|---|
| **macOS** | `.dmg` — Apple Silicon only. ONNX Runtime no longer ships a macOS x64 build, so Intel Macs would need it compiled from source |
| **Windows** | `.msi` |
| **Linux** | `.AppImage` and `.deb` — needs glibc 2.38+ (Ubuntu 24.04, Fedora 39, Debian 13 or newer) |

The app is not notarized, so the first launch needs a nudge: on macOS, right-click → Open; on Windows, *More info* → *Run anyway*. An Apple Developer ID costs money a project with no revenue doesn't have.

If macOS says the app is **damaged and can't be opened**, that is the v0.1.0 build, not your download: it shipped without a bundle signature, and Gatekeeper reports an invalid signature as damage. Later builds are ad-hoc signed and open with right-click → Open. To use v0.1.0 anyway, strip the quarantine flag:

```sh
xattr -cr /Applications/OpenFoto.app
```

**Face recognition and scene search are optional.** They need about 200 MB of models, downloaded on first use, from Hugging Face and the OpenCV model zoo. Every download is checked against a pinned SHA-256 before it's installed. Skip it and everything else still works.

## Build it yourself

You need [Rust](https://rustup.rs) 1.88+ and [Node](https://nodejs.org) (for the frontend tests only — there's no bundler, no build step, no `node_modules`).

```bash
git clone https://github.com/notdefined-inc/openfoto
cd openfoto

cargo run -p openfoto-desktop --release      # the app
cargo run -p openfoto-cli -- --help          # the command line

cargo test --workspace                       # 124 tests
node apps/desktop/tests/grammar.test.mjs     # the command grammar
```

On Linux you'll need the usual WebKitGTK development packages; Tauri's [prerequisites page](https://tauri.app/start/prerequisites/) lists them per distribution.

There's also a CLI, which does everything the app does and is the honest way to try this on a library you care about:

```bash
openfoto scan     -C ~/Photos     # index; never modifies a photo
openfoto analyze  -C ~/Photos     # thumbnails, faces and scene search in one pass
openfoto find     -C ~/Photos "a night sky"
openfoto dedupe   -C ~/Photos     # preview first, always
openfoto undo     -C ~/Photos     # reverse anything
```

## How it's built

Rust throughout — a core library, a CLI, and a [Tauri](https://tauri.app) desktop shell sharing the same engine. SQLite for the disposable index, ONNX Runtime for the models, and no Python anywhere near the runtime.

The decisions that shaped it are written down in [`docs/DECISIONS/`](docs/DECISIONS) — including the ones that turned out wrong and had to be corrected in public. [ADR-0008](docs/DECISIONS/ADR-0008-semantic-search.md) is the honest one: a quantised text model looked fine, measured 4× smaller, and was silently wrong in a way that nearly shipped.

## Contributing

Issues and pull requests welcome. The house rules are in [`AGENTS.md`](AGENTS.md), which is mostly a list of mistakes already made here so nobody has to make them twice — a lock held across a slow scan, a comment claiming an optimisation that wasn't there, a test that passed for the wrong reason.

## Licence

[GPL-3.0-or-later](LICENSE).

OpenFoto is free software, and the licence exists to keep it that way: if you ship a modified version, you ship your changes too. It's GPLv3 rather than GPLv2 for a specific reason — the windowing and tokenizer libraries underneath are Apache-2.0, which the FSF considers incompatible with GPLv2.

The models are downloaded, not bundled, and carry their own licences: [YuNet](https://github.com/opencv/opencv_zoo) and [SFace](https://github.com/opencv/opencv_zoo) from the OpenCV model zoo, and [MobileCLIP-S0](https://huggingface.co/Xenova/mobileclip_s0) for scene search.
