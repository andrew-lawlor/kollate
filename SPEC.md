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
| UI | `gtk4`, `libadwaita`, plus `gio` / `glib` for the mount monitor and async main loop. UI is written with composite templates in GtkBuilder `.ui` XML, compiled into GResources. |
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
  crates/kollate/         # GTK4/libadwaita app (src/ + data/ui/*.ui, resources)
  tests/fixtures/KoboReader.sqlite
```

**Reading the device safely**
1. Detect the mount (§5), then copy `.kobo/KoboReader.sqlite` and any `-wal`/`-shm` files to a temp dir.
2. Open the copy with `?mode=ro`. Never hold a handle on the device, because that blocks a clean eject.
3. Treat `ExtraAnnotationData` and any unexpected column as bytes. Check `DbVersion` and warn (don't fail) on unknown schema versions.

---

## 5. Device detection & import flow

- **Autodetect** by subscribing to `gio::VolumeMonitor` `mount-added` / `mount-removed`. On startup, also scan existing mounts. A mount counts as a Kobo if it contains `.kobo/KoboReader.sqlite`. There's no hard-coded path; `/media/andrew/KOBOeReader` is just the typical one. A manual "Import from folder/file…" option handles DB backups.
- On detection there are three behaviours, set in Preferences: **Auto-import** (default), **Ask**, or **Ignore**.
- **Import pipeline while the reader is mounted:**
  1. Copy the DB, then diff and merge highlights, notes and vocab (§6).
  2. Copy markup `.svg`/`.jpg` files for new markups.
  3. **Vocab context pass (§8):** for every word without a context sentence, open the book's EPUB on the device and extract candidate sentences. This runs in the background, with progress shown in the header bar.
  4. Look up definitions offline for new words (§8).
  5. Cache covers from `.kobo-images/` for new books.
- Steps 2–5 are best-effort. If the reader is unplugged mid-way, the unfinished work is queued and resumes on the next connect.
- After an import, a toast reads "Kobo Libra Colour: 7 new highlights, 2 new words · Review". Clicking it opens the **Inbox**.
- Every import writes an `import_runs` row (device, time, counts: new, updated, unchanged, removed-on-device) for traceability.
- The app shows an "Eject" button once the import finishes (via `gio::Mount::unmount_with_operation`).

---

## 6. Deduplication & merge rules

### 6.1 Identity keys
| Entity | Primary key | Fallback fingerprint (catches factory reset, re-sideloaded books, a second device) |
|---|---|---|
| Book | `(device_id, VolumeID)` mapped to a `book_id` | `sha1(normalize(title) + normalize(author))`, plus ISBN if present |
| Annotation | `BookmarkID` (a globally unique UUID) | `sha1(book_fingerprint + normalize(text) + start_path + start_offset)`. For markups: `BookmarkID` only. |
| Vocab word | `(normalize(word), language)` | n/a. Each device/book occurrence becomes a `vocab_sighting` row. |

`normalize(text)` applies NFC, collapses whitespace, trims, and unifies curly/straight quotes. Book paths change when files are renamed, which is why the fingerprint exists.

### 6.2 Merge logic (per annotation)
- **Not in store:** insert it with `status=inbox`.
- **In store, device `DateModified` unchanged:** skip it.
- **In store, device changed (note edited, colour changed):** update the *device fields*. If the user has already edited that field in Kollate, keep the Kollate value, store the device value in `annotation_revisions`, and flag it as "device changed". The user can accept the device version from the UI.
- **In store but missing from the device:** set `removed_on_device_at`. Nothing is ever deleted automatically.
- **Hidden on device:** handled the same as removed.

To support this, every annotation keeps the `device_*` fields (original) separate from `user_*` overrides (curated text, note), and the UI shows `coalesce(user, device)`.

---

## 7. Local data model (`~/.local/share/kollate/library.db`)

```
device(id, serial, model, name, last_seen_at)
book(id, fingerprint UNIQUE, title, author, isbn, publisher, language, series, series_no,
     cover_path, percent_read, last_read_at, user_title, user_author, hidden)
book_source(book_id, device_id, volume_id, UNIQUE(device_id, volume_id))
annotation(id, bookmark_id UNIQUE, fingerprint, book_id, kind{highlight,note,markup},
     device_text, device_note, color, chapter_title, chapter_file, start_path, start_offset,
     chapter_progress, created_at, device_modified_at,
     user_text, user_note, starred, status{inbox,kept,archived,trashed},
     removed_on_device_at, markup_svg_path, markup_jpg_path, imported_at, updated_at)
