# Kollate — Spec

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

**Markups:** the drawing isn't in the DB. It lives on the device at `.kobo/markups/<BookmarkID>.svg` (plus a `.jpg` page snapshot). Verified on the device (§13). Import copies both files into the library.

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

The minimum is GTK 4.12 and libadwaita 1.5 (Ubuntu 24.04, the .deb's oldest platform), so the app enables only the `v4_12` / `v1_5` features (§12).

| Concern | Crate |
|---|---|
| UI | `gtk4` 0.11 (`v4_12`), `libadwaita` 0.9 (`v1_5`), plus `gio` / `glib` for the mount monitor and async main loop. Widgets are built in Rust code (no `.ui` templates); styles live in `style.css`. |
| SQLite (device + library) | `rusqlite` with the `bundled` feature (plus FTS5), so there's no system sqlite dependency. |
| EPUB (context extraction) | `zip`, plus `quick-xml` to strip XHTML to text; sentence splitting with `unicode-segmentation`. |
| Dictionaries (offline) | Bundled English Wiktionary (reader.dict DictFile, converted to SQLite), plus importers for DictFile, StarDict, kaikki.org JSONL and WordNet LMF. |
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
    dict/               # dictionary format, importers (DictFile, StarDict, kaikki, WordNet), lemmatizer
    import.rs           # diff + merge device data into the store
    export/             # markdown(+vault), anki, csv, json, readwise
  crates/kollate-cli/     # `kollate-cli import <mount|db> [--dry-run]`, `export …`
  crates/kollate/         # GTK4/libadwaita app: window/ (mod: setup and actions; sidebar, content,
                          #   annotations, device, preferences, exports), card.rs, edit.rs, word.rs, style.css
  tests/fixtures/KoboReader.sqlite
```

**Reading the device safely**
1. Detect the mount (§5), then copy `.kobo/KoboReader.sqlite` and any `-wal`/`-shm` files to a temp dir.
2. Open the copy with `?mode=ro`. Never hold a handle on the device, because that blocks a clean eject.
3. Treat `ExtraAnnotationData` and any unexpected column as bytes. Check `DbVersion` and warn (don't fail) on unknown schema versions: `TESTED_DB_VERSIONS` lists the verified ones (174 on the Clara 2E and Elipsa 2E, 176 on the Libra Colour; §13). Any other version is still read; the CLI prints a warning, and the app shows a one-time toast per version with a **Report** button that opens a pre-filled GitHub compatibility issue. If reading an untested version fails, the error is `Error::UntestedDb`, which names the version, so the cause is clear.


**Flatpak document-portal paths:** locations picked in a file chooser arrive as `/run/user/<uid>/doc/<id>/<name>/…`. `portal.rs` resolves them to the real location with `org.freedesktop.portal.Documents.GetHostPaths`. The Export dialog uses that to show the folder as a person would recognise it (`~/Notes/Reading`), and the Kobo check uses it too, since the `.kobo` folder above a chosen subfolder isn't visible through a portal path.

**Never writing to the device** is enforced in the core. `kobo::ensure_not_on_kobo(path)` rejects any path on a Kobo: the path or its nearest existing ancestor is resolved through symlinks and checked for `.kobo/KoboReader.sqlite`. It's called before every write: `Library::open`, every export (Obsidian, Anki, JSON, CSV, Readwise), `DictionaryBuilder::create`, `copy_assets`, and even the temp folder for the database copy. The app also rejects a Kobo folder as the Obsidian target when you pick it. The error is `Error::OnKobo`, and a test confirms a fake Kobo stays byte-for-byte unchanged. Remaining caveat: on the `.deb`, Linux may update FAT last-access dates when files are read; the Flatpak's read-only access prevents that.

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
- Book pages open with a header showing the cover, % read and last-read date. Markup cards show the page with the ink drawn on it, and "Open Page Image" opens that in the default viewer.

---

## 6. Deduplication & merge rules

### 6.1 Identity keys
| Entity | Primary key | Fallback fingerprint (catches factory reset, re-sideloaded books, a second device) |
|---|---|---|
| Book | `(device_id, VolumeID)` mapped to a `book_id` | `blake3(normalize(title) + normalize(author))`. ISBN is stored but deliberately not part of the fingerprint: most sideloaded books have none (51 of 168 in the sample), and a store copy and a sideloaded copy of the same book often differ, so it would split one book in two. |
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
- A sideloaded, DRM-free book's `VolumeID` (`file:///mnt/onboard/...`) maps to its file under the mount. Books with `rights.xml`, or an `encryption.xml` that encrypts anything but fonts, are DRM-protected and skipped; an `encryption.xml` listing only fonts (the EPUB standard's font obfuscation, 14 of 96 books on the Clara 2E) leaves the text readable. A book that has moved since (calibre re-sending it into another folder, while `WordList` keeps the old path) is found by file name.
- `container.xml` → OPF → spine; each XHTML document is stripped to paragraphs (Kobo's `koboSpan` wrappers vanish, HTML entities are decoded) and split into sentences. Matches are whole-word and case-insensitive on the form that was looked up. Footnote markers like `[39]` are removed, and long sentences are trimmed to about 320 characters around the word.
- **Ranking:** Kobo doesn't record where a word was looked up, so the nearest highlight in time (within 3 days) in the same book gives the likely chapter. Its `ContentID` maps to a zip entry: `…epub!OEBPS!ch09.xhtml` → `OEBPS/ch09.xhtml`. Candidates are sorted by spine distance from that chapter, and up to 5 are kept.
- Candidates are stored per sighting (`vocab_sighting.context_candidates`, JSON). The first becomes the context unless the user has already chosen one, and the user can pick another in the word dialog. The book text itself is never stored.
- Verified on the device: all 12 words resolved in about 0.15 s total, e.g. *soteriology* → "His soteriology is escapist."

**Definitions** (offline, no network permission):
- Kobo's own dictionaries are encrypted (§13), so they aren't used.
- Every source is converted once into one SQLite format (`dict/`): `entry(key, headword, pos, gloss, example, rank)` plus `form(form, key)` for irregular forms, where `key` is lowercase with accents removed.
- **Bundled:** the English Wiktionary as compiled by reader.dict (CC BY-SA 4.0), in DictFile format. It replaced Open English WordNet on 2026-09-26: it has far better coverage (819k headwords, 1.21M senses, 613k inflected forms) and better senses (e.g. *hierophant*, and the Greek sense of *daimon*). The upstream URL isn't versioned, so the exact file is mirrored on Kollate's `data-wiktionary-en-2026-09-09` release and pinned by sha256. `scripts/fetch-dictionary.sh` builds `data/dictionaries/wiktionary-en.db` (180 MB, about 9 s; roughly 55 MB compressed in the `.deb`). Packages install it to `<prefix>/share/kollate/dictionaries/`. Credits are in About, THIRD-PARTY.md and the Preferences row.
- **DictFile import** (`dict/dictfile.rs`): `@` headword, `:` pronunciation, `&` inflected form, then HTML with one `<p><b>Part of speech</b></p><ol>` block per part of speech. Sub-senses in nested lists are kept, including lists that follow their parent `</li>`. A parent ending mid-sentence ("Synonym of demon, particularly as") prefixes its sub-senses. Synonym lists, quotations, usage notes, etymology and other non-part-of-speech sections are skipped.
- **Changing dictionaries:** when the set of installed dictionaries changes (`setting.dictionaries`), every definition that came from a dictionary is looked up again (`refresh_definitions`). Definitions the user wrote are never touched.
- **Ambiguous inflections:** when an inflected form is listed under several headwords, lookup prefers the base the inflection rules also produce, then the shortest.
- **User-added:** in Preferences → Dictionaries, users can add DictFile (`.df`, `.df.bz2`; reader.dict's `dict-<lang>-<lang>` files get language `<lang>`), StarDict (`.ifo` + `.idx` + `.dict[.dz]`) or kaikki.org Wiktionary JSONL. A "Dictionaries for Other Languages" row links to reader.dict with credit. Preferences lists dictionaries grouped by language, in lookup order. A word is only looked up in dictionaries for its language, and dictionaries the user added are tried before the included one; there is no manual ordering. These are stored in `<library dir>/dictionaries/`, searched first, and can be removed.
- **Search order:** user dictionaries, then `$KOLLATE_DATA_DIR/dictionaries`, then `$XDG_DATA_DIRS/kollate/dictionaries`. Debug builds also search the repo's `data/`.
- Only words without a definition are looked up, so the user's own edits are never overwritten. "Look Up Again" clears a definition and fetches it again.
- On the 12 fixture words, Wiktionary defines all 12; WordNet missed *hierophant*.

**Lemmas and merging:** a rule-based English lemmatizer proposes candidates (`theophanies` → `theophany`, `debouched` → `debouch`, `stopped` → `stop`). The dictionary's headwords and irregular forms decide which one is right. Words sharing a lemma are merged into the earliest one: sightings, tags and every lookup key (`vocab_form`) move over, and the most advanced learning status wins. Because the old forms stay recorded, a re-import doesn't split them apart again.

## 8a. Handwriting transcription

Turns stylus markups into text: what the reader wrote, and which printed words they underlined or circled. **Local only.** A cloud model was considered and rejected: sending pages of the reader's books and their handwriting to a server contradicts the offline promise. Evaluated 2026-09-26 on four real markups from a Libra Colour (8 handwritten notes, 3 marks); the harness and results are summarised below.

**Pipeline.** Only step 4 uses a model to read handwriting; everything else is geometry or the book itself.
1. **Split the ink** (no model). Each `<path>` in the markup SVG is one pen stroke, in writing order. A stroke at least 150 px wide and at most 0.12 as tall is an **underline** (consecutive ones over adjacent lines are one mark). A large stroke (≥100×60 px) is a **circle**; if other strokes' centres fall inside it, it encloses handwriting and belongs to that note, otherwise it marks printed text. Remaining strokes are grouped into **notes** when their boxes come within 0.8× the median letter height of each other vertically, or 1.6× sideways; specks under 12 px are dropped. Writing down the margin has its word gaps the other way, so it falls apart into pieces a word or two long; the strokes are grouped a second time with the reaches swapped, letters measured by their width (a sideways letter's height), and pieces are joined where that makes a long, narrow note (at least six strokes and five letters long) of pieces that aren't lines across the page. That keeps a two-column note down the margin whole, and leaves a question mark and its dot alone. Last, pieces of one line (each under four letters tall, mostly level with each other) join when they're within three letters sideways, for a hand with wide word gaps (on the Elipsa 2E, "This is a much longer note" came apart around the lone "a", read as "1"); a piece about a letter wide joins only with writing on both sides, so a star or `?` at either end of a line stays its own note. On the 47-page evaluation this changed no score, and one page improved ("Marseilles" found circled). On the samples this found 8/8 notes and 3/3 marks, including a circled word and a two-line underline.
2. **Upright sideways notes** (no model). A group more than 1.8× taller than wide is sideways; the pen's direction (first stroke's position vs last) says which way to rotate. Choosing by dictionary words instead picked wrong readings ("Havens", "Suzanne H").
3. **Marked text** (small model + the book). Printed lines are found on the Kobo's page JPG, which has no ink, from the row profile of dark pixels (bands 8–70 px tall; illustrations are taller). An underline stroke marks the line whose bottom is nearest above its centre; a circle marks the lines whose middles it encloses, cut to the printed words lying mostly inside it (words are told apart by gaps wider than a fifth of the line's height, and the crop never reaches past the circle, since a word joined to the next by a dash, "Marseilles—The", has no gap), so the edges of neighbouring words it takes in ("e Latian r") aren't read as words of their own. Each marked span is cropped and read, then **snapped to the book**: the best-matching run of words in the chapter XHTML around the markup's `StartContainerPath`/`EndContainerPath`. The stored text is always the book's, which fixes clipped words ("e testimony…") and restores typography (’). 3/3 exact with Qwen3-VL 2B, under a second each.
4. **Read each note** (model). Each note is rendered alone (black ink on white, enclosing circle removed), rotated if needed, and transcribed with the prompt "Transcribe the handwritten text in this image exactly. Output only the text." Output is constrained by a GBNF grammar to Latin script (U+0020–007E, U+00A0–024F, U+2010–2027, newline), which stopped small models from answering in Cyrillic.
5. **Correct misread names** (no model). A word in a note that no dictionary knows is replaced by a close name (≥ 0.75 similarity, five letters or more) from the book's words around the markup: a word the book capitalises mid-sentence, or anywhere if it isn't a dictionary word itself. A second writer's "Who is Polemarchus?" came back from every model as "Pokmarchus"; this fixes it, while "Athena" beside "Athens" is left alone.
6. **Store as guesses.** `annotation.ink_text`/`ink_note` (migration 7) hold the reading, `ink_source` the model and `ink_hash` what it was made from (ink, page, book words, model), so a changed markup or a different model reads it again. `text()`/`note()` show the Kobo's, else the reading, and the user's edits always win; the edit dialog can restore the reading. Readings count as notes in the Notes view and search. Cards show the ink, then the marked text and the note, captioned "Read from handwriting by …" until edited; Obsidian notes embed the ink and quote the marked text.

