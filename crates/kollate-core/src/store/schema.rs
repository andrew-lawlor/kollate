//! Library schema. Migrations are forward-only and tracked with
//! `PRAGMA user_version`; append new entries, never edit existing ones.

pub const MIGRATIONS: &[&str] = &[
    // 1: initial schema
    r#"
    CREATE TABLE device (
        id            INTEGER PRIMARY KEY,
        serial        TEXT NOT NULL UNIQUE,
        model_id      TEXT,
        firmware      TEXT,
        first_seen_at TEXT NOT NULL,
        last_seen_at  TEXT NOT NULL
    );

    CREATE TABLE book (
        id            INTEGER PRIMARY KEY,
        fingerprint   TEXT NOT NULL UNIQUE,
        title         TEXT NOT NULL,
        author        TEXT,
        publisher     TEXT,
        isbn          TEXT,
        language      TEXT,
        series        TEXT,
        series_number TEXT,
        percent_read  INTEGER,
        last_read_at  TEXT,
        user_title    TEXT,
        user_author   TEXT,
        cover_path    TEXT,
        hidden        INTEGER NOT NULL DEFAULT 0,
        created_at    TEXT NOT NULL,
        updated_at    TEXT NOT NULL
    );

    -- Where a book lives on each device (paths change when files are renamed).
    CREATE TABLE book_source (
        device_id INTEGER NOT NULL REFERENCES device(id),
        volume_id TEXT NOT NULL,
        book_id   INTEGER NOT NULL REFERENCES book(id) ON DELETE CASCADE,
        image_id  TEXT,
        PRIMARY KEY (device_id, volume_id)
    );

    CREATE TABLE annotation (
        id                   INTEGER PRIMARY KEY,
        book_id              INTEGER NOT NULL REFERENCES book(id),
        fingerprint          TEXT,
        kind                 TEXT NOT NULL CHECK (kind IN ('highlight', 'note', 'markup')),
        device_text          TEXT,
        device_note          TEXT,
        color                INTEGER NOT NULL DEFAULT 0,
        chapter_title        TEXT,
        content_id           TEXT NOT NULL,
        spine_index          INTEGER,
        start_path           TEXT NOT NULL,
        start_offset         INTEGER NOT NULL,
        end_path             TEXT NOT NULL,
        end_offset           INTEGER NOT NULL,
        chapter_progress     REAL NOT NULL DEFAULT 0,
        created_at           TEXT,
        device_modified_at   TEXT,
        user_text            TEXT,
        user_note            TEXT,
        starred              INTEGER NOT NULL DEFAULT 0,
        status               TEXT NOT NULL DEFAULT 'inbox'
                             CHECK (status IN ('inbox', 'kept', 'archived', 'trashed')),
        device_changed_at    TEXT,
        removed_on_device_at TEXT,
        markup_svg_path      TEXT,
        markup_jpg_path      TEXT,
        imported_at          TEXT NOT NULL,
        updated_at           TEXT NOT NULL
    );
    CREATE INDEX annotation_fingerprint ON annotation(fingerprint);
    CREATE INDEX annotation_book ON annotation(book_id);

    -- Kobo BookmarkIDs that map to an annotation. Several IDs can map to one
    -- annotation (factory reset, second device).
    CREATE TABLE annotation_source (
        bookmark_id   TEXT PRIMARY KEY,
        device_id     INTEGER NOT NULL REFERENCES device(id),
        annotation_id INTEGER NOT NULL REFERENCES annotation(id) ON DELETE CASCADE,
        first_seen_at TEXT NOT NULL,
        last_seen_at  TEXT NOT NULL
    );
    CREATE INDEX annotation_source_device ON annotation_source(device_id);

    CREATE TABLE annotation_revision (
        id            INTEGER PRIMARY KEY,
        annotation_id INTEGER NOT NULL REFERENCES annotation(id) ON DELETE CASCADE,
        field         TEXT NOT NULL,
        old_value     TEXT,
        new_value     TEXT,
        source        TEXT NOT NULL CHECK (source IN ('device', 'user')),
        at            TEXT NOT NULL
    );

    CREATE TABLE vocab (
        id                INTEGER PRIMARY KEY,
        word              TEXT NOT NULL,
        key               TEXT NOT NULL,
        language          TEXT NOT NULL DEFAULT '',
        lemma             TEXT,
        definition        TEXT,
        definition_source TEXT,
        status            TEXT NOT NULL DEFAULT 'new'
                          CHECK (status IN ('new', 'learning', 'known', 'ignored')),
        starred           INTEGER NOT NULL DEFAULT 0,
        user_note         TEXT,
        first_seen_at     TEXT,
        imported_at       TEXT NOT NULL,
        updated_at        TEXT NOT NULL,
        UNIQUE (key, language)
    );

    CREATE TABLE vocab_sighting (
        id               INTEGER PRIMARY KEY,
        vocab_id         INTEGER NOT NULL REFERENCES vocab(id) ON DELETE CASCADE,
        book_id          INTEGER REFERENCES book(id),
        device_id        INTEGER NOT NULL REFERENCES device(id),
        surface_form     TEXT NOT NULL,
        looked_up_at     TEXT,
        context_sentence TEXT
    );
    CREATE UNIQUE INDEX vocab_sighting_unique
        ON vocab_sighting(vocab_id, IFNULL(book_id, 0), surface_form);

    CREATE TABLE tag (
        id    INTEGER PRIMARY KEY,
        name  TEXT NOT NULL UNIQUE COLLATE NOCASE,
        color TEXT
    );
    CREATE TABLE annotation_tag (
        annotation_id INTEGER NOT NULL REFERENCES annotation(id) ON DELETE CASCADE,
        tag_id        INTEGER NOT NULL REFERENCES tag(id) ON DELETE CASCADE,
        PRIMARY KEY (annotation_id, tag_id)
    );
    CREATE TABLE vocab_tag (
        vocab_id INTEGER NOT NULL REFERENCES vocab(id) ON DELETE CASCADE,
        tag_id   INTEGER NOT NULL REFERENCES tag(id) ON DELETE CASCADE,
        PRIMARY KEY (vocab_id, tag_id)
    );

    CREATE TABLE import_run (
        id          INTEGER PRIMARY KEY,
        device_id   INTEGER NOT NULL REFERENCES device(id),
        started_at  TEXT NOT NULL,
        finished_at TEXT NOT NULL,
        db_version  INTEGER,
        stats_json  TEXT NOT NULL
    );
    "#,
    // 2: sortable reading position (backfilled in Library::init)
    r#"
    ALTER TABLE annotation ADD COLUMN position_key TEXT;
    CREATE INDEX annotation_reading_order ON annotation(book_id, spine_index, position_key);
    "#,
    // 3: key/value settings
    r#"
    CREATE TABLE setting (
        key   TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );
    "#,
    // 4: vocab enrichment
    r#"
    ALTER TABLE vocab_sighting ADD COLUMN context_candidates TEXT; -- JSON array of sentences
    -- Every lookup key ever merged into a word, so re-imports find it.
    CREATE TABLE vocab_form (
        key      TEXT NOT NULL,
        language TEXT NOT NULL,
        vocab_id INTEGER NOT NULL REFERENCES vocab(id) ON DELETE CASCADE,
        PRIMARY KEY (key, language)
    );
    INSERT INTO vocab_form (key, language, vocab_id) SELECT key, language, id FROM vocab;
    "#,
    // 5: remember the status of highlights auto-trashed because they were
    // deleted on the Kobo, so they can go back if they reappear
    r#"
    ALTER TABLE annotation ADD COLUMN status_before_removal TEXT;
    "#,
];
