# Setting up revui for someone

You are an agent setting up `revui`, a terminal viewer for guided code reviews, for the person you are working with. Everything it needs lives in one JSON file. Work through these steps, ask where it says to ask, and finish when `revui doctor` exits 0.

Run `revui doctor` first. It prints the config file's path and what is missing. If the config file already exists, read it and change only what is needed: never drop a key the person set.

## 1. Which merge requests to list

Ask which projects they review, and on which host. To suggest some, look at what they are already involved in:

    glab mr list --reviewer=@me -F json -P 50
    gh search prs --review-requested=@me --state=open

Write them into `repos`: a GitLab project as its full path, a GitHub repo as `github.com/owner/repo`. If everything is on GitHub, set `"host": "github"` and write plain `owner/repo`. Use `groups` only when they want every project under a GitLab group: it lists up to 100 merge requests per group.

    { "repos": ["group/subgroup/project", "github.com/owner/repo"], "groups": [] }

## 2. Where an agent opens

`t` (ask) and `v` (verify) start a coding agent from the `agents` list. The first entry is the default. With no `agents` key, revui runs Claude Code in a tab inside itself, which works everywhere, so only write this key when the person wants something else. Ask which coding agent they use and where they like it to open.

If they start Claude Code through their own command (a wrapper or another account), set `"claude": "<that command>"`. The defaults run it, and any command can use it as `$REVUI_CLAUDE`. It must be a program on PATH, not a shell alias.

For an agent in a tab inside revui, leave the prompt out of the command: revui types its short prompt into the agent's input and the person edits and sends it. A command that does use `$REVUI_PROMPT` sends it as the first message straight away. Always quote it as `"$REVUI_PROMPT"`: the text contains quotes and spaces. `$REVUI_SESSION_ID` is a fresh UUID for each new agent.

- A tab inside revui (the default shape), with resume after the agent is quit:

      { "name": "agent", "modal": true,
        "run": "claude --session-id \"$REVUI_SESSION_ID\"",
        "resume": "claude --resume \"$REVUI_SESSION_ID\"" }

- A tmux split that is reused (`$TMUX` is set when they are in tmux). `send` gets whatever `run` printed in `$REVUI_SESSION`:

      { "name": "tmux split",
        "run": "tmux split-window -h -P -F '#{pane_id}' -e \"REVUI_PROMPT=$REVUI_PROMPT\" 'claude \"$REVUI_PROMPT\"'",
        "send": "tmux send-keys -t \"$REVUI_SESSION\" \"$REVUI_PROMPT\" Enter" }

- Another multiplexer or terminal app: find its command for "open a pane running this program" and "type this text into that pane", and write `run` and `send` the same way. Without `modal`, `run` must return at once. Test the command by hand with a harmless prompt before writing it.

If their agent is started through a wrapper script, check how the wrapper passes arguments: a flag that takes an optional value can swallow the prompt, and `--` before the prompt prevents that.

## 3. Opening files

`enter` runs `open` for the file under the cursor. With no `open` key it opens the line on the web. Ask which editor they use. The command gets `$REVUI_HEAD_FILE` (the file at the merge request's head, fetched read-only), `$REVUI_BASE_FILE` (the same file before the merge request, empty when it is new), `$REVUI_FILE`, `$REVUI_LINE`, `$REVUI_REPO`, `$REVUI_SHA` and `$REVUI_FILE_URL`.

- A terminal editor, drawn over the review, collecting files as tabs (Neovim shown):

      "open_modal": true,
      "open": "nvim --listen \"$REVUI_SOCKET\" -d \"${REVUI_BASE_FILE:-/dev/null}\" \"$REVUI_HEAD_FILE\" -c \"wincmd l | $REVUI_LINE\"",
      "open_more": "nvim --server \"$REVUI_SOCKET\" --remote-send \"<C-\\\\><C-N>:tabedit $REVUI_HEAD_FILE | diffthis | leftabove vsplit $REVUI_BASE_FILE | diffthis | wincmd l | $REVUI_LINE<CR>\""

  This opens the file as a side-by-side diff: `$REVUI_BASE_FILE` is the file before the merge request, `$REVUI_HEAD_FILE` after. Drop the base file to open the new version alone.

- A graphical editor on a local clone. Ask where their clones live:

      "open": "code -g \"$HOME/src/$REVUI_REPO/$REVUI_FILE\":$REVUI_LINE"

## 4. Generating guides

`g` runs `generate` with the merge request in `$REVUI_URL`. The default runs Claude Code with `/revui-guide $REVUI_URL` in a tab inside revui, using a skill revui bundles:

- Run `revui skill install`. It installs three skills: `revui-guide`, which reviews a merge request or pull request and writes its guide; `revui`, for reading and updating bundles; and `revui-drive`, for running the product and driving its UI in the background.
- If their team has its own review skill, it can write guides too by following `revui format`. Then set `generate` to call it.

If they want the run somewhere else, point `generate` at the same kind of pane command as in step 2, with `/revui-guide $REVUI_URL` as the prompt. A custom `generate` runs in the background; add `"generate_modal": true` to keep it in a revui tab.

## 5. How agents drive a browser or app

`v` (verify) and `i` (artifact) can have an agent reproduce something in the running product. Ask how that should happen on this machine without taking over their screen, and write it as one or two plain sentences in `"drive"`. Agents read it with `revui drive`. Look at what they have: a terminal app with its own browser (Herdr Browser, Orca), Playwright or headless Chrome, `agent-desktop` for native apps, a project skill that runs the local stack.

    { "drive": "Web pages: Playwright, headless. Local stack: the run-local skill. Never the system browser." }

## 6. Finish

Run `revui doctor` until it exits 0. Then tell the person, in a few lines: which repos are listed, where agents and files will open, and that `,` inside revui opens this config again.
