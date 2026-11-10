# revui bundle format

A bundle is one directory that `revui <dir>` opens:

    review.json   the guide: sections, findings, decisions
    mr.patch      the raw diff, one per source. Never written by a model.
    assets/       evidence files (screenshots, recordings, logs)
    state.json    the reviewer's verdicts and comments. Never written by a generator.

`revui fetch <merge request or pull request URL>` creates the directory with `mr.patch` and a
`review.json` that has no sections, and prints its path. Running it again for the
same head finds the same directory and keeps the `review.json` that is there. A
generator then fills in `review.json` and leaves `title` and `sources` as they are.

## review.json

    {
      "title": "Retry order submission",
      "summary": "What the change does and how it was reviewed.",
      "sources": [ ...written by revui fetch, keep as is... ],
      "sections": [
        {
          "title": "Retry the submit call",
          "tier": "core",
          "why": "Why this part exists, and the invariant or branch a reviewer should check here.",
          "hunks": [
            { "file": "src/submit.ts", "hunk": 0 },
            { "file": "src/sleep.ts" }
          ]
        }
      ],
      "items": [
        {
          "id": "B1",
          "kind": "finding",
          "severity": "blocker",
          "section": 0,
          "title": "A retry can create the order twice",
          "body": "What breaks, the path that gets there, and what to do about it.",
          "at": { "file": "src/submit.ts", "line": 11 },
          "evidence": [
            {
              "provenance": "observed",
              "caption": "Orders list after one checkout: two orders",
              "path": "assets/duplicate-orders.png",
              "repro": "the command that produced it"
            },
            { "provenance": "inferred", "caption": "Sequence of the double submit", "text": "plain text, shown inline" }
          ]
        }
      ]
    }

Text fields are plain text: newlines are kept, markdown is not rendered, so write
a link as a bare URL. Keep `summary` to two short paragraphs at most. In an item's
`body`, a line that starts with `# ` is shown as a sub-heading: use it to label the
parts of a longer finding, each on its own line.

Required: `title`, `sources`, `sections` (each with `title`, `why`, `hunks`), and
for each item `id`, `kind`, `title`, `body`. Everything else can be left out.

## Sections

- Order them the way a reviewer should read: `core` first (the behavior the change
  exists for), then `supporting`, then `low` (tests, generated files, lockfiles, renames).
- `tier` is `core`, `supporting` or `low`.
- `hunks` cites the diff. `revui hunks <dir>` lists every file with its hunk indexes.
  Leave `hunk` out to take the whole file. With several sources, add `"source": "<id>"`.
- A hunk that serves two purposes cannot be split: put it in the section of the
  purpose that matters more, and say in the other section's `why` where it is.
- A hunk belongs to the first section that cites it. Every hunk should be cited once:
  the viewer shows uncited hunks to the reviewer as "no section".
- A citation of a file or hunk that is not in the diff makes the bundle fail to load.

## Items

- `kind` is `finding` (a problem), `decision` (a choice the change made where the
  requirements were silent, for the reviewer to accept or reject) or `good` (a right
  call that is not obvious from the diff). Judgments go in items, not in a section's `why`.
- `id` is unique in the bundle. The reviewer's verdicts are stored against it.
- Items show under their section in the order they are written: most severe first.
- `section` is the index of the section that owns the line to fix. Leave it out for
  an item that spans the whole change.
- `at.line` is a line number in the new version of the file. The viewer scrolls the
  diff to it, so it has to be a line inside one of the section's hunks. When the line
  to fix is unchanged code outside every hunk, point `at` to the nearest changed line
  and name the real file and line in `body`.
- `severity` is free text shown next to the kind. Leave it out when there is none.
- `status` is left out until someone checks the item. `confirmed` means it was
  reproduced and has `observed` evidence. `unreproduced` means the check did not
  show it: the item stays, with evidence saying what was run. `resolved` means an
  earlier guide raised it and the current head fixes it.

## Evidence

`provenance` says how much the reviewer can lean on it:

- `observed`: output of something that was actually run (a screenshot, a test run,
  a log). Give `repro`, the command or steps that produce it again.
- `checked`: a machine verified it (a model checker, an assertion that ran).
- `inferred`: reasoned out by the author of the guide (a diagram, a trace, an
  explanation). Never label something `observed` that was not run.

`path` is relative to the bundle directory. Images render inline; anything else opens
with the `o` key. `revui diagram <file.mmd> <out.png>` renders Mermaid to an image and
`revui shot <url or html file> <out.png> [WIDTHxHEIGHT]` screenshots a page in headless
Chrome. An entry can carry both a `path` and `text`.
`text` is shown inline as is, and the viewer wraps it. Keep whole lines: shorten output by choosing lines (`grep`, `head`), never by cutting characters (`cut -c`), which leaves lines that end mid-word. Short command output can go in `text` with no `path`,
whatever the provenance.

## A merge request that moved on

Each bundle is for one head commit. When the head moves, `revui fetch` makes a new
bundle and, if an earlier head had a guide, writes its directory into a file named
`previous`. Update instead of starting over:

- Start from the earlier `review.json`. Keep every item's `id`.
- Look at what changed between the two heads (`git diff <earlier head>..<new head>`)
  and review that. Re-place every section's hunks against the new `mr.patch`.
- Check each earlier finding at the new head: still there, changed, or fixed. A
  fixed one stays with `status` `resolved` and one line saying what fixed it.
- New findings get new ids.

## Check

`revui check <dir>` prints `ok` and exits 0 for a clean bundle. It prints `not ok`
and exits 1 when a hunk is in no section, an evidence file is missing, or an item's
`at` is outside its section's hunks. It fails with an error when the bundle does not
load. Run it after writing `review.json`.
An open viewer reloads by itself when `review.json` changes.