annotation_revision(id, annotation_id, field, old_value, new_value, source{device,user}, at)
vocab(id, word, lemma, language, definition, definition_source, status{new,learning,known,ignored},
     starred, user_note, first_seen_at, UNIQUE(lemma, language))
vocab_sighting(id, vocab_id, book_id, device_id, surface_form, looked_up_at, context_sentence,
     UNIQUE(vocab_id, book_id, surface_form))
tag(id, name UNIQUE, color)   annotation_tag(...)   vocab_tag(...)
import_run(id, device_id, started_at, finished_at, stats_json, db_version)
export_target(id, name, format, path, options_json, last_exported_at)
export_item(export_target_id, item_type, item_id, content_hash, exported_at)  -- incremental exports
fts: annotation_fts(text, note, book title, author), vocab_fts(word, definition, context)
```
Schema versioning via `PRAGMA user_version` with forward-only migrations.

---

## 8. Vocab enrichment (offline only, while the reader is plugged in)

**Context sentences** come from the book on the device:
- For sideloaded, DRM-free books (`VolumeID` = `file:///mnt/onboard/...`), map `/mnt/onboard/` to the mount point, open the `.kepub.epub` zip, and walk the spine in order. Strip XHTML to text (Kobo's `span.koboSpan` wrappers are dropped), split into sentences and find whole-word, case-insensitive matches of the surface form.
- **Picking the right sentence:** Kobo doesn't store where a word was looked up, so we narrow the candidates. We prefer chapters around the book's reading position at lookup time, estimated from highlights made near that timestamp and from `content.ChapterIDBookmarked`. We keep up to 5 candidates ranked by proximity, and the UI lets the user choose one (the first is the default).
- Store books (encrypted) get no automatic context, and the user can paste one in.
- The book text is **not** cached; only the chosen and candidate sentences are stored.

**Definitions** (offline, no network permission). Kobo's own dictionaries are encrypted (§13), so we don't use them. Instead:
1. **Bundled: Open English WordNet** (CC BY 4.0), shipped as a compact SQLite file of about 15–25 MB. It covers most literary vocabulary, and it's the default for English.
2. **User-imported:** StarDict dictionaries or a kaikki.org Wiktionary JSONL extract that the user downloads themselves and imports via a file picker (converted once into SQLite). This gives richer definitions and other languages, and the app still never touches the network.
3. **Manual:** the user can edit any definition.
Definitions are looked up at import and cached in the library DB.

**Lemma merge:** a rule-based English lemmatizer (`theophanies` → `theophany`, `daimons` → `daimon`), which also checks the dictionary's headwords for the lemma. Users can split or merge words.

---

## 9. UI (libadwaita)

**Main window: `AdwNavigationSplitView`**

Sidebar:
- 📥 **Inbox** (new since last review, with a count badge)
- 📚 **Books**, a list with cover, title, author and highlight/word counts
- ✏️ **All Highlights** · 🗒 **Notes only** · ✍️ **Markups**
- 🔤 **Vocabulary**
- ⭐ **Starred** · 🏷 **Tags** (user tags) · 🗄 **Archive** · 🗑 **Trash**

Content pane:
- **Highlight cards:** a colour bar in the Kobo colour, the quote, the note underneath, and the chapter plus date. Hover actions: star, tag, edit, archive, copy, and "copy as Markdown quote". Multi-select for bulk tag, archive or export.
- **Book detail:** a header with cover and metadata, then highlights grouped by chapter in reading order (`VolumeIndex`, `ChapterProgress`, `StartOffset`).
- **Vocabulary:** a `GtkColumnView` table with columns for word, book(s), date, definition, context and status. Selecting a row opens a detail panel where you pick a context, edit the definition or change the status.
- **Edit:** inline editing of the note and a "corrected text" field. The device original stays visible and can be restored.
- **Merge highlights:** Kobo splits highlights that cross page or element boundaries, so you can select adjacent ones and merge them.
- **Search:** global (Ctrl+F) over FTS, with filter chips for book, colour, type, tag, date range and status.
- **Keyboard triage in Inbox:** `K` keep, `A` archive, `S` star, `T` tag, `J`/`↓` next.

**Preferences:** device-detection behaviour, definition sources, export targets, colour names (for example "blue = definitions").

---

## 10. Export

