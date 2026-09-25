# Kollate — Spec (v0.2)

A Linux desktop app that pulls highlights, notes, stylus markups and Vocab Builder words off a Kobo e‑reader over USB, keeps them in a local library with no duplicates, lets you curate them, and exports them to standard formats.

---

## 1. Goals / non-goals

**Goals**
- Plug in the Kobo → new items appear in the app automatically (or with one click).
- Re-importing is always safe: no duplicates, edits made in Kollate are never clobbered.
- The local library is the source of truth. Things deleted on the device (or lost to a factory reset) stay in Kollate.
- Fast curation: search, filter, tag, star, hide, edit, bulk actions.
- Export to Markdown (Obsidian-friendly), CSV, JSON, Anki, Readwise CSV. Exports are repeatable and incremental.

**Non-goals (v1)**
- Writing anything back to the Kobo, ever. The device is only ever read.
- Kobo cloud sync, Windows/macOS support, mobile.
- **Network access of any kind.** The Flatpak ships with no `--share=network`, and all lookups are offline.
- Reading or displaying the books themselves.

---

## 2. What the Kobo DB gives us (from the sample `KoboReader.sqlite`, DbVersion 176)

### 2.1 `Bookmark`: highlights, notes, markups (52 rows in the sample)

| Column | Use |
|---|---|
| `BookmarkID` (UUID, PK) | Stable device-side ID. Primary dedup key. |
| `VolumeID` | Book. `file:///mnt/onboard/<Author>/<Title>.kepub.epub` for sideloaded books, a UUID for store books. Joins `content.ContentID` where `ContentType=6`. |
| `ContentID` | Chapter file plus fragment, e.g. `…epub!OEBPS!chapter009.xhtml#Ref_16114`. |
| `Text` | Highlighted text. Contains stray leading/trailing whitespace and `\n`, so it needs normalizing. |
| `Annotation` | The user's note. Empty string or NULL when there isn't one. |
| `Type` | `highlight`, `note` (a highlight with an annotation), or `markup` (stylus handwriting; `Text` is NULL). |
| `Color` | 0 yellow, 1 pink, 2 blue, 3 green (verified on the device). |
| `StartContainerPath` / `StartOffset` / `End*` | Position inside the chapter, e.g. `span#kobo\.10\.4`. Used for sort order and fingerprinting. |
| `ChapterProgress` | 0–1 progress within the chapter. Used for ordering. |
| `DateCreated`, `DateModified` | ISO strings in mixed formats (`…36.152` without a Z, `…36Z`). Normalize to UTC. |
| `Hidden` | The string `'false'`/`'true'`. Skip hidden rows. |
| `ExtraAnnotationData` | Qt `QDataStream`-serialized `QVariantMap` (markups only). Holds `ETagSvg`, `ETagJpg`, `MarkupRect`, and so on. Must be read as bytes because it isn't valid UTF‑8. |

**Chapter title resolution:** strip the `#fragment` from `Bookmark.ContentID`, then find `content` rows with `ContentType=899` (TOC entries), `BookID = VolumeID` and `ContentID LIKE '<stripped>%'`. This works on the sample: "Prologue: Driftwood", "9. Inroads". When there are several matches (a nested TOC), take the deepest or last one.

**Markups:** the drawing isn't in the DB. It lives on the device at `.kobo/markups/<BookmarkID>.svg` (plus a `.jpg` page snapshot). I need to verify this path when the device is connected. Import copies both files into the library.

### 2.2 `WordList`: Vocab Builder (12 rows)

| Column | Notes |
|---|---|
| `Text` (PK) | The word as tapped, e.g. `séances`, `Demiurge`, `theophanies`. Case and inflection are preserved. |
| `VolumeId` | The book the word was looked up in. |
| `DictSuffix` | Dictionary language, e.g. `-en`. |
| `DateCreated` | ISO timestamp. |

Limitations:
- **No context sentence and no definition.** Kollate has to supply both (see §6).
- `Text` is the PK across all books. A word looked up again in another book overwrites or keeps a single row. Kollate therefore records each import where a word was seen as a separate *sighting*.
- If the user clears a word in Vocab Builder, it disappears from the device DB. Kollate keeps it.

### 2.3 `content` (ContentType 6 = book)
`Title`, `Attribution` (author), `Publisher`, `ISBN`, `Language`, `Series`, `SeriesNumber`, `Description`, `ImageId` (cover lookup under `.kobo-images/`), `___PercentRead`, `DateLastRead`, `ReadStatus`, `TimeSpentReading`.

