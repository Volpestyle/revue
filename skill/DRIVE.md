---
name: revui-drive
description: >-
  Run a product locally and drive its browser or desktop UI in the background, to reproduce a claim or capture a screenshot or recording for a revui guided review, using whatever this machine has (Herdr Browser, Orca, Playwright, headless Chrome, agent-desktop). Use when verifying a finding or making an artifact means running the product. A project skill for running or driving that product wins over this one.
---

# Driving the product for a review

The job: reproduce one claim in the running product and capture what happened, without touching the person's screen, pointer or keyboard.

## 1. Find out what is here

Run `revui drive <bundle dir>`. It prints:

- **The person's preference**, if they set one. Follow it over everything below.
- **The driving tools on this machine**, each with the command that prints its own manual.
- **Notes for this repo** left by earlier runs: how to start it, sign in and reach a feature.

Then check for a project skill or doc that runs or drives this product (your skill list, the repo's `CLAUDE.md` or `AGENTS.md`). If one exists, use it for the product-specific steps and use this skill only for what it leaves out.

## 2. Pick the tool

Take the first that `revui drive` found and that fits the target. Read the tool's own manual before the first call: do not guess flags.

| Target | Tool | Why it is safe |
| --- | --- | --- |
| Web page, inside Herdr Browser | `herdr-browser` (its in-app browser, the `agent` profile) | A pane in the app, not the person's browser |
| Web page, inside Orca | `orca` (its embedded browser: `orca tab create`, `orca snapshot`, `orca click`), in a profile made for the run | Same |
| Web page, anywhere | `agent-browser`, `playwright-cli` or Playwright, headless | No window |
| Web page, nothing else | Chrome or Chromium with `--headless=new --user-data-dir=<temp dir>`, or `revui shot` for a page that needs no sign-in | No window, own profile |
| Native desktop app | `agent-desktop`, without `--headed` | Its element actions do not move the pointer or take focus |

Never attach to the browser or profile the person uses, and never bring a window to the front. If the only way left is visible and takes focus, stop and ask.

## 3. Get the product running locally

- Start from the notes `revui drive` printed. With none, read the README, `Makefile`, `package.json` scripts, compose files, `.env` examples and the end-to-end test config: together they give the start command, the base URL and how tests sign in.
- Run the merge request's head in a scratch worktree, never in the person's checkout. Before starting anything, check whether the ports are taken: a stack the person is running is theirs. Use it only to read, and only if it is on the right commit.
- Sign in with the test credentials the repo provides (env files, test fixtures). Never use the person's own account or password manager unless they say so, and never write a secret into the bundle.
- Use a shared environment (dev, UAT) only when the claim is about that environment or the person said to.

## 4. Drive and capture

- Decide from what the page says (an accessibility snapshot or its text), not from a picture. Take the screenshot afterwards, as the evidence.
- For a before and after, run the base and the head and capture both.
- Save captures under the bundle's `assets/`, and add them to the item as `observed` evidence with the exact steps as `repro` (see the `revui` skill). If you could not reach the state the claim is about, say so: a screenshot of something nearby is not evidence.

## 5. Leave it as you found it

- Close the browsers and stop the servers you started. Remove the scratch worktree.
- Append what you learned to the notes file `revui drive` named: the start command, the URL, how sign-in works, what tripped you up. Short facts, no secrets. The next run reads them, so nobody works it out twice.

## When nothing fits

Say what is missing and what would fix it (for example: no headless browser found, `npx playwright install chromium` would add one). Do not fall back to controlling the visible screen, and do not label anything `observed` that was not captured from a real run.
