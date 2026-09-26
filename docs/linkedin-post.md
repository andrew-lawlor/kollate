# LinkedIn post

**I built an app with Claude Code over a weekend. The first version took three hours. Making it trustworthy took the rest.**

Outside of work, I read on a Kobo e-reader. Looking under the hood, I noticed it stores every highlight, note and looked-up word in a local SQLite database, with images of my stylus scribbles alongside: data that's mine but hard to get at.

So I built Kollate, a Linux app that imports that data, curates it, and exports to Obsidian and Anki. With Claude Code, it went from written spec to a first open-source release in about three hours. The rest of the weekend went into the parts that make it trustworthy.

The speed is the headline, but it isn't the lesson. What mattered were the same calls I'd want on any client engagement:

📋 **Non-negotiables first.** Before any code, I wrote down the rules: never touch the device, my library is the source of truth, re-importing never creates duplicates, and my own edits survive every sync. Claude Code moved fast, but every design decision had to answer to those four lines.

🏗️ **No shortcuts on architecture.** The usual way to build a quick side project is an Electron app or a thrown-together Python GUI. I didn't have to settle: Kollate is written in Rust with GTK 4, a native, compiled desktop app that's fast, lean and dependable. AI didn't lower the bar; it made the higher bar affordable.

🔒 **Least privilege.** The app never writes to the device, and refuses to even if you ask it to. The packaged version can't reach the network and can only *read* the e-reader.

⚖️ **Respecting boundaries.** Kobo's built-in dictionaries are encrypted. Rather than work around that, Kollate ships the community-built English Wiktionary: over 800,000 words, openly licensed and properly credited. Replacing our first open dictionary with it took under an hour, and every word I'd looked up got a better definition, even the arcane ones from Platonic philosophy like *hierophant*, *demiurge* and *daimon*.

🧪 **Verify, don't trust.** I tested against my real device, and that caught a subtle sorting bug no test data would have exposed. The Anki export was confirmed with Anki's own importer. And releases come from a CI/CD pipeline in GitHub Actions with signed build provenance, so nobody has to take my word for what's in them.

🗂️ **Data hygiene.** Before open-sourcing, I replaced my personal reading data in the test fixtures with a scrubbed minimum and purged it from the project history.

AI changes how fast we can build, and how high we can aim. It doesn't change who's accountable for what gets built. For those of us bringing AI into public-sector and enterprise work, that's the part to get right.

github.com/andrew-lawlor/kollate

#AI #GenAI #PublicSector #GovTech #ResponsibleAI #Rust #OpenSource

---

**Images** (1080×1350, in posting order; `docs/linkedin/`):

1. `1-starred.png`: Every highlight worth keeping (dark mode, starred highlights with notes)
2. `2-vocabulary.png`: Every word, in the sentence you met it (Wiktionary definitions)
3. `3-inbox.png`: Triage new highlights in seconds
4. `4-books.png`: Your library, by cover