### 2.4 Device identity
Read `.kobo/version` (comma-separated: serial, firmware, …, model ID) to tell devices apart. The sample `user` row is a Kobo demo account, and colour highlights plus stylus markup suggest a Libra **Colour** rather than a Libra 2. This matters only for feature scope: the Libra 2 has no colour or stylus.

---

## 3. Tech stack (decided)

**Rust · GTK 4 + libadwaita (gtk4-rs / libadwaita-rs) · SQLite · Flatpak**

The host has GTK 4.18 and libadwaita 1.7, so we can target `v4_18` / `v1_7` features.

| Concern | Crate |
|---|---|
| UI | `gtk4` 0.11 (`v4_18`), `libadwaita` 0.9 (`v1_7`), plus `gio` / `glib` for the mount monitor and async main loop. Widgets are built in Rust code (no `.ui` templates); styles live in `style.css`. |
| SQLite (device + library) | `rusqlite` with the `bundled` feature (plus FTS5), so there's no system sqlite dependency. |
| EPUB (context extraction) | `zip`, plus `quick-xml` to strip XHTML to text; sentence splitting with `unicode-segmentation`. |
| Dictionaries (offline) | Bundled Open English WordNet (prebuilt SQLite), plus our own importers for StarDict and kaikki.org Wiktionary JSONL. |
| Export | `minijinja` (Markdown templates), `csv`, `serde_json`, and a hand-rolled `.apkg` writer (an SQLite file plus a media JSON, zipped). |
| Misc | `serde`, `chrono`, `blake3` (fingerprints), `unicode-normalization`, `thiserror` / `anyhow`, `tracing`. |

Workspace layout: `kollate-core` is a pure library with no GTK; `kollate-cli` is a debug and scripting binary; `kollate` is the GTK app.

Flatpak permissions: `--filesystem=/media:ro` and `--filesystem=/run/media:ro` for the reader, plus `xdg-documents` or a portal-selected export folder for the Obsidian vault. No network.

## 4. Architecture

```
kollate/                     (cargo workspace)
  crates/kollate-core/src/
    kobo/reader.rs      # opens a *copy* of KoboReader.sqlite read-only; yields raw DTOs
    kobo/qvariant.rs    # minimal QDataStream QVariantMap decoder (markups)
    kobo/device.rs      # identify a mounted Kobo (.kobo/version)
    kobo/epub.rs        # read book files on the device for vocab context
    normalize.rs        # text cleanup, date parsing, fingerprints
    store/              # local library DB (schema, migrations, repositories)
    dict/               # WordNet + imported dictionaries (offline definitions)
    import.rs           # diff + merge device data into the store
    export/             # markdown(+vault), anki, csv, json, readwise
  crates/kollate-cli/     # `kollate-cli import <mount|db> [--dry-run]`, `export …`
  crates/kollate/         # GTK4/libadwaita app: window.rs (sidebar, list, actions), card.rs, edit.rs, style.css
  tests/fixtures/KoboReader.sqlite
```