**Code.** `kollate-core::markup` (segment, image, snap; the model is a `Reader`), `store/markup.rs` (queue and storage), `epub::markup_words` (book words saved at import as `annotation.markup_context`), and the `kollate-transcribe` crate (llama.cpp `Reader`, the model catalog, adding and checking model files). The app runs pending markups after each import and at startup, one at a time on a worker thread with the model loaded once. The CLI has `models [add FILES…|remove ID]` and `transcribe [--model ID] [--cpu]`; its `import` now also copies images and saves context, like the app.

**Models.** Qwen3-VL (Apache 2.0) at three sizes, one prompt and code path for all:

| Choice | Files (model + vision) | Handwriting | Per note, GPU / CPU |
|---|---|---|---|
| **Recommended: Qwen3-VL 2B (Q8_0)** | 2.7 GB | 81% | 0.2 s / 1.5 s |
| Middle: Qwen3-VL 4B (Q4_K_M) | 3.3 GB | 85% | 0.3 s / 2.1 s |
| Best: Qwen3-VL 8B (Q4_K_M) | 6.2 GB | 86% | 0.4 s / 3.4 s |

The 4B was recommended until 0.2.2; in daily use the 2B read real notes and notebook pages as well (the difference is in the messiest notes), at the smallest size and speed, so it is recommended since. Handwriting is the mean character similarity to the reader's own reading of the notes; clear writing is exact with all three, and the messiest notes ("woa", "Yeah, swiping setting helps.") defeat every model. CPU times are a Ryzen 5 5600X; GPU is an RX 6750 XT via Vulkan. Also tried and not chosen: Qwen3-VL 4B at Q8_0 (86%, +1.8 GB); Gemma 3 12B (strong, but misread one note as a crude phrase); LFM2.5-VL 1.6B (79%; its own licence); Ministral 3 3B; and the OCR specialists Nanonets-OCR2 3B, PaddleOCR-VL 0.9B, LightOnOCR-2 1B and DeepSeek-OCR 3B (64–82%; trained for printed documents). Asking a model to do everything from the whole page works only at 8B and up; small models loop or invent marks, which is why the pipeline splits the job.

