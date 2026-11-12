# Configuring revui

Everything lives in one JSON file, `~/.config/revui/config.json`. `,` inside
revui opens it in a tab, and `revui doctor` checks it. Every key is optional:
with none, pasting a link reviews it with Claude Code in a tab.

| Key | What it sets | Default |
| --- | --- | --- |
| `claude` | The command that starts Claude Code, available to every command as `$REVUI_CLAUDE`. | `claude` |
| `agents` | The coding agent that reviews, and that `t`, `v` and `i` ask. | Claude Code in a tab |
| `generate`, `generate_modal` | A command of your own that writes guides, in place of the review's agent, and whether it runs in a tab. | not set |
| `open`, `open_more`, `open_modal` | What `o` runs to open a file. | the line on the web |
| `drive` | How agents should drive a browser or app on this machine. | they pick a background tool |
| `accent` | The highlight colour: a name or a hex value such as `"#d77757"`. | `cyan` |

`REVUI_HOME=<dir>` keeps the config and all data in that one folder instead, for
a second setup or a sandbox.

Commands you configure get their data through environment variables, never by
pasting text into the command line:

| Variable | Holds |
| --- | --- |
| `$REVUI_URL`, `$REVUI_REPO`, `$REVUI_SHA` | the merge request, its project path, its head commit |
| `$REVUI_DIR` | the bundle directory |
| `$REVUI_FILE`, `$REVUI_LINE`, `$REVUI_FILE_URL` | the file and line under the cursor, and its link on the web |
| `$REVUI_HEAD_FILE`, `$REVUI_BASE_FILE` | that file at the head and before the change, fetched read-only |
| `$REVUI_PROMPT`, `$REVUI_ITEM` | what revui would say to an agent, and the finding it is about |
| `$REVUI_SESSION_ID`, `$REVUI_SESSION` | a new agent's id, and whatever its `run` command printed |
| `$REVUI_SOCKET` | a socket path that is stable for the session, for editors that take remote commands |
| `$REVUI_CLAUDE` | the `claude` setting |

## Your reviews

revui opens on a paste box over the reviews you have started, the latest first.
Paste a merge request or pull request link, or run `revui <link>`, and the review
starts. Typing a few letters filters the list. There is nothing to configure: the
list is the bundles under `~/.local/share/revui/`, one per head commit, and a
merge request shows once however many heads it has had.

Opening a review asks the code host whether the head moved. When it did, the
earlier guide opens with an offer to update it for the new commits.

## Writing guides

Pasting a link starts the review's agent on it. `g` does the same in a review
that has no guide, and regenerates one that has. The agent is the first entry of
`agents`, in its tab: it is told to run `revui skill guide` and follow it, so no
skill has to be installed and any coding agent can do it. `ctrl-q` shows the
agent, which may ask you to allow a command, and goes back to the review.

The guide is written in stages. The summary and the parts land first, then each
finding as it is found, and the open review picks each one up without moving you
from the card you are on. The top line says what the agent is on until it is done.

