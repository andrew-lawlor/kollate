# LinkedIn post

**My e-reader saves my margin notes as pictures. Now a small AI model on my own laptop turns them into text, and nothing leaves the machine.**

Outside of work, I read on a Kobo. It stores every highlight, note and looked-up word in a local SQLite database, and keeps my stylus scribbles as images alongside: data that's mine, but hard to get at.

So I built Kollate with Claude Code: a Linux app that imports all of it, curates it, and exports to Obsidian and Anki. Its headline feature is one the Kobo doesn't have at all: it reads your handwriting.

Building fast with AI is the part everyone expects. The lessons were the same calls I'd make on any client engagement:

🧠 **AI only where it's needed.** A model reads the handwriting, and nothing else. What you underlined or circled comes straight from the book's own text, so it's exact, and a misread name gets corrected from the page. I compared ten local models: a 2-billion-parameter one reads clear handwriting almost perfectly, in a second or two, on an ordinary laptop.

🔒 **Local by choice.** A cloud model would have been easier. But these are pages of my books in my handwriting, so it runs on my computer. The app can't reach the network at all; you download the model yourself.

📋 **Non-negotiables first.** Before any code, four rules: never touch the device, my library is the source of truth, no duplicates, and my own edits survive every sync. So a machine reading is only a suggestion: a correction always wins.

🏗️ **No shortcuts on architecture.** Not an Electron app or a quick Python script: Rust and GTK 4, native and compiled. AI didn't lower the bar; it made the higher bar affordable.

🧪 **Verify, don't trust.** Testing on my real device caught bugs no test data would have, like a page that slid out from under its ink. Releases come from a CI/CD pipeline in GitHub Actions with signed build provenance, so nobody has to take my word for what's in them.

⚖️ **Respecting boundaries.** Kobo's dictionaries are encrypted, so Kollate ships the openly licensed English Wiktionary instead, properly credited, down to the arcane words of Platonic philosophy like *hierophant* and *daimon*.

🗂️ **Data hygiene.** Before open-sourcing, I scrubbed my personal reading data from the tests and the project history.

AI changes how fast we can build, and how high we can aim. It doesn't change who's accountable for what gets built. For those of us bringing AI into public-sector and enterprise work, that's the part to get right.

github.com/andrew-lawlor/kollate

#AI #GenAI #PublicSector #GovTech #ResponsibleAI #Rust #OpenSource

---

**Images** (1080×1350, in posting order; `docs/linkedin/`):

1. `1-handwriting.png`: Your handwriting, as text. Alt text: "Kollate showing a page of the Odyssey with a handwritten margin note. Below it, the underlined sentence taken from the book, and the note as read by a local model. The handwriting is staged; the reading is real."
2. `2-starred.png`: Every highlight worth keeping (dark mode, starred highlights with notes)
3. `3-vocabulary.png`: Every word, in the sentence you met it (Wiktionary definitions)
4. `4-inbox.png`: Triage new highlights in seconds
5. `5-books.png`: Your library, by cover
