---
name: revui
description: >-
  Read and update a revui guided-review bundle: the directory the `revui` terminal viewer opens for a merge request (review.json, mr.patch, assets/). Use when a prompt names a revui bundle directory, when asked a question about a guided review or one of its findings, or when asked to verify, add, change or withdraw a finding or decision in one. Not for writing a whole guide from nothing: that is the generating skill's job.
---

# revui bundles

A bundle is one directory holding a guided review of a merge request. A person reads it in the `revui` viewer while you work, one finding at a time, and the viewer reloads whenever `review.json` changes. Write the whole file to `review.json.tmp` and rename it over `review.json`, so the viewer never reads half a file.

## Find and read

- `revui fetch <merge request or pull request URL>` prints the bundle directory for the MR's current head. It creates a diff-only bundle when there is none and never overwrites an existing `review.json`.
- `review.json` holds the sections, findings and decisions. `revui format` prints every field. Read it before you edit.
- `mr.patch` is the diff the guide cites. `revui hunks <dir>` lists its files and hunk indexes.
- Read code at the head SHA in `sources`, not from a working tree, which can be on another branch.

## Update

Edit `review.json` in place. Then run `revui check <dir>` and fix the file until it exits 0.

- Never edit `mr.patch`, and never touch `state.json`: it is the reviewer's read marks and verdicts.
- Keep `title` and `sources` as they are.
- Keep the `id` of an existing item. The reviewer's verdicts are stored against ids, so a renamed id loses its verdict. A new item gets a new id.
- Do not delete an item to withdraw it. Set its `status` to `unreproduced` and say why, so the reviewer sees what was checked.
- Change only what you were asked to change.
- When the person disputes an item, decide first whether it still says something about this change. If it does, rewrite it from nothing for a reader who never saw the old version: what could go wrong, and what to do. Do not answer the old text or explain why it was wrong inside the item. If what is left is how the team's process normally works, withdraw it.

## Verify a finding

Asked to verify, validate or dig into an item:

1. Try to reproduce the claim by running something: a test, a script, a query, or the app when this session can already drive it. Reading the code again is not verification.
2. Reproduced: add an evidence entry with `provenance` `observed`, the output as `text` or a file under `assets/` as `path`, and the command as `repro`. Keep output lines whole: pick the lines that matter, never cut them by character count. Set the item's `status` to `confirmed`. Keep the earlier `inferred` evidence.
3. Not reproduced: set `status` to `unreproduced` and add an evidence entry that says what you ran and what happened instead.
4. Could not run anything: change nothing in the bundle and say what blocked you.

A claim about a race, ordering or timing needs care: a run that passes proves nothing, because the bad ordering may simply not have happened. Force it (a delay, a stub that holds one side, a query between the two steps) in a scratch worktree, never in the person's checkout. If you cannot force it, do not mark the item `unreproduced`: leave the status alone and add evidence saying it could not be decided by running.

Never write `observed` for something that was not run.

Keep the check from spilling over:

- Reproduce it where the person wants runs to happen for this repo: `revui drive <bundle dir>` prints it under "Where evidence comes from", and the `revui-drive` skill says what to do when it has not been asked yet. A check that needs only the code (a test, a script) runs in a scratch worktree at the merge request's head. A deployed environment only has merged code: read today's behaviour from it, never what this change does, and never write to it.
- Stay off the person's screen. Never take their pointer, keyboard or focus, and never drive the window they are working in.
- To run the product or drive its browser or desktop UI, follow the `revui-drive` skill (`revui skill drive` prints it). It starts with `revui drive <bundle dir>`, which lists the person's preference, the driving tools on this machine and the notes earlier runs left for this repo.
- Verifying is the whole job. If the check breaks something or turns up an unrelated bug, stop, say what happened and what state things are in, and leave the fix to the person.

## Artifacts

Most items need none: a picture for a point nobody doubts wastes effort. Make one in two cases.

- The person asks for one for an item. Then use whatever it takes.
- You are writing a guide and an item is hard to follow in text: the flow crosses several components, the outcome depends on ordering, timing or state, the change has many dependencies, or you needed more than a short paragraph to explain it. Then make one without being asked, in a cheap form such as a diagram. Leave the expensive forms (running the app, recordings) for when the person asks.

Pick the cheapest form that shows the impact:

- A flow, a sequence or a state change: write Mermaid to `assets/<name>.mmd` and run `revui diagram assets/<name>.mmd assets/<name>.png`. A diagram you reasoned out is `inferred`. Keep the numbered steps as `text` on the same entry.
- A bug that is a path through states (a lifecycle, a protocol, a job queue): a statechart, when this session has a skill or tool that model-checks one. Render it to an image. What a model checker verified is `checked`.
- A page that loads without signing in: `revui shot <url or html file> assets/<name>.png [WIDTHxHEIGHT]`. It runs headless. A screenshot of a real page is `observed`: say in the caption exactly what was loaded.
- Behaviour before and after the change: the baseline and the change, each from where `revui drive` says this repo's evidence comes from, with a screenshot of each or a recording. This is `observed`, with the steps as `repro`. Use the project's own skills and tools for running and driving the app.
- Output worth keeping (a log, a trace, a test run): save the file under `assets/`.

Attach the file as an evidence `path` on the item, with a caption that says what it shows. Images show inline in the viewer; other files open with `o`. One artifact per item unless asked for more. If none of these is possible here, say what blocked you: do not fake one, and do not label something `observed` that was not captured from a real run.

## Limits

Do not post, push, comment or approve anywhere unless the person asks. The bundle is the only thing you write.