**Getting a model (no network permission).** Preferences lists the three choices, the 2B marked recommended, each with links to its two files (pinned to a repository revision) that download in the browser, plus **+** to add the downloaded files. Known files are recognised by size and BLAKE3 checksum, which confirms they're intact and names the choice; they're copied via a `.part` file so a half-copied model never looks installed. Models are stored in `<library dir>/models/` and can be removed. Later, each could ship as an optional Flatpak extension on Flathub, installable from GNOME Software, so the app itself still never touches the network. Transcription is off until a model is added.

**Processor.** llama.cpp's CPU code is built with ggml's defaults for non-native builds: AVX, AVX2, FMA, F16C and BMI2 (x86-64 processors from about 2013). `kollate_transcribe::supported()` checks this at runtime; without it the model is never loaded and Preferences says why, rather than crashing. The rest of Kollate is unaffected. Measured on the portable build: about 2.2 s per image read on a Ryzen 5 5600X's CPU, 0.3–0.5 s on the GPU.

**Markup pages are drawn by Kollate.** 0.1.6 composed the page and ink as an SVG with the page embedded; GNOME's newer image loaders (glycin, in the GNOME 51 runtime) placed the embedded page differently from librsvg, so in the Flatpak the page sat shifted under the ink. `markup::image::compose_page` now renders the ink with resvg onto the page (or its band) and writes a JPEG, identical everywhere.

**Readings that loop.** On ink it can't read (a sideways note cut into pieces, say), a small model can repeat itself ("+ 1 + 1 + 1…", "2222…") until it runs out of tokens. `markup::runaway_start` spots a run of at least five copies of a piece up to eight characters long, 16 characters or more in all. Generation stops there, and the loop is cut from the reading; if less than a word is left, the note is dropped.

**Running it.** In-process via `llama-cpp-2` (features `mtmd`, `vulkan`, `sampler`, `common`), verified in a spike built in the GNOME 51 SDK and run in the GNOME 51 runtime with only `--device=dri`: same output as `llama-server`, same speed. Build requirements: `org.freedesktop.Sdk.Extension.llvm22` (libclang for bindgen), a `SPIRV-Headers` module in the Flatpak manifest (not in the GNOME SDK), and on Ubuntu 24.04 CI `cmake`, `libclang-dev`, `glslc`, `spirv-headers` and `libvulkan-dev`. Cost: about 40 MB of binary (llama.cpp plus Vulkan shaders) and about 2 minutes of build time. Transcription runs in the background after import, one note at a time, on the GPU when Vulkan finds one and the CPU otherwise. mtmd's logging must be silenced.

**Open questions.** The evaluation is 9 notes and 3 marks from two writers: collect more samples (other hands, notebooks, other languages) before tuning further. Progress feedback during a long background run is only a toast at the start and end. Stylus notebooks: see §8b.

## 8b. Kobo notebooks

Stylus Kobos keep notebooks as `My Notebooks/<name>.nebo`, listed in `content` as `ContentType = 6` with `MimeType` `application/vnd.myscript.nebo+raw` (Basic) or `+text` (Advanced), with a cover thumbnail under `.kobo-images` like a book's. The Kobo turns handwriting into text only in Advanced notebooks, only on request (double-tap, **Convert all**), and exports Basic ones only as images. Kollate reads both kinds itself (verified 2026-09-29 on a Libra Colour, firmware 4.45, iink 2.0.6, and 2026-10-03 on an Elipsa 2E, firmware 4.38).

