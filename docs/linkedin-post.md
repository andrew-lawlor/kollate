# LinkedIn post

**I built an app with Claude Code in an afternoon. The guardrails took more thought than the code.**

Outside of work, I read on a Kobo e-reader. Looking under the hood, I noticed it stores every highlight, note and looked-up word in a local SQLite database, data that's mine but hard to get at.

So I built Kollate, a Linux app that imports that data, curates it, and exports to Obsidian and Anki. With Claude Code, it went from written spec to a published open-source release in about three hours.

The speed is the headline, but it isn't the lesson. What mattered were the same calls I'd want on any client engagement:

🏗️ **No shortcuts on architecture.** The usual way to build a quick side project is an Electron app or a thrown-together Python GUI. I didn't have to settle: Kollate is written in Rust with GTK 4, a native, compiled desktop app that's fast, lean and dependable. AI didn't lower the bar; it made the higher bar affordable.

🔒 **Least privilege.** The app never writes to the device, and refuses to even if you ask it to. The packaged version can't reach the network and can only *read* the e-reader.

⚖️ **Respecting boundaries.** Kobo's built-in dictionaries are encrypted. Rather than work around that, Kollate ships the community-built English Wiktionary: over 800,000 words, openly licensed and properly credited. Replacing our first open dictionary with it took under an hour, and every word I'd looked up got a better definition. Other languages are now a single download.

🧪 **Verify, don't trust.** I tested against my real device, and that caught a subtle sorting bug no test data would have exposed. The Anki export was confirmed with Anki's own importer.

🗂️ **Data hygiene.** Before open-sourcing, I replaced my personal reading data in the test fixtures with a scrubbed minimum and purged it from the project history.

AI changes how fast we can build, and how high we can aim. It doesn't change who's accountable for what gets built. For those of us bringing AI into public-sector and enterprise work, that's the part to get right.

github.com/andrew-lawlor/kollate

#AI #GenAI #PublicSector #GovTech #ResponsibleAI #Rust #OpenSource