**Reading the device safely**
1. Detect the mount (§5), then copy `.kobo/KoboReader.sqlite` and any `-wal`/`-shm` files to a temp dir.
2. Open the copy with `?mode=ro`. Never hold a handle on the device, because that blocks a clean eject.
3. Treat `ExtraAnnotationData` and any unexpected column as bytes. Check `DbVersion` and warn (don't fail) on unknown schema versions.

---

## 5. Device detection & import flow

- **Autodetect** by subscribing to `gio::VolumeMonitor` `mount-added` / `mount-removed`. On startup, also scan existing mounts. A mount counts as a Kobo if it contains `.kobo/KoboReader.sqlite`. There's no hard-coded path; `/media/andrew/KOBOeReader` is just the typical one. A manual "Import from folder/file…" option handles DB backups.
- On detection there are three behaviours, set in Preferences (stored in the library's `setting` table, key `on_connect`): **Import automatically** (default), **Ask first** (the banner offers Import), or **Do nothing**.
- An `AdwBanner` at the top of the content pane shows the connection state: "Importing from your Kobo Libra Colour…", then "Your Kobo Libra Colour is connected" with an **Eject** button.
- **Import pipeline while the reader is mounted:**
  1. Copy the DB, then diff and merge highlights, notes and vocab (§6).
  2. Copy markup `.svg`/`.jpg` files for new markups.
  3. **Vocab context pass (§8):** for every word without a context sentence, open the book's EPUB on the device and extract candidate sentences. This runs in the background, with progress shown in the header bar.
  4. Look up definitions offline for new words (§8).
  5. Copy covers from `.kobo-images/<h&0xff>/<(h&0xff00)>>8>/<ImageId> - N3_LIBRARY_FULL.parsed` (JPEG; `h` = Kobo's qhash of the ImageId, verified on the device) into `<library dir>/covers/`. Markup images go to `<library dir>/markups/<BookmarkID>.{svg,jpg}`. Files are written atomically (`.part`, then rename) and skipped when unchanged.
- Steps 2–5 are best-effort. If the reader is unplugged mid-way, the unfinished work is queued and resumes on the next connect.
- After an import, a toast reads "Kobo Libra Colour: 7 new highlights, 2 new words · Review". Clicking it opens the **Inbox**.
- Every import writes an `import_runs` row (device, time, counts: new, updated, unchanged, removed-on-device) for traceability.
- Eject uses `gio::Mount::eject_with_operation` (or unmount when the mount can't eject). It's disabled while an import is running.
- Book pages open with a header showing the cover, % read and last-read date. Markup cards show the page image, and "Open Page Image" opens it in the default viewer.

---

## 6. Deduplication & merge rules

### 6.1 Identity keys
| Entity | Primary key | Fallback fingerprint (catches factory reset, re-sideloaded books, a second device) |
|---|---|---|
| Book | `(device_id, VolumeID)` mapped to a `book_id` | `blake3(normalize(title) + normalize(author))`, plus ISBN if present |
| Annotation | `BookmarkID` (a globally unique UUID) | `blake3(book_fingerprint + normalize(text) + start_path + start_offset)`. For markups: `blake3(book_fingerprint + start/end anchors + created_at)`. |
| Vocab word | `(normalize(word), language)` | n/a. Each device/book occurrence becomes a `vocab_sighting` row. |

`normalize(text)` applies NFC, collapses whitespace, trims, and unifies curly/straight quotes. Book paths change when files are renamed, which is why the fingerprint exists.

### 6.2 Merge logic (per annotation)
- **Not in store:** insert it with `status=inbox`.
- **In store, device `DateModified` unchanged:** skip it.
- **In store, device changed (note edited, colour changed):** update the *device fields*. If the user has already edited that field in Kollate, keep the Kollate value, store the device value in `annotation_revisions`, and flag it as "device changed". The user can accept the device version from the UI.
- **In store but missing from the device:** set `removed_on_device_at`. The card gets a "deleted on Kobo" badge, it's listed in the **Deleted on Kobo** sidebar view (shown only when non-empty), and the import toast says "N deleted on Kobo". Nothing is ever deleted from the library.
- **Preference: "When a highlight is deleted on the Kobo"** (`setting.on_device_delete`). *Keep it* (the default) or *Move it to Trash*. Trashing saves the previous status in `annotation.status_before_removal` (migration 5). If the highlight reappears on the Kobo, it returns to that status, unless the user changed its status by hand in the meantime (`set_status` clears the saved value). The policy applies to deletions detected from then on, not retroactively.
- **Hidden on device:** handled the same as removed.

To support this, every annotation keeps the `device_*` fields (original) separate from `user_*` overrides (curated text, note), and the UI shows `coalesce(user, device)`.

---

## 7. Local data model (`~/.local/share/kollate/library.db`)

```
device(id, serial UNIQUE, model_id, firmware, first_seen_at, last_seen_at)
book(id, fingerprint UNIQUE, title, author, publisher, isbn, language, series, series_number,
     percent_read, last_read_at, user_title, user_author, cover_path, hidden, created_at, updated_at)
book_source(device_id, volume_id, book_id, image_id, PK(device_id, volume_id))
annotation(id, book_id, fingerprint, kind{highlight,note,markup},
     device_text, device_note, color, chapter_title, content_id, spine_index,
     start_path, start_offset, end_path, end_offset, chapter_progress, created_at, device_modified_at,
     user_text, user_note, starred, status{inbox,kept,archived,trashed},
     device_changed_at, removed_on_device_at, markup_svg_path, markup_jpg_path, imported_at, updated_at)
annotation_source(bookmark_id PK, device_id, annotation_id, first_seen_at, last_seen_at)
     -- several Kobo IDs → one annotation (factory reset, second device); drives "removed on device"
annotation_revision(id, annotation_id, field, old_value, new_value, source{device,user}, at)
vocab(id, word, key (NFC lowercase), language, lemma, definition, definition_source,
     status{new,learning,known,ignored}, starred, user_note, first_seen_at, imported_at, updated_at,
     UNIQUE(key, language))
vocab_sighting(id, vocab_id, book_id, device_id, surface_form, looked_up_at, context_sentence,
     UNIQUE(vocab_id, IFNULL(book_id,0), surface_form))
tag(id, name UNIQUE NOCASE, color)   annotation_tag(...)   vocab_tag(...)
import_run(id, device_id, started_at, finished_at, db_version, stats_json)
-- export settings live in `setting` (M5); FTS5 not needed so far
```
Schema versioning via `PRAGMA user_version` with forward-only migrations.

---

## 8. Vocab enrichment (offline only, while the reader is plugged in)

**Context sentences** come from the book on the device (`kobo/epub.rs`):
- A sideloaded, DRM-free book's `VolumeID` (`file:///mnt/onboard/...`) maps to its file under the mount. Books with `META-INF/encryption.xml` or `rights.xml` (store/DRM) are skipped.
- `container.xml` → OPF → spine; each XHTML document is stripped to paragraphs (Kobo's `koboSpan` wrappers vanish, HTML entities are decoded) and split into sentences. Matches are whole-word and case-insensitive on the form that was looked up. Footnote markers like `[39]` are removed, and long sentences are trimmed to about 320 characters around the word.
- **Ranking:** Kobo doesn't record where a word was looked up, so the nearest highlight in time (within 3 days) in the same book gives the likely chapter. Its `ContentID` maps to a zip entry: `…epub!OEBPS!ch09.xhtml` → `OEBPS/ch09.xhtml`. Candidates are sorted by spine distance from that chapter, and up to 5 are kept.
- Candidates are stored per sighting (`vocab_sighting.context_candidates`, JSON). The first becomes the context unless the user has already chosen one, and the user can pick another in the word dialog. The book text itself is never stored.
- Verified on the device: all 12 words resolved in about 0.15 s total, e.g. *soteriology* → "His soteriology is escapist."

**Definitions** (offline, no network permission):
- Kobo's own dictionaries are encrypted (§13), so they aren't used.
- Every source is converted once into one SQLite format (`dict/`): `entry(key, headword, pos, gloss, example, rank)` plus `form(form, key)` for irregular forms, where `key` is lowercase with accents removed.
- **Bundled:** Open English WordNet 2025 (CC BY 4.0). `scripts/fetch-wordnet.sh` downloads it, checks its sha256 and builds `data/dictionaries/oewn-2025.db` (185k senses, 24 MB, 0.7 s). Packages install it to `<prefix>/share/kollate/dictionaries/`, and it's credited in About.
- **User-added:** in Preferences → Dictionaries, users can add StarDict (`.ifo` + `.idx` + `.dict[.dz]`) or kaikki.org Wiktionary JSONL. These are stored in `<library dir>/dictionaries/`, searched first, and can be removed.
- **Search order:** user dictionaries, then `$KOLLATE_DATA_DIR/dictionaries`, then `$XDG_DATA_DIRS/kollate/dictionaries`. Debug builds also search the repo's `data/`.
- Only words without a definition are looked up, so the user's own edits are never overwritten. "Look Up Again" clears a definition and fetches it again.
- On the fixture words, WordNet defines 11 of 12. *hierophant* isn't in WordNet; a Wiktionary import covers it.

**Lemmas and merging:** a rule-based English lemmatizer proposes candidates (`theophanies` → `theophany`, `debouched` → `debouch`, `stopped` → `stop`). The dictionary's headwords and irregular forms decide which one is right. Words sharing a lemma are merged into the earliest one: sightings, tags and every lookup key (`vocab_form`) move over, and the most advanced learning status wins. Because the old forms stay recorded, a re-import doesn't split them apart again.

---

## 9. UI (libadwaita)

**Main window: `AdwNavigationSplitView`**

Sidebar:
- 📥 **Inbox** (new since last review, with a count badge)
- 📚 **Books**: one entry that opens a page of covers with title, author and highlight/word counts, sorted by most recent activity (or title or author; saved in `setting.books_sort`), with search. Only books with an Inbox or kept highlight, or a word that isn't Ignored, are listed (`Library::shelf`). A Back button (Alt+←) returns from a book to the grid.
- 🕘 **Recent**: the 5 most recently annotated books, as sidebar rows
- ✏️ **All Highlights** · 🗒 **Notes only** · ✍️ **Markups**
- 🔤 **Vocabulary**
- ⭐ **Starred** · 🏷 **Tags** (user tags) · 🗄 **Archive** · 🗑 **Trash**

Content pane:
- **Highlight cards:** a colour bar in the Kobo colour, the quote, the note underneath, and the chapter plus date. Hover actions: star, tag, edit, archive, copy, and "copy as Markdown quote". Multi-select for bulk tag, archive or export.
- **Book detail:** a header with cover and metadata, then highlights grouped by chapter in reading order (`VolumeIndex`, `ChapterProgress`, `StartOffset`).
- **Vocabulary:** a `GtkColumnView` table with columns for word, book(s), date, definition, context and status. Selecting a row opens a detail panel where you pick a context, edit the definition or change the status.
- **Edit:** inline editing of the note and a "corrected text" field. The device original stays visible and can be restored.
- **Merge highlights** (not built yet): Kobo splits highlights that cross page or element boundaries, so you'd select adjacent ones and merge them.
- **Search:** Ctrl+F searches the current view (text, note, chapter, book, author, tags) with an escaped `LIKE`, which is plenty for a personal library. FTS5 and filter chips for colour and date come later if needed.
- **Keyboard triage** on the selected card: `K` keep, `A` archive, `S` star, `E`/`Enter`/double-click edit, `I` back to Inbox, `Delete` trash, `↑`/`↓` move. Status changes show an Undo toast. **Starring an Inbox item also keeps it** (one Undo reverts both).
- **Multi-select:** the list allows multiple selection (Ctrl/Shift-click, Shift+arrows, Ctrl+A; Escape goes back to one). With several highlights selected, the triage keys act on all of them, and a bottom action bar shows "N selected" with Keep, Archive, Star and Trash. **Keep All** in the Inbox header keeps everything listed, respecting the search. Bulk actions reload the list and offer one Undo for everything.
- **Search** is a permanent field at the top of every page (Ctrl+F focuses it, Escape clears it). Its placeholder says what it searches: highlights, books (title/author) or words (word, definition, context).

**Preferences:** device-detection behaviour, definition sources, export targets, colour names (for example "blue = definitions").

---

## 10. Export

All exports read the library only. They're in `kollate-core/src/export/`, available in the app (Export dialog, Ctrl+E, or the sidebar button) and from `kollate-cli export …`.

| Format | What you get | Notes |
|---|---|---|
| **Obsidian vault sync** | One note per book in a folder you choose, plus `Vocabulary.md` | YAML front matter (title, author, isbn, publisher, series, `tags: [book, kobo]`, `kollate_id`), then chapter headings and highlights as `>` quotes with a block ID (`^k<id>`) so you can link to them. Notes appear as plain paragraphs, followed by a `<small>` line with the local date, colour (when not yellow), ★ and `#tags`. Each book's words are listed under `## Vocabulary` with the definition and the context sentence, the word in bold. Markup page images go to `attachments/` and are embedded. `Vocabulary.md` lists all words with `[[links]]` back to their book notes. **Idempotent:** a note is rewritten only when its content changes. Everything from the `%% kollate:user` line down is kept. Notes are found again by `kollate_id`, so a renamed book moves its note. Optionally syncs after every import. |
| **Anki (.apkg)** | The `Kollate::Vocabulary` deck, plus an optional `Kollate::Highlights` deck | Note type "Kollate Vocab" has fields Word, Lemma, Definition, Context, ContextBlank and Book, and two cards: **Recognition** (word and context → meaning) and **Fill In** (context with a blank, definition as a hint → word; only when a context exists). "Kollate Highlight" has one Review card. Deck IDs, note-type IDs and note GUIDs are stable. **Verified with Anki 26.09's importer:** the first import added 66 notes and 77 cards. Re-importing a new export updated all 66, added none, and kept study progress. Anki's integrity check passes. Known words are left out unless you opt in; ignored words are always left out. |
| **JSON backup** | Everything, including trashed highlights and ignored words | `{"format": "kollate-backup", "version": 1, "books": [...]}`. Restore is future work. |
| **CSV** | Highlights, or vocabulary | UTF-8, one row per highlight, or one per word and book. |
| **Readwise CSV** | Highlight, Title, Author, URL, Note, Location, Date | Tags go in the note as Readwise inline tags (`.tag`). |

Options (stored in the `setting` table): include archived highlights, include known words, a highlights deck for Anki, the Obsidian folder, and sync after import. Settings keys replace the `export_target`/`export_item` tables planned earlier: Obsidian sync compares content and Anki matches notes by GUID, so neither needs per-item export tracking.

Later: user-editable Markdown templates (minijinja), and a "since last export" option for CSV.

---

## 11. Milestones

1. ✅ **M0, core + CLI:** Kobo reader, normalization, chapter resolution and `kollate-cli inspect <mount|db>`. Tests run against the fixture DB here: 52 bookmarks, 12 words, 5 books.
2. ✅ **M1, store + dedup:** the schema, importer and merge rules. Tests cover re-importing the same DB (0 changes), an edited note, a deleted row and a factory reset (new IDs, same text).
3. ✅ **M2, UI browse & curate:** books, highlights, notes, search, star/tag/archive, edit.
4. ✅ **M3, device integration:** autodetect, auto-import, Inbox, toasts, eject, markup files, covers.
5. ✅ **M4, vocab enrichment:** EPUB context extraction, WordNet bundle and dictionary import, lemma merge.
6. ✅ **M5, export:** Obsidian vault sync and Anki, then JSON, CSV and Readwise.
7. **M6, polish & ship:**
   - ✅ `.deb` via cargo-deb (`scripts/build-deb.sh` → `target/debian/kollate_<ver>_amd64.deb`). It contains `kollate`, `kollate-cli`, the desktop entry, AppStream metainfo, a scalable icon, the WordNet dictionary under `/usr/share/kollate/dictionaries/`, the GPL-3 copyright file and a WordNet notice. Dependencies come from shlibdeps. App ID: `io.github.andrew_lawlor.Kollate`.
   - ✅ Flatpak: `flatpak/io.github.andrew_lawlor.Kollate.yml` on GNOME 51 with rust-stable//26.08. The build is offline, using pinned sources in `flatpak/cargo-sources.json`, and WordNet is downloaded by sha256 and converted at build time. Permissions: wayland, fallback-x11, dri, ipc, `/media:ro`, `/run/media:ro`, `org.gtk.vfs.*`. **No network.** `scripts/build-flatpak.sh` installs the app for the user and writes `target/flatpak/kollate.flatpak`. Verified with the Kobo connected: it autodetected the device through gvfs, imported 56 highlights, found 12 contexts, defined 10 of 11 words, and copied 6 covers and 1 markup image.
   - ✅ README with screenshots. Public repo at https://github.com/andrew-lawlor/kollate.
   - ✅ Eject works from inside the Flatpak sandbox (confirmed by the user on the Libra Colour).
   - Still to do: markup SVG rendering, and a Flathub submission (which needs a git source in place of `type: dir`).

---

## 12. Decisions log
- Stack: Rust + GTK4/libadwaita.
- Exports: Obsidian and Anki are the priority; Readwise is included because it's cheap.
- Never write to the Kobo.
- Autodetect the reader, and pull vocab context while it's plugged in.
- Offline only; the app requests no network permission.
- Device: Kobo Libra Colour (colour highlights, stylus markups).
- App ID `io.github.andrew_lawlor.Kollate`. License GPL-3.0-or-later. Maintainer Andrew Lawlor <andrew@lawlor.io>.
- The .deb is packaged before the Flatpak.

## 13. Verified on the device (2026-09-25, Libra Colour, firmware 4.45.23697)
- `.kobo/version` = `N000000000000,4.9.77,4.45.23697,4.9.77,4.9.77,00000000-0000-0000-0000-000000000390`, i.e. serial, ?, firmware, ?, ?, model ID (`…0390` = Libra Colour). The parser matches.
- Markups: `.kobo/markups/<BookmarkID>.svg` holds **only the ink strokes** (Qt SVG, page-sized viewBox 1264×1680). `.jpg` is the rendered page with the ink on it. Import both; the UI shows the JPG.
- Dictionaries: `.kobo/dict/dicthtml.zip` (English, **no `-en` suffix**) plus `dicthtml-en-zh-CN.zip` / `-zh-TW`. Inside: `words` / `prefix_exceptions` are marisa tries, and the `*.html` shards are **encrypted** (not gzip). **Decision: we don't use Kobo's dictionaries** (see §8).
- `Exported Annotations/` and `Exported Notebooks/` exist (Kobo's own export feature) and are empty. Ignore them.
- `driveinfo.calibre` is present, so the user manages books with calibre. Calibre may rename or re-send books, which the book fingerprint (§6) handles.
- Colour index: 0 yellow, 1 pink, 2 blue, 3 green (verified with test highlights in *Free Software, Free Society*).