Priority: **Obsidian and Anki first**, then the cheap formats (JSON, CSV, Readwise).

| Format | Scope | Notes |
|---|---|---|
| **Obsidian vault sync** | A folder in the vault (e.g. `Books/Kobo/`) | One note per book: YAML front matter (title, author, isbn, series, tags, `kollate_id`), then highlights grouped by chapter as `>` quotes with the note, a colour label and a `^block-id` (so the user can link to individual highlights). Vocabulary gets its own note per book or one global `Vocabulary.md`. Stylus markups are copied into an attachments folder and embedded as `![[...svg]]`. Templates use `minijinja` and are user-editable. Exports are idempotent: only changed books are rewritten, and anything below the `%% kollate:user %%` marker is preserved, so the user can add their own thoughts. |
| **Anki (.apkg)** | Vocab (primary), highlights (optional) | A custom note type, "Kollate Vocab", with fields for word, lemma, definition, context (the word **bolded**), a cloze version of the context, book and author. Two card templates: recognition (word → meaning) and cloze. A stable deck ID and stable note GUIDs (from `vocab.id`), so re-importing into Anki updates the cards instead of duplicating them. Words marked "known" or "ignored" are excluded by default. |
| **JSON** | Everything | A full dump plus a schema version, which doubles as the backup/restore format. |
| **CSV** | Highlights / vocab | Flat columns, UTF‑8. |
| **Readwise CSV** | Highlights | Readwise's documented import columns (Highlight, Title, Author, Note, Location, Date). Cheap to add, low priority. |

Export dialog options: scope (selection, book, filter, everything), include archived yes/no, **only items new since the last export to this target**, and a preview of the first item. Obsidian and Anki targets can be saved and re-run with one click, and can optionally run automatically after each import.

---

## 11. Milestones

1. ✅ **M0, core + CLI:** Kobo reader, normalization, chapter resolution and `kollate-cli inspect <mount|db>`. Tests run against the fixture DB here: 52 bookmarks, 12 words, 5 books.
2. **M1, store + dedup:** the schema, importer and merge rules. Tests cover re-importing the same DB (0 changes), an edited note, a deleted row and a factory reset (new IDs, same text).
3. **M2, UI browse & curate:** books, highlights, notes, search, star/tag/archive, edit.
4. **M3, device integration:** autodetect, auto-import, Inbox, toasts, eject, markup files, covers.
5. **M4, vocab enrichment:** EPUB context extraction, WordNet bundle and dictionary import, lemma merge.
6. **M5, export:** Obsidian vault sync and Anki, then JSON, CSV and Readwise.
7. **M6, polish & ship:** markup SVG rendering, Flatpak (no network), app icon, `.desktop` file.

---

## 12. Decisions log
- Stack: Rust + GTK4/libadwaita.
- Exports: Obsidian and Anki are the priority; Readwise is included because it's cheap.
- Never write to the Kobo.
- Autodetect the reader, and pull vocab context while it's plugged in.
- Offline only; the app requests no network permission.
- Device: Kobo Libra Colour (colour highlights, stylus markups).

## 13. Verified on the device (2026-09-25, Libra Colour, firmware 4.45.23697)
- `.kobo/version` = `N000000000000,4.9.77,4.45.23697,4.9.77,4.9.77,00000000-0000-0000-0000-000000000390`, i.e. serial, ?, firmware, ?, ?, model ID (`…0390` = Libra Colour). The parser matches.
- Markups: `.kobo/markups/<BookmarkID>.svg` holds **only the ink strokes** (Qt SVG, page-sized viewBox 1264×1680). `.jpg` is the rendered page with the ink on it. Import both; the UI shows the JPG.
- Dictionaries: `.kobo/dict/dicthtml.zip` (English, **no `-en` suffix**) plus `dicthtml-en-zh-CN.zip` / `-zh-TW`. Inside: `words` / `prefix_exceptions` are marisa tries, and the `*.html` shards are **encrypted** (not gzip). **Decision: we don't use Kobo's dictionaries** (see §8).
- `Exported Annotations/` and `Exported Notebooks/` exist (Kobo's own export feature) and are empty. Ignore them.
- `driveinfo.calibre` is present, so the user manages books with calibre. Calibre may rename or re-send books, which the book fingerprint (§6) handles.
- Colour index: 0 yellow, 1 pink, 2 blue, 3 green (verified with test highlights in *Free Software, Free Society*).
