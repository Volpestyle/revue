---
name: revui-guide
description: >-
  Review a GitLab merge request or GitHub pull request and write the review as a guided-review bundle for the `revui` terminal viewer: the change split into ordered parts, each with why it exists and the findings for its code. Use when asked for a guide or a guided review of a merge request or pull request URL, or with /revui-guide <url>. Read-only: it never posts, pushes or approves.
---

# Writing a guided review

You are given the URL of a merge request or pull request. The result is a bundle the person opens in `revui`: every changed hunk placed in a part, and every problem you found attached to the code it is about.

Read-only, always: never post a comment, push, approve or change the branch.

## 1. Get the change

1. `dir="$(revui fetch <url>)"` creates or finds the bundle for the current head: `mr.patch` and a `review.json` with no sections. Never edit `mr.patch`.
2. Read the description, the linked ticket if there is one, and the conversation already on it (`glab mr view` or `gh pr view`, with comments).
3. Get the code at the head, in a scratch worktree or a fresh clone, never in the person's checkout. Read each changed file whole, not only its hunks, and follow the callers of what changed.

If `$dir/previous` exists, an earlier head already has a guide: read "A merge request that moved on" in `revui format` and update that guide instead of starting over. If `review.json` already has sections for this head, the person asked to regenerate: review again from scratch and write it fresh.

## 2. Review it

Look for problems this change introduces, in this order: does it do what the ticket asks, security and data integrity, runtime behaviour and edge cases, error handling, then consistency with the code around it and needless complexity.

- Every finding needs evidence from the code at the head: the file and line, and the path that makes it go wrong. Trace a realistic input before calling something a bug.
- A risk you cannot settle by reading (concurrency, ordering, another system's behaviour) is a question, with what you checked and why it stays open.
- Know how this repo releases before faulting what the pipeline did not cover: read its CI config. "This was not tested in staging" is not a finding when staging only ever sees merged code.
- Note what the change does well where it is worth knowing, and any choice it made that the ticket did not ask for.
- Leave out taste, and anything that was already there before this change.

Give each finding an id by severity: `B1`, `F1`, `S1`, `Q1`, with `severity` set to exactly `blocker`, `fix`, `suggestion` or `question`. Good calls are `G1`, `G2`.

## 3. Write the bundle

Run `revui format` and follow it. `revui hunks "$dir"` lists every file's hunks by index.

- **Summary**: two short paragraphs at most. What the ticket asks, what the change does (including anything beyond the ask), then what you checked and what you did not.
- **Parts** (`sections`): group hunks that belong together, the core of the change first, supporting code next, low-signal churn (renames, generated files, formatting) last. Each part gets one paragraph on why it exists. Place every hunk: split a file across parts by hunk when its hunks serve different purposes.
- **Items**: each finding under the part whose code it is about, with `at` pointing at the line to fix. In the body, put what breaks, the path to it, and what to do instead, each under its own `# ` heading. A `Q` about a choice the change made where the ticket was silent is a `decision`.
- **Evidence**: only what you have. Output of something you ran is `observed`, with the command as `repro` and whole lines of output. Something you worked out by reading is `inferred`. Never mark as `observed` what was not run.
- **Artifacts**: only for the few items that are hard to follow in text (a flow across several components, an outcome that depends on ordering or state). Use a cheap form, usually a diagram: see the `revui` skill.

Then run `revui check "$dir"` and fix what it reports until it prints `ok`.

## 4. Finish

Say in a few lines what you found, by id, and that the guide is ready in `revui`. Do not paste the guide.