**Format** (undocumented; worked out from the device). A `.nebo` is a zip: `meta.json` (`format-version` `4.0`; page size in `iink-user-metadata.kobo.geometry`, resolution in `.dpi`: 300 on the Libra Colour, 228 on the Elipsa 2E, which millimetres are converted at), and per page `pages/<id>/meta.json` (`creationDate`, `lastModificationDate`), `ink.bink` (the strokes), `page.bdom` (iink's document, including its own recognition as candidate lattices, not read) and `style.css`. BINK, after a header declaring one or two stroke formats, holds strokes back to back (little-endian), with runs of `ff` bytes between some; the first is found by its pen field, `0x0c49` for every pen, brush and colour: a *plain* stroke (`u32 0`, `u64` time, `u32` pen, `u32 n`, `n` × `f32` x/y in mm, then `n` × `u32` pressure and time) or a *packed* one (`u32 0x80000000`, `u64` time, `f32` x/y of the start in mm, `u16`, `u16` pen, `u16`, `u32 n`, `n` × `i16` dx, `n` × `i16` dy in 2 µm steps, `n` bytes). Other data follows the strokes; parsing stops at the first thing that isn't a plausible stroke. Advanced pages scroll: ink can go well past the nominal page height. A notebook with another `format-version` is skipped (`unread_notebooks`), so a firmware change can't import garbage. Erasing removes strokes from the file. Pens (FeltPen, FountainPen, CalligraphicQuill, CalligraphicBrush), widths and colours are CSS in a style table after the strokes, assigned to ranges of iink's stroke IDs, which skip numbers where strokes were erased; Kollate doesn't decode that yet, so pages are drawn in black. The highlighter in an Advanced notebook leaves no strokes: it's a text decoration (`-myscript-text-decoration-background-color`). Verified with all four pens in three colours, erasing in both notebook kinds and an added page (2026-09-29); the same format on the Elipsa 2E (firmware 4.38, iink 2.0.6), a Basic and an Advanced notebook (2026-10-03).

**Import.** `kobo::notebook::add_notebook_pages` (needs the mounted Kobo) adds each notebook with ink as a book and each page as an annotation of kind `page` (migration 8 rebuilds `annotation` to allow it), with bookmark ID `<volume ID>#<page id>`, chapter "Page N" and pages ordered by creation. `copy_assets` writes the page's ink as a markup-style SVG (a filled outline per stroke) and a blank page JPEG under `notebooks/`, cropped to the ink, so pages display and export like markups. Pages of notebooks that weren't read (an import from a database file alone, or an unknown format) are never flagged as deleted on the Kobo.

**Reading.** `markup::read_page`: strokes more than 2.5 letters tall or 6 long are drawing and left out. Letter-sized strokes are grouped into lines by the height of their centres (a jump of 0.8 letter starts a new line); dots, apostrophes, ascenders and descenders then join the nearest line within a letter. Lines split at gaps over 6 letters (labels side by side). Each line is read like a note; a piece of a drawing that still gets through (an eye, an arrowhead, a box) reads as a stray character or a runaway repeat and is dropped when it has at most two strokes. The page's text is stored as `ink_text`, one line per line of writing. On the two sample pages (13 lines, 2 diagrams), Qwen3-VL 2B read 11 lines exactly, about 2 s per page on the GPU.

## 8c. Pen marks and glosses (0.4)

Two ways to curate while reading, with the stylus, instead of afterwards at the desk. Both build on §8a: the note text and the circled word are already read; what's new is acting on them. Both follow reading habits far older than e-readers (see *Precedents*), which the guide explains; the UI keeps plain words.

### Pen marks: triage from the margin

**Marks.** Written in a markup's note, or on its own line of a notebook page:

| Mark | Effect | Precedent |
|---|---|---|
| `*` (a drawn star, as the model reads it) | Star | Aristarchus's *asteriskos*, "little star", beside lines of Homer (Alexandria, 2nd c. BC) |
| `NB`, `N.B.` | Star | *nota bene*, medieval and later |
| `?` alone | Tag `question` | *quaere*, early modern readers' query |
| `#word` | Tag `word` | commonplace-book headings (Erasmus; Locke's indexing method, 1706) |

**Parsing** (`markup::marks`). A mark counts only as a *whole token* standing apart: `#word` anywhere in a note (starting with a letter; then letters, digits, `-`, `_`); `*`, `NB` (also `N B`, `N.B.`, `nb`) and `?` only as a note, or a line, of their own. So "Is this true?" is a note, not a question tag, and "see #1" isn't a tag.

**Shapes** (`markup::shape`). Three things the model misreads are known from the ink instead, on real samples (2026-09-29, Libra Colour): a loop around a word or two (one stroke, a speck aside, at least 1.5× wider than tall, enclosing over 30% of its box once gaps of 4 px are closed) is a circle mark, not a note (the model "read" empty loops as `123456789`; circles by size, §8a, need 100×60 px, too big for a short word); a star drawn as an outline in one to three strokes (five far points in its radial profile) is `*` (read as `5`); and a hook with a dot or dash below it is `?` (one of two read as `3`). Asterisk-style stars are read as `*` by the model. `Q` was dropped: handwritten, it read as `9` and `a`. Tag names are lower-cased; a `#word` that isn't an existing tag but is within 0.8 similarity of one (four letters or more, so one letter off) takes that tag's name, so a misread `#leadershp` or a typed `#bsed` joins the existing tag.

**The note.** Marks are taken out of a handwritten note (and a page's text) before it's stored in `ink_note` (`ink_text`), so `#leadership` becomes the tag, not words; a note that was only marks leaves no note, so it doesn't count in the Notes view.

**Where to write them.** Beside what they're about: underline or circle the passage and write the mark in the margin, so the card shows the passage. A mark applies to its whole markup, and the Kobo keeps all the ink from one visit to a page as one markup; so with two passages underlined on a page, a `?` beside one tags both (the card shows which it sits beside). On a notebook page, a mark on its own line applies to the page.

**Applied once.** A mark takes effect the first time it's read, and is remembered (`annotation.pen_marks`, JSON of what was applied). A later reading (another model, or the ink read again) applies only marks not applied before, so a star or tag the user removed in Kollate stays removed. A pen star stars *and keeps* the highlight, as starring in the Inbox does.

**Typed notes too.** The same rules (`#word`, and `*`, `NB` or `?` as the whole note) apply to notes typed on the Kobo's keyboard, bringing this to every Kobo, not just stylus models: `#leadership` typed on a highlight on a Clara becomes a tag, and a note of just `?` a question. Typed marks are applied once, like handwritten ones; the typed note is the Kobo's and is shown as it is.

### Glosses: circle a word to learn it

A medieval reader who met a hard word wrote its meaning above it or in the margin: a *gloss*; collected, glosses became the first glossaries. In Kollate, **circling a single word** on a page does the same: after the markup is read, the circled word goes to **Vocabulary** like a word looked up on the Kobo.

- **Which marks.** A circle mark (§8a) whose marked text, snapped to the book, is one word (letters, with inner `-` or `'`: "knight-errant", "o'er"). Underlines stay highlights: an underline means "this passage". A markup can have several circled words; each is a gloss.
- **The word.** Merged by lemma with existing words (§8), so circling "leviathans" joins "leviathan". Language from the book's. The definition comes from the dictionaries as for any word.
- **The sentence.** The book's sentence containing the word at that spot, from the words around the markup that import already saves (`markup_context`, §8a): they keep their punctuation, so joined they split into sentences, and the one nearest the middle (the markup) is taken. So markups imported before 0.4 get glosses too.
- **Your gloss.** If exactly one word was circled and the markup has a handwritten note (after marks are taken out), the note becomes the word's own note (`vocab.user_note`), shown as "Your gloss" in the word's row and dialog (editable), in the Anki card's back (with the definition, so the note type is unchanged) and in Obsidian. With several words circled, which one a note is about can't be told. The markup keeps its note too, and an existing user note on the word is never replaced.
- **Once, and linked.** The sighting records its markup (`vocab_sighting.annotation_id`, unique), so a re-read doesn't duplicate it, and a sighting's source reads "Circled on the Kobo" rather than "Looked up". Trashing the markup leaves the word; deleting the word leaves the markup.
- **Setting.** Preferences → Handwriting: **Circled Words Go to Vocabulary** (on by default).

### Data model (migration 9)

`annotation.pen_marks TEXT` (JSON: `{"star": true, "tags": ["leadership"]}`) and `vocab_sighting.annotation_id INTEGER REFERENCES annotation(id) ON DELETE SET NULL`, unique with the word. No change to import identity: marks and glosses are derived from the reading, after import. Marked passages are listed in page order (top to bottom), not drawing order, and a word or two the pen ran on to past a full stop ("…humanity. At"), or started on before one, is trimmed. The transcription hash (§8a) includes a reading version (2 in 0.4.0, 3 after these fixes, 4 once readings that loop are dropped; see §8a), so every markup is read once more after upgrading: loops that were read as notes become circles, and marks already written are applied.

### Samples (2026-09-29)

Fourteen markups on a Libra Colour: asterisk stars (including one alone), an outlined star, `NB`, two `?`, two `Q`, "why?", `#based`, `#justice`, and three circled words with and without glosses; plus typed notes `#based`, `#bsed` and `?`. With the shape checks, every mark was applied as intended, `Q` aside, and all circled words went to Vocabulary with the right sentence and gloss.

### Precedents (for the guide's "Old habits" note)

Aristarchus of Samothrace, head of the Library at Alexandria, edited Homer with marginal signs: the *asteriskos* and the *obelos* (a dash for a doubted line). Medieval readers wrote *nota* or *nota bene*, later the manicule ☞. Glosses explained hard words between the lines, and glossaries grew from them. Commonplace books gathered passages under headings, from Erasmus to Locke. And scriptoria *collated* copies against their exemplar, which is Kollate's name.

### Decided (2026-09-29)

A pen star also keeps the highlight; marks work in typed notes too, on every Kobo; glosses come from circles only; `?` tags `question`. The `?` is a one-stroke shortcut for `#question`: a list of passages to look into, worked through like the Inbox.

### Out of scope

A drawn manicule (hard to recognise reliably), the obelus as a "doubt" tag, colour as meaning (colours aren't decoded, §8b), and marks that act on other highlights on the page.

## 8c'. Dictionaries for other languages (0.5)

Preferences → **Get a Dictionary** lists reader.dict's free Wiktionary dictionaries (`dict::catalog::READER_DICT`: 17 languages with approximate sizes; `https://www.reader-dict.com/file/<l>/dict-<l>-<l>.df.bz2`), the way Handwriting lists models: the languages the library uses (books with annotations or words, and looked-up words' dictionary languages, `Library::languages_in_use`) that no dictionary covers come first, marked "Your books use it"; the rest are under More Languages. **Download** opens the address in the browser; the file is then added with **+**, whose chooser starts in the Downloads folder, and named and assigned a language from its file name as before. reader.dict updates its files in place, so unlike models they aren't pinned by checksum; any DictFile is accepted.

## 8d. Quote cards (0.5)

**Share as Image…** in a card's menu draws the highlight (or a markup's or notebook page's text) as a 1080×1080 or 1080×1350 PNG in one of six palettes (Paper, Night, Sepia, Highlight: a tint of the Kobo colour, Ink, E-ink), each designed as a whole rather than free colour choices, so a card stays readable: a rule in the Kobo highlight colour, optionally the markup's page with its ink, the quote in Literata sized to fit (88 px down to 24, then cut with an ellipsis), the note in italics, and the book and author at the foot; optionally a small "Kollate". A notebook page with its page shown is the page alone. Drawn in the app with Pango and Cairo; the page JPEG is decoded with `image`, not glycin, whose sandboxed loaders can fail inside a sandbox. Literata (SIL OFL, credited in THIRD-PARTY.md) is built into the binary, written to the cache on first use and added to fontconfig for the process; a fresh Pango font map sees it, so the rest of the app is untouched.

The card can be copied (as a texture), saved (never onto a Kobo), or attached to a draft in the user's email app through the desktop's Email portal (`org.freedesktop.portal.Email.ComposeEmail`, with the PNG passed as a file descriptor from Kollate's cache). The portal opens the default mail app (Thunderbird, Evolution…); Kollate sends nothing itself and needs no network permission. An optional address in Preferences → Sharing (usually the user's own) fills in the recipient (`address`). Choices are remembered (`card_*` settings).

---

## 8e. KOReader (phase 1 built 2026-10-02)

KOReader is an alternative reader that runs alongside Nickel on Kobos (and on Kindles, PocketBooks and Android). It keeps its own highlights, notes and vocabulary, and with the third-party **Pencil** plugin (`pencil.koplugin`) it records stylus handwriting. None of this is in `KoboReader.sqlite`. Read-only, like everything else: Kollate never writes to the device.

Sample (Libra Colour, KOReader v2026.07.1, Pencil plugin `pencil_strokes.lua` version 3) in `~/.cache/kollate-eval/koreader-sample`: 3 highlights (blue, yellow, green with a typed note), 2 vocabulary words, 8 Pencil groups (handwritten notes, underlines with a note and a `?`, circled words) in *The Broken Sword*. `lua2json.py` there parses the files.

### Where the data lives (on a Kobo: `.adds/koreader/`)
- **Book settings ("sidecar"):** `<book>.sdr/metadata.<ext>.lua`, a Lua table written by KOReader's `dump.lua` (`return { ... }`: strings, numbers, booleans, nested tables, `["key"] =` and `[n] =` keys, `--` comments). Location follows `document_metadata_folder` in `settings.reader.lua`: `doc` (next to the book, the default and the sample's), `dir` (`.adds/koreader/docsettings/<full book path>.sdr`) or `hash` (`.adds/koreader/hashdocsettings/<md5[0..2]>/<md5>.sdr`). Read all three. `.old` files are KOReader's previous copy; ignore them. Books can be in any folder: the device is walked eight folders deep (skipping hidden ones), and every book in KOReader's `history.lua` is looked up directly (`Book.kepub.epub` → `Book.kepub.sdr`).
- **Vocabulary:** `settings/vocabulary_builder.sqlite3`.
- Also present, not needed: `history.lua` (recent books), `settings/statistics.sqlite3` (reading time per page; could later feed `last_read_at`), `settings/lookup_history.lua` (every dictionary lookup).

`koreader::lua` reads that subset with a small recursive parser (nesting capped), never `eval`. Unknown keys are ignored, so new KOReader fields cost nothing.

### Books
Per sidecar: `doc_path` (`/mnt/onboard/...`), `doc_props` (`title`, `authors`, `language`, `identifiers` newline-separated with ISBN/calibre/uuid, `series`), `partial_md5_checksum` (KOReader's own book identity), `percent_finished`, `summary.status` (`reading`, `complete`, `abandoned`). A KOReader book gets the volume ID Nickel uses for a sideloaded book (`file:///mnt/onboard/<path>`), so a book read in both is one book with one `book_source`, and Nickel's details win when both list it. Otherwise the fingerprint (§6.1) matches it as usual. Book details a reader doesn't know (KOReader has no publisher) no longer erase what another recorded (`coalesce` on import). A book without a Nickel cover (one only KOReader opened, say) gets the EPUB's own: the manifest item with the EPUB 3 `cover-image` property, else the one EPUB 2's `<meta name="cover">` names, else an image named "cover", kept only if it's really an image (`epub::cover_image`). KOReader writes settings for every book it opens; only those with an annotation or a word are imported, as with Nickel.

### Highlights and notes (`annotations` in the sidecar)
Each entry: `text`, `note` (optional, typed), `chapter`, `color`, `drawer`, `pos0`/`pos1` (crengine XPointers, e.g. `/body/DocFragment[9]/body/div/div/p[20]/span[1]/text().78`), `page` (= `pos0` for EPUB), `pageno` (layout-dependent; ignore), `datetime` and `datetime_updated` (device local time, `YYYY-MM-DD HH:MM:SS`, no zone). Bookmarks have no `pos0`/`text`; skip them, as with Nickel's dog-ears.
- **Identity:** KOReader keys annotations by `datetime`. `bookmark_id = "koreader:" + blake3(partial_md5 + datetime + pos0)`. `datetime_updated` plays the part of Nickel's `DateModified` in the merge rules (§6.2); a missing entry is "removed on device". Annotations from KOReader aren't taken as removed when its files weren't read (`KoboSnapshot::koreader_read`, false for a database-only import or without KOReader) or when any of them couldn't be (`koreader_unread`): an unreadable sidecar can't be tied to its book.
- **Colours:** KOReader has nine (`red`, `orange`, `yellow`, `green`, `olive`, `cyan`, `blue`, `purple`, `gray`); Kobo has four (§13). **Decided: named colours throughout.** Migration 10 turns `annotation.color` into a name (`TEXT`): Kobo's 0–3 become `yellow`, `pink`, `blue`, `green`, and KOReader's are stored as they are. That touches the card's colour bar (`hl-<name>` classes, one per colour in `style.css`), the quote card's Highlight palette (tinted by name), the Obsidian, CSV and JSON exports, and the CLI. Yellow stays the default that exports leave unsaid.
- **Drawers:** `lighten` (highlight), `underscore`, `strikeout`, `invert`. All import as highlights; the drawer isn't stored yet.
- **Positions:** `DocFragment[n]` is the n-th spine item (1-based), stored as `spine_index` n − 1; `start_path` is the rest of the XPointer and `start_offset` its trailing offset. Within a chapter, KOReader highlights sort among themselves by that path; they don't interleave exactly with Nickel's (`position_key` falls back to chapter progress). An XPointer-to-text walk for marks' context words (§8a) comes with phase 2.
- **Pen marks** (§8c) apply to the typed `note`, as they do to Nickel notes.

### Vocabulary (`vocabulary_builder.sqlite3`)
`vocabulary(word UNIQUE, title_id, create_time, prev_context, next_context, highlight, ...)` and `title(id, name)`, read from a copy. The sentence is already there: `prev_context + word + next_context`, trimmed to the sentence, and stored as the sighting's context unless one was chosen. The language is the book's (`en` filed under none, as Kobo files English words), so a word looked up in both readers is one word. The book is known only by **title** (`title.name`), matched to a library book by normalized title (unique match only, else the sighting has no book). KOReader's review schedule (`due_time`, `streak_count`) is ignored. `highlight` holds the selection when the word was looked up from a highlight.

### Handwriting (Pencil plugin)
Per book: `<book>.sdr/pencil_strokes.lua` and `<book>.sdr/pencil_images/<group id>.jpg`.
- **Strokes** (`strokes[]`): `points[]` of `{x, y}` in **screen pixels of the page as laid out at the time** (1264×1680 portrait on the Libra Colour), pen centre-lines (not Kobo's filled outlines), plus `page` (layout page number), `tool` (`pen` or `highlighter`), `width`, `color_name`, `alpha`, `datetime` (Unix seconds). Erasing removes whole strokes, so nothing erased remains.
- **Groups** (`annotation_groups[]`): strokes on one page within 10 s and 200 px of each other, like one Nickel markup (one visit's ink): `stroke_indices`, `bbox`, `page`, `tool` (majority), `datetime`/`datetime_last`, and usually `xpointer` (the text at the bbox centre, `xpointer_v2 = true`), `image_path`, `image_rotation`.
- **Images:** a full-width strip of the rendered page **with the ink drawn on**, from the bbox ± 24 px, at least 350 px tall, centred on the bbox and shifted to stay on screen (`Geometry.captureStripRect`). The strip's top is computable, so strokes line up with it exactly (checked). There's no ink-free page as on Nickel: erasing the strokes (their lines widened to ~9 px) from the strip leaves the print readable enough to find lines, with faint ghosts and some nicked descenders.
- **Fragile anchoring:** layout page numbers change with font, margins or rotation, so a group is only placeable through its `xpointer`. When a stroke is erased, the plugin regroups everything; groups on other pages then **lose their `xpointer` and image** until that page is shown again in the same rotation (`backfillGroupXPointers`). In the sample 3 of 8 groups were in that state. Group `id`s also change on every regroup (`pencil_<regroup time>_<first stroke>`).
- **Identity:** not the group id. `bookmark_id = "koreader-ink:" + blake3(partial_md5 + datetime and first point of the group's first stroke)`; a group whose strokes change is "changed on device", and new ink re-reads it (§8a hash).

**The plugin itself** is AGPL-3.0, but its maintainer has been inactive since May 2026, with six pull requests unreviewed. Kollate reads its files as they are, best-effort, and doesn't depend on it changing. The changes that would make its handwriting as dependable as Nickel's: an anchor recorded per stroke when drawn (never dropped on regroup), a strip saved without ink next to the inked one, and group ids derived from the first stroke. Offer them upstream first; the licence allows a fork if they go unreviewed.

**Reading it** reuses §8a/§8c:
1. Build an ink SVG from the points: one `<path d="M… L…">` per stroke with `stroke-width` = `width`, in strip coordinates. `segment::strokes` and `image::render_note` learn stroked paths (bounds grow by half the width) next to Kobo's filled ones.
2. Segment as now (underlines, circles, word loops, stars, `?`, notes; sideways notes).
3. **Marked text:** the "page" is the strip with the strokes erased; lines come from the row profile as on Nickel; crops are read and snapped to the book's words around `xpointer` (the chapter from `DocFragment`, a window of words around the anchor).
4. **Without an `xpointer`:** read the handwriting and pen marks only; skip marked text; place the note by `page / doc_pages` as an approximate position, flagged "approximate". Re-imports upgrade it when the plugin backfills the anchor.
5. Highlighter strokes count as marks (like underlines), never as writing.
6. The card shows the strip image (as Nickel markups show the page band).

### Detection and UI
- A mounted Kobo with `.adds/koreader/` is read for KOReader data on every import, alongside `KoboReader.sqlite` (`koreader::add_koreader`, in the app and the CLI). The import toast counts both; once the library has any annotation from KOReader, every card names its reader in its details, "Kobo" or "KOReader" (`Annotation::from_koreader`, `Library::has_koreader`); a Kobo-only library shows neither.
- Other KOReader devices over USB (Kindle `koreader/`, PocketBook `applications/koreader/`) use the same files; detecting them comes later (decided 2026-10-02: Kobo only at first).
- Untested Pencil versions (`version` other than 3) import highlights and vocabulary as usual and skip handwriting with a one-time notice.

### Plan
- **Phase 1:** KOReader highlights, notes (with pen marks, §8c) and vocabulary, and named colours.
- **Phase 2:** Pencil handwriting: from the fork's markup export (§8f), and best-effort as above from the original plugin's files, groups without an anchor imported at an approximate position.
- **Kobo only** at first; Kindle and PocketBook later.

## 8f. Pencil fork and markup export (planned, 2026-10-02)

The Pencil plugin (§8e) is AGPL-3.0 and unmaintained. A maintained fork, by the Kollate maintainer, fixes its anchoring and writes each page of handwriting the way Nickel does (ink, a clean page), plus what Nickel can't: where every word on the page is. Kollate then reads KOReader handwriting through the same pipeline as Nickel's (§8a), with marked text taken from the words by geometry instead of read from pixels.

### The fork
- **Drop-in:** the folder stays `pencil.koplugin`, so it replaces the original rather than competing for the stylus. Existing `pencil_strokes.lua` (version 3) is read and upgraded on save; no ink is lost.
- **Licence and credit:** AGPL-3.0 kept, original copyright kept, the fork's changes under the maintainer's copyright, noted in the README. An issue on the original repo says the fork exists and offers to merge back. Open pull requests there (#86, strokes lost on file rename; #77, drawing lag) are reviewed and carried over with credit.
- **Support:** the current KOReader release and nightlies; no older versions. EPUB first (rolling layout); PDF and other paged documents keep working and get the same export where they have a text layer.
- **Name:** open (`andrew-lawlor/pencil.koplugin`, or a new repo name with "Pencil" as the plugin's display name).

### Plugin store, version 4 (`pencil_strokes.lua`)
The plugin's own fast store, still a Lua table. Changes from version 3:
- **Stable group ids,** from the group's first stroke (its `datetime` and first point), never from the time of a regroup.
- **An anchor per stroke,** recorded when it's drawn (the page is on screen, so it always can be): the XPointer of the nearest word and the stroke's offset from that word's box, in line heights. Regrouping (after an erase or undo) can't lose it. Version 3 strokes are anchored the first time their page is shown again, as today.
- `version = 4`. Unknown fields are kept on save.

### Markup export, the contract with Kollate
One **markup** is the ink of one page visit: everything drawn on a page between arriving and leaving it, as on Nickel (§13). Returning later and writing more starts a new markup; erasing strokes rewrites the markup they belong to, and erasing all of them removes it. Written to the book's sidecar:

```
<book>.sdr/pencil/markups/<markup id>/
  markup.json  written last: its presence means the folder is complete
  ink.json     the strokes, in page pixels, in writing order
  page.png     the page as rendered, without the plugin's ink
  words.json   every word on the page, with its box and XPointers
```

- **markup.json:** `format` (1), `id`, `created`, `modified` (Unix seconds), `document` (`doc_path`, `partial_md5`), `page` (layout page number), `start`/`end` (XPointers of the first and last word on the page), `chapter`, `screen` (`width`, `height`, `rotation`), `layout` (font face and size, margins, line spacing: what changes pagination), `plugin_version`.
- **ink.json:** `strokes[]`, each `points` (`[[x, y], ...]` in page pixels), `width`, `color` (name), `tool` (`pen` or `highlighter`), `datetime`, `anchor` (`xpointer`, `dx`, `dy`). Centre-lines; Kollate draws them as stroked paths (`segment::strokes` and `render_note` learn stroked paths next to Nickel's filled outlines).
- **page.png:** the whole screen, painted by `ReaderView:paintTo` with the plugin's own drawing off (KOReader's highlights stay, as Nickel's page images keep theirs). PNG, not JPEG: text compresses far better without loss (a 1053×1400 page: 112 KB as PNG, 310 KB as JPEG at quality 85, 59 KB as 16-grey PNG). Greyscale on a black-and-white screen, colour on a colour one. Written once per markup, after the existing capture delay or on leaving the page. Kollate's `image` dependency gains PNG decoding.
- **words.json:** `words[]`, each `text`, `boxes` (one `[x0, y0, x1, y1]` per line the word is on: a word hyphenated across lines has two), `pos0`, `pos1`. EPUB: the visible text's first and last positions (`getTextFromPositions` over the whole screen, not `getPageXPointer`, which didn't match the screen in the spike), then each word with `getNextVisibleWordStart`/`End`, its text from `getTextFromXPointers` and boxes from `getWordBoxesFromPositions`. PDF: `getTextBoxes(page)`. A page without a text layer (a scanned PDF) has none.
- **Atomic:** each file is written to a temporary name and renamed into place, `markup.json` always last; Kollate ignores folders without `markup.json`. (Rewriting a markup after an erase keeps its page image and words, which a whole-folder swap would have to copy.)
- **When:** the page picture and words are taken at the first pause (0.6 s) after writing on a page, never while the pen is down, and kept in memory; the PNG is encoded and the files written with the plugin's next deferred save. Ink drawn before the fork gets a markup per group the first time its page is shown again as drawn (the same condition as anchoring it).
- **Built (fork commit `7af521d`, 2026-10-02):** checked in KOReader's desktop build: a markup per visit, a grey page with none of the plugin's ink under the strokes, words on their boxes, an underline's words found from the boxes alone ("Laertes when his fatal hour shall"), a markup removed when all its ink is erased, and a new one on returning to a page.
- **Partial re-rendering:** after a layout change, KOReader (with `partial_rerendering`, on by default) lays out only what's needed, finishes the full layout in the background, and shows a small icon in the top-left corner meanwhile (`ReaderRolling.rendering_state` is set; a `DocumentRerendered` event follows when it's done). What's on screen is what the reader wrote on, so ink, anchors and words taken then are right, but page numbers can still change and the icon would be painted into the page image. So `page.png` is captured only once `rendering_state` is clear (waiting for `DocumentRerendered` if need be), `page` in `markup.json` is advisory (the XPointers are what place a markup), and any test that changes the layout waits for the re-render before measuring.

### Kollate, phase 2 (built 2026-10-03)
- A sidecar's `pencil/markups/` folders are read (`koreader::pencil`); a folder without `markup.json` is being written and is skipped, and one that can't be read leaves KOReader's annotations as they are (`koreader_unread`). The original plugin's own file (version 3) isn't read: only the fork's export.
- Each markup becomes a `markup` annotation: `bookmark_id = "koreader:ink:" + id` (under KOReader's prefix, so it's labelled KOReader and guarded like its highlights), positioned by `start` (spine from `DocFragment`), chapter and times from `markup.json`, the crop from the ink's bounds plus 40 px (§13).
- **Ink:** `ink.json` becomes an SVG of stroked paths (pen centre lines, each with its colour and width; the highlighter at 40% opacity). The segmenter counts a stroked path's width in its bounds, as Nickel's filled outlines already are: without it, letters measured small and word gaps split lines ("slip, / if you / will."). Nickel's paths have no `stroke-width`, so their reading is unchanged. For the model, pen lines are drawn at least 6 px wide (`READING_WIDTH`): on 18 notes from an Elipsa 2E, 97.4% of characters right at the plugin's 3 px, 97.9% at 4.5, 98.6% at 6. Cards show the ink as drawn.
- **Page:** `page.png` is converted to a JPEG in the library (`<id>.jpg`, the `image` crate's PNG decoding, already vendored through resvg), so it's composed and shown like Nickel's; without one, a blank page. The page's words are kept beside the ink (`<id>.words.json`, `store::page_words_path`), and their text is the markup's context (`markup_context`), for name correction and glosses.
- **Marked text by geometry** (`markup::words`): an underline marks the words on the line whose bottom is nearest it (among lines starting above it), more than half within its width (±12 px); a circle the words whose middle is inside it, 60% within its width. The text is the words' own: no model reads print, nothing is snapped. Words that follow one another are one passage, so underlines drawn one after another over separate passages give one passage each, which Nickel's page images can't tell apart. Markups without words fall back to reading the page and snapping (§8a).
- **Checked on an Elipsa 2E** (26 markups in *The City of God* and *The Western Canon*, 2B model): every underline and circle gave the exact words; notes 98.6% of characters right.

### Fork status (2026-10-02)
github.com/andrew-lawlor/pencil.koplugin, released as 0.6.0 (continuing the original's 0.5.0), tried on the Libra Colour. It works with stock KOReader 2026.07 `input.lua` (pen, eraser end, stylus button, finger page turns), so the original's patched file is no longer needed. Not yet tried on the Elipsa 2E.
- **Store version 5:** upstream PR #77's packed points (and its save-debounce fix, which had made the original rewrite its whole strokes file after nearly every stroke), plus anchors per stroke and stable group ids. A real 204-stroke file: 1.29 MB → 204 KB, every point identical.
- **Markup export** as above, with nothing slow near the pen: pages captured on arrival, anchors from the captured words, encoding and writing at 8 s idle (one picture per pause), saves never mid-stroke.
- **Pen latency:** on MediaTek Kobos, KOReader waits for the display controller to accept each partial UI-waveform update, and the controller holds one back while an overlapping update is still running (~250 ms). Black pen ink now uses the fast (DU) waveform while writing, then one UI refresh once writing stops. The slowest point per stroke went from a 263 ms median to 3 ms (the original plugin: 12 ms); measured with the fork's profiler (`lib/profile.lua`).

### Later (in the fork)
**Ink after a font change:** strokes are drawn near their anchor words on the new layout (offsets in line heights), flagged in the plugin as "moved". Never exact for a margin note, but better than losing it; not promised for the first release.

### Spike (2026-10-02)
In KOReader 2026.07.1's Linux build, at 1053×1400 and 300 dpi, on *The Odyssey* (Standard Ebooks), with a throwaway plugin (`~/.cache/kollate-eval/pencil-spike/`):
- **Word layer:** 181–196 words a page in 5–10 ms on a desktop CPU. Every box sat on its word, drawn over the page image to check; hyphenated words came back with a box per line.
- **Page image:** painting 4 ms; PNG encoding 51 ms, JPEG 4 ms (sizes above).
- **Anchors survive a font change:** a word's XPointer, taken at size 22, resolved after a re-layout at size 30 (page 40 became page 47) to the word's new box. The re-layout happens on the next screen refresh, and may be partial at first (above), so code that changes the layout must wait for it before measuring. The first capture in the spike, taken just after opening the book, has the partial-rendering icon in its corner.
- **On the Libra Colour** (1264×1680, 300 dpi, `Kobo_monza`, *The Broken Sword*, six pages; results in `~/.cache/kollate-eval/pencil-spike-device/results/`): 117–179 words a page in 39–68 ms, every box on its word; painting 35–63 ms; JPEG 41–54 ms (206–348 KB); **PNG 458–552 ms** (84–118 KB), of a colour (RGB32) buffer. KOReader is single-threaded, so half a second of PNG encoding would freeze the screen. The fork converts the page to 8-bit grey before encoding (a quarter of the data; colour adds nothing to finding lines or reading ink) and encodes when the page is left or the reader is idle, never right after a stroke; if grey PNG is still slow on the device, JPEG at quality 85 is the fallback (Kollate reads both). The fork's first build measures this.
- **The page image must leave out the plugin's ink:** the spike's capture on the device includes the installed Pencil plugin's strokes, since `paintTo` paints every view module. The fork skips its own drawing while capturing (the plugin already sets `_capturing` for its strips).

### Plan
1. ~~Spike~~ (above), on the desktop and the Libra Colour.
2. **Fork, first release:** version 4 store (stable ids, anchors per stroke), the markup export, the carried-over fixes, tests (the repo's `busted` specs, plus new ones for the store and export).
3. **Kollate phase 2,** with fixture markups from the fork in `tests/fixtures/`.
4. **Fork, later:** ink after a font change.

### Open questions
1. The fork's name.
2. When a page visit ends for a long session on one page: on leaving only, or also after some minutes without ink?
3. Whether highlighter strokes should also become native KOReader highlights (the plugin's `experimental_text_highlight`), now that the words under them are known.

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
- **Keyboard Shortcuts window** (Ctrl+? or the main menu): grouped sections (Triage, Selecting Several, Moving Around, General) with keys drawn as keycaps. It's built with AdwPreferencesGroups because AdwShortcutsDialog needs libadwaita 1.8 and the app targets 1.5 (§3). A one-time tip banner in the Inbox mentions K/A/S/Delete and Ctrl+?; "Got It" dismisses it for good (`setting.tip_triage_dismissed`).
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
5. ✅ **M4, vocab enrichment:** EPUB context extraction, bundled dictionary (now English Wiktionary) and dictionary import, lemma merge.
6. ✅ **M5, export:** Obsidian vault sync and Anki, then JSON, CSV and Readwise.
7. **M6, polish & ship:**
   - ✅ `.deb` via cargo-deb (`scripts/build-deb.sh` → `target/debian/kollate_<ver>_amd64.deb`). It contains `kollate`, `kollate-cli`, the desktop entry, AppStream metainfo, a scalable icon, the English Wiktionary dictionary under `/usr/share/kollate/dictionaries/`, the GPL-3 copyright file and a third-party notice. Dependencies come from shlibdeps. App ID: `io.github.andrew_lawlor.Kollate`.
   - ✅ Flatpak: `flatpak/io.github.andrew_lawlor.Kollate.yml` on GNOME 51 with rust-stable//26.08. The build is offline, using pinned sources in `flatpak/cargo-sources.json`, and the English Wiktionary is downloaded by sha256 from Kollate's data release and converted at build time. Permissions: wayland, fallback-x11, dri, ipc, `/media:ro`, `/run/media:ro`, `org.gtk.vfs.*`. **No network.** `scripts/build-flatpak.sh` installs the app for the user and writes `target/flatpak/kollate.flatpak`. Verified with the Kobo connected: it autodetected the device through gvfs, imported 56 highlights, found 12 contexts, defined 10 of 11 words, and copied 6 covers and 1 markup image.
   - ✅ README with screenshots. Public repo at https://github.com/andrew-lawlor/kollate.
   - ✅ Eject works from inside the Flatpak sandbox (confirmed by the user on the Libra Colour).
   - ✅ Markup pages are drawn by Kollate (`compose_page`, §8a).
   - Still to do: a Flathub submission (which needs a git source in place of `type: dir`).

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
- Releases are built by GitHub Actions, not on a developer machine: a `v*` tag builds the .deb (on Ubuntu 24.04, the oldest supported platform) and the Flatpak (GNOME 51 container), attests their provenance, and creates a draft release. CI runs fmt, clippy, tests and a `cargo-sources.json` check on every push and pull request. The app's GTK/libadwaita feature flags (`v4_12`, `v1_5`) and its CSS (named colours, not CSS variables) match that minimum.
- Handwriting transcription will be local only (Qwen3-VL via llama.cpp, in-process); a cloud model was rejected as contrary to the offline promise. Models are user-added files, never downloaded by the app (§8a).
- Unknown Kobo database versions are imported with a warning rather than refused (§4); compatibility reports come through a GitHub issue form.

## 13. Verified on the device (2026-09-25, Libra Colour, firmware 4.45.23697)
- `.kobo/version` = `N000000000000,4.9.77,4.45.23697,4.9.77,4.9.77,00000000-0000-0000-0000-000000000390`, i.e. serial, ?, firmware, ?, ?, model ID (`…0390` = Libra Colour). The parser matches.
- Markups: `.kobo/markups/<BookmarkID>.svg` holds **only the ink strokes** (Qt SVG, page-sized viewBox 1264×1680). `.jpg` is the rendered page **without** the ink (corrected 2026-09-26 with real stylus notes; the earlier sample's SVG held a single stray dot, so this went unnoticed). Import both. `assets::markup_page` draws the ink onto the page (`compose_page`, §8a) and writes `<id>.page.jpg` in the library; "Open Page Image" uses it. The card and the Obsidian export use `<id>.page-<top>-<bottom>.jpg`, the same page cut to the full-width band from the top of the ink or its anchored text to the bottom of either, plus a 40px margin. That band comes from `ExtraAnnotationData`, which `kobo::qvariant` reads leniently (it stops at any unknown type, so a firmware change costs the crop, never the import), and is stored in `annotation.markup_crop` (migration 6) when assets are copied.
- One markup holds all the ink from one visit to a page, so it can contain several separate notes. `Text`/`Annotation` stay empty: the Kobo doesn't transcribe handwriting in books. `ExtraAnnotationData` decodes fully (19 keys); the useful ones are `MarkupRect` (ink bounding box) and `RangeRect` (the text the markup is anchored to, in page pixels), plus `StartContainerPath`/`EndContainerPath` on the row.
- Dictionaries: `.kobo/dict/dicthtml.zip` (English, **no `-en` suffix**) plus `dicthtml-en-zh-CN.zip` / `-zh-TW`. Inside: `words` / `prefix_exceptions` are marisa tries, and the `*.html` shards are **encrypted** (not gzip). **Decision: we don't use Kobo's dictionaries** (see §8).
- `Exported Annotations/` and `Exported Notebooks/` exist (Kobo's own export feature) and are empty. Ignore them.
- `driveinfo.calibre` is present, so the user manages books with calibre. Calibre may rename or re-send books, which the book fingerprint (§6) handles.
- Colour index: 0 yellow, 1 pink, 2 blue, 3 green (verified with test highlights in *Free Software, Free Society*).

## 13a. Verified on a Kobo Clara 2E (2026-09-27, firmware 4.38.21908, DbVersion 174)
- Model ID suffix `…0386`; serial prefix N506. `content`, `WordList` and `DbVersion` match the Libra Colour's; `Bookmark` lacks only `Color` (no colour screen). Highlights read as colour 0, what colour Kobos record for a default highlight.
- Its `WordList` pointed at `/mnt/onboard/books/…` paths no longer in `content`: the books had been moved (one) or removed (four). Moved books are matched by file name; removed ones get their title and author from calibre's path (`Toole, John Kennedy/Confederacy of Dunces, A.kepub.epub` → *A Confederacy of Dunces*, John Kennedy Toole; `_` for characters files can't hold).
- Context sentences: EPUB 3 note references (`<a epub:type="noteref">`, `role="doc-noteref"`) and numeric superscript notes after punctuation are dropped ("entropy,10" → "entropy,"); exponents after a letter stay.
