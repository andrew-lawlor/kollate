<p align="center">
  <img src="crates/kollate/data/io.github.andrew_lawlor.Kollate.svg" width="112" alt="Kollate icon">
</p>

<h1 align="center">Kollate</h1>

<p align="center">
  Collect and curate the highlights, notes and vocabulary from your Kobo e-reader.<br>
  A native GNOME app for Linux. Offline, read-only on your device, and free of duplicates.
</p>

<p align="center">
  <img src="docs/screenshots/inbox.png" alt="Kollate's inbox listing new highlights grouped by book">
</p>

## What it does

Plug in your Kobo and Kollate imports everything you've marked while reading:

- **Highlights and notes**, in all four Kobo colours, sorted into chapters in reading order.
- **Handwritten markups** from stylus models (tested on the Libra Colour), shown as the page image the Kobo saved.
- **Vocabulary Builder words.** Kollate goes further than the Kobo here: it finds the sentence you looked each word up in, inside the book itself, and adds an offline dictionary definition.

Then you curate:

- An **Inbox** of new highlights you can triage from the keyboard: `K` keep, `A` archive, `S` star, `E` edit, `Delete` trash.
- **Star, tag, archive** and **search** everything.
- **Fix typos or rewrite notes.** Kollate keeps the Kobo's original and can restore it.
- **Pick the right context sentence** and edit definitions for each word, and track words as *New*, *Learning*, *Known* or *Ignored*.

And export:

- **Obsidian:** one note per book plus a Vocabulary note, synced into your vault (optionally after every import). Highlights can be linked individually, and anything you write under the `%% kollate:user` line is kept.
- **Anki:** a vocabulary deck (word → meaning, plus fill-in-the-blank from the book's sentence) and an optional highlights deck. Exporting again updates your cards without losing study progress.
- **JSON** backup, **CSV** (highlights or vocabulary) and **Readwise** CSV.

## Screenshots

| | |
|---|---|
| ![A book page with its cover, reading progress and highlights by chapter](docs/screenshots/book.png) | ![Vocabulary words with definitions and the sentence each was found in](docs/screenshots/vocabulary.png) |
| **Books:** cover, progress, highlights by chapter | **Vocabulary:** definition and the sentence from your book |
| ![Word dialog for choosing the context sentence and editing the definition](docs/screenshots/word.png) | ![Dark mode, with a starred and tagged note](docs/screenshots/dark.png) |
| **Words:** pick the context, edit the definition | **Dark mode**, tags and stars |

<p align="center">
  <img src="docs/screenshots/export.png" width="70%" alt="Export dialog with Obsidian, Anki and file options">
</p>

## Your Kobo stays untouched, and nothing goes online

- Kollate **never writes to your Kobo.** It copies the Kobo's database to a temporary folder and reads the copy, so you can eject at any time.
- **No duplicates, ever.** Highlights are matched by the Kobo's own IDs, and by content if the IDs change (a factory reset, a second device, or calibre re-sending a book). Re-importing the same Kobo changes nothing.
- **Your library is the source of truth.** Highlights you delete on the Kobo stay in Kollate, flagged "deleted on Kobo". If you've edited something in Kollate and it later changes on the Kobo, your version wins and the Kobo's is kept alongside it.
- **Fully offline.** Definitions come from the bundled [Open English WordNet](https://en-word.net). You can add StarDict dictionaries or [kaikki.org](https://kaikki.org) Wiktionary extracts in Preferences.

## Install

### Debian / Ubuntu (.deb)

Build the package (see below) and install it:

```sh
./scripts/build-deb.sh
sudo apt install ./target/debian/kollate_0.1.0-1_amd64.deb
```

The package includes the app, the `kollate-cli` tool and the WordNet dictionary. It needs GTK ≥ 4.12 and libadwaita ≥ 1.5, which Debian 13 and Ubuntu 24.04 or newer provide.

### From source

```sh
# Debian/Ubuntu build dependencies (plus Rust from https://rustup.rs)
sudo apt install build-essential pkg-config libgtk-4-dev libadwaita-1-dev

./scripts/fetch-wordnet.sh      # one-time: download and build the dictionary (~24 MB)
cargo run --release -p kollate
```

## Usage

1. Start Kollate and connect your Kobo with a USB cable. Choose *Connect* on the Kobo if it asks.
2. Kollate spots the Kobo and imports it automatically. You can change this in *Preferences*: *Ask first* or *Do nothing*.
3. Work through the **Inbox**, browse by book or tag, and visit **Vocabulary**.
4. Open **Export** (`Ctrl+E`), pick a folder in your Obsidian vault and/or export an Anki deck.
5. Press **Eject** in the banner when you're done.

Your library lives in `~/.local/share/kollate/`.

### Command line

`kollate-cli` does the same work from a terminal:

```sh
kollate-cli inspect /media/$USER/KOBOeReader          # show what's on the Kobo (read-only)
kollate-cli import  /media/$USER/KOBOeReader --dry-run
kollate-cli import  /media/$USER/KOBOeReader
kollate-cli library                                    # what's in your library
kollate-cli contexts /media/$USER/KOBOeReader          # context sentences for vocab words
kollate-cli export obsidian ~/Vault/Books
kollate-cli export anki Kollate.apkg --highlights
kollate-cli dict build-stardict ~/dicts/foo.ifo foo.db # convert a dictionary
```

## Compatibility

Developed and tested on a **Kobo Libra Colour** (firmware 4.45, database version 176). Other Kobo models use the same database layout and should work, but they aren't tested yet; reports are welcome. Context sentences need sideloaded, DRM-free books. Kobo Store books with DRM still import, just without context sentences.

## Development

```
crates/
  kollate-core/   Kobo reader, library (SQLite), import/dedup, dictionaries, EPUB context, exports
  kollate-cli/    command-line tool
  kollate/        GTK 4 / libadwaita app
tests/fixtures/   a trimmed Kobo database used by the tests
SPEC.md           design notes, data model and what was verified on a real device
```

```sh
cargo test          # unit and integration tests, including against a real Kobo database
cargo clippy --all-targets
```

## License

Kollate is free software under the **GNU General Public License v3.0 or later**; see [LICENSE](LICENSE).

Definitions come from **Open English WordNet** © Princeton University and the Open English WordNet contributors, under [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/).

Kollate isn't affiliated with or endorsed by Rakuten Kobo. "Kobo" is a trademark of Rakuten Kobo Inc.
