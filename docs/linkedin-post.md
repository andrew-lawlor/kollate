# LinkedIn post

**In government and enterprise work, the question about AI is rarely "can it do this?" It's "where does the data go?" I built an open-source app to show one good answer: nowhere.**

Kollate turns the handwritten margin notes from my e-reader into searchable text, using an AI model that runs entirely on my laptop. No cloud, no API keys, no network access at all. My notes are a stand-in for any document you can't send to someone else's server.

I built it with Claude Code, alongside the rest of the app: it imports everything the Kobo saves (highlights, notes, looked-up words), curates it, and exports to Obsidian and Anki. Building fast with AI is the part everyone expects. What mattered were the same calls I'd make on any client engagement:

🧠 **AI only where it's needed.** A model reads the handwriting, and nothing else. What you underlined or circled comes straight from the book's own text, so it's exact, and a misread name gets corrected from the page. I compared ten local models: a 2-billion-parameter one reads clear handwriting almost perfectly, in a second or two, on an ordinary laptop.

🔒 **Least privilege.** A cloud model would have been easier. Instead the app can't reach the network, and can only *read* the e-reader; you download the model yourself, once.

📋 **Non-negotiables first.** Before any code, four rules: never touch the device, my library is the source of truth, no duplicates, and my own edits survive every sync. So a machine reading is only a suggestion: a correction always wins.

🏗️ **No shortcuts on architecture.** Not an Electron app or a quick Python script: Rust and GTK 4, native and compiled. AI didn't lower the bar; it made the higher bar affordable.

🧪 **Verify, don't trust.** Testing on my real device caught bugs no test data would have, like a page that slid out from under its ink. Releases come from a CI/CD pipeline in GitHub Actions with signed build provenance, so nobody has to take my word for what's in them.

⚖️ **Respecting boundaries.** Kobo's dictionaries are encrypted, so Kollate ships the openly licensed English Wiktionary instead, properly credited, down to the arcane words of Platonic philosophy like *hierophant* and *daimon*. And every book in the screenshots is a free, public-domain edition from Standard Ebooks, a volunteer project well worth supporting.

🗂️ **Data hygiene.** Before open-sourcing, I scrubbed my personal reading data from the tests and the project history.

AI changes how fast we can build, and how high we can aim. It doesn't change who's accountable for what gets built. For those of us bringing AI into public-sector and enterprise work, that's the part to get right.

github.com/andrew-lawlor/kollate

#AI #GenAI #PublicSector #GovTech #ResponsibleAI #Rust #OpenSource

---

**Images** (1080×1350, in posting order; `docs/linkedin/`):

1. `1-handwriting.png`: Your handwriting, as text. Alt text: "Kollate showing a page of the Odyssey with a handwritten margin note. Below it, the underlined sentence taken from the book, and the note as read by a local model."
2. `2-starred.png`: Every highlight worth keeping (dark mode, starred highlights with notes)
3. `3-vocabulary.png`: Every word, in the sentence you met it (Wiktionary definitions)
4. `4-inbox.png`: Triage new highlights in seconds
5. `5-books.png`: Your library, by cover

All books, covers and quotes in the images are Standard Ebooks editions (standardebooks.org), in the public domain.