To write guides with something else (a team's own review skill, a CI job), set
`generate` to a command. It gets the merge request in `$REVUI_URL` and the bundle
in `$REVUI_DIR`, and runs in the background unless you add `"generate_modal": true`:

    "generate": "tmux new-window \"claude '/revui-guide $REVUI_URL'\""

Anything can write a guide: `revui format` prints the format, and
`revui check <dir>` validates the result.

## Agents

Each review has one agent: the one that wrote its guide. `t` goes to it, already
told what you are looking at, so you can ask it about the change. `v` asks it to verify the finding
under the cursor (or a section's unproven findings) by running something, and to
record the result in the guide: `confirmed` with observed evidence, or
`unreproduced`. The open review reloads when the agent saves. To verify specific
claims, pick evidence entries first with `space` or a click: the detail pane's
bottom edge shows how many are picked, and `v` sends just those.

Pressing `t` or `v` again goes to the agent that is already running for this
merge request, or picks its conversation up again if it was quit. `T` and `V` let
you choose instead: that agent, or a new one in any of the places you configured.
An agent that is still writing a guide keeps running when you go back to your
reviews; one that is idle is closed, and resumed the next time you ask.

Where an agent opens is the `agents` list in the config. The first entry is the
default. Out of the box there is one, which runs Claude Code in a full-screen tab
over the review. `ctrl-q` goes back to the review while it keeps working, and
`ctrl-q` again, or the tab's number, brings the tabs forward. Keep `ctrl` held and
press `q` again to step through the tabs; let go, and the next `ctrl-q` goes back
to the review. That needs a terminal that reports key releases (`revui probe` says
whether yours does). In one that doesn't, `ctrl-q` only switches between the tabs
and the review: click a tab or press its number to change tabs.

    "agents": [
      { "name": "agent", "modal": true,
        "run": "$REVUI_CLAUDE --session-id \"$REVUI_SESSION_ID\"",
        "resume": "$REVUI_CLAUDE --resume \"$REVUI_SESSION_ID\"" }
    ]

- `run` starts a new agent. revui then types a one-line prompt into its input
  (which guide, and what you are looking at) and leaves it for you to edit and
  send. A `run` that passes `$REVUI_PROMPT` itself sends it straight away instead.
- `modal: true` runs it in a tab over the review. Follow-ups are typed into that
  tab. If you quit the agent, `resume` brings the same conversation back the next
  time, in the directory it started in, even after restarting revui.
  `$REVUI_SESSION_ID` is a UUID revui makes for each new agent.
- Without `modal`, the command must return at once (a split, a tab, a window).
  `send` then delivers follow-ups to it: whatever `run` printed is in
  `$REVUI_SESSION`, so print the pane or session id there. Leave `send` out and
  every press starts a new agent.

A tmux split that is reused:

    { "name": "split",
      "run": "tmux split-window -h -P -F '#{pane_id}' -e \"REVUI_PROMPT=$REVUI_PROMPT\" 'claude \"$REVUI_PROMPT\"'",
      "send": "tmux send-keys -t \"$REVUI_SESSION\" \"$REVUI_PROMPT\" Enter" }

Agents learn how to read and update a bundle from `revui skill`, which the prompt
tells them to run. `revui skill install` also installs it as a Claude Code skill,
so any session can work on a bundle when you ask.

## Opening files

`enter` opens the file under the cursor. The default opens it on the web at that line
(`$REVUI_FILE_URL`). Set `open` to use an editor on a local clone. It gets
`$REVUI_FILE`, `$REVUI_LINE`, `$REVUI_REPO` (the project path) and `$REVUI_SHA`:

    "open": "nvim +$REVUI_LINE ~/src/$REVUI_REPO/\"$REVUI_FILE\""
    "open": "code -g ~/src/$REVUI_REPO/\"$REVUI_FILE\":$REVUI_LINE"

A local clone shows the branch it has checked out, which may not be the merge
request's head. `$REVUI_HEAD_FILE` is the file as it is at the head and
`$REVUI_BASE_FILE` the same file before the merge request: revui fetches them
into the bundle, read-only, the first time a command asks. Open both in your
editor's diff mode to see the change in context.

With `"open_modal": true` the command runs in a terminal drawn over the review
instead of taking the screen. `ctrl-q` hides it and `enter` shows it again. To
collect several files in one editor, add `open_more`: it runs when the editor is
already open, with `$REVUI_SOCKET` naming a socket path that is stable for the
session. For Neovim, which then shows each file as a tab:

    "open_modal": true,
    "open": "nvim --listen \"$REVUI_SOCKET\" +$REVUI_LINE \"$REVUI_HEAD_FILE\"",
    "open_more": "nvim --server \"$REVUI_SOCKET\" --remote-tab \"$REVUI_HEAD_FILE\""

## How agents drive the product

`v` and `i` ask the agent to reproduce something, off your screen. Two things
decide how.

**Where it runs** is per repo, and the agent asks once: the first time a check
needs the product running, it asks where the change runs (a local stack, or a
preview deploy of the branch) and where today's behaviour is read from (the base
commit locally, or staging or production, read only), with what it found in the
repo and on the merge request as suggestions. The answer is kept under
`## Evidence` in that repo's notes, `~/.local/share/revui/notes/<repo>.md`, which
you can edit. Saying otherwise in a request ("check this one on staging") wins for
that request. A deployed environment only has merged code, so it is never used to
show what an unmerged change does.

**What drives it** is per machine. Say it in `"drive"`: one or two sentences naming
what to use here (your terminal app's browser, Playwright, `agent-desktop`, a
local-stack skill). Agents read both with `revui drive`. Unset, they pick a
background tool they can find and ask before anything that would take the screen.

The how is a second bundled skill, `revui-drive` (`revui skill drive` prints it,
`revui skill install` installs it with the other). It is generic: it finds the
local stack from the repo, picks a driving tool from what `revui drive` reports on
the machine, and adds what it learns to the same notes file, so the start command
and sign-in are worked out once. A project's own skill for
running its product still wins when there is one.

A third bundled skill, `revui-guide`, is what `g` runs by default: it reviews the
merge request or pull request and writes the guide. Point `generate` at your own
review skill when you have one that follows `revui format`.

## Images

Evidence screenshots render inline in terminals with Kitty, Sixel or iTerm2
graphics, and as block characters elsewhere. `REVUI_IMAGES=halfblocks` forces
block characters.
