# cawd

Code Aware Workspace Display — the terminal for **reviewing and orchestrating AI-generated code**.

![cawd](asset/image.png)

## Why cawd?

When an AI assistant (Claude Code, Cursor, Copilot…) writes code for you, the bottleneck is no
longer typing — it's **reading, judging, and steering** what gets produced. cawd is built for that
loop: a fast, always-visible terminal where you read the generated code, mark what's wrong, and fire
off fixes without ever leaving the keyboard.

Run it in a split terminal next to your AI tool and treat it as your **control tower**: watch files
change in real time, annotate the parts that need work, and dispatch workers to resolve them — then
verify the result, all in one place.

The loop cawd is designed around:

1. **Read** — browse the tree and read generated code with syntax highlighting.
2. **Track changes** — see every file that was just modified and inspect the diff, side by side.
3. **Annotate** — select the lines that are wrong and leave a comment, pinned right on the code.
4. **Dispatch** — send a headless worker to fix the annotation for you, ultra fast.
5. **Verify** — the annotation flips to _resolved_ when the worker succeeds; you review the result.

## Install

```bash
cargo install --path cawd
```

## Usage

```bash
cawd [path]
```

Switch panels by clicking, pressing `Tab`, or jumping straight to one with the number keys:
`1` Explorer · `2` Changes · `3` Review · `4` the open file · `5` Notion.

## Reviewing changes

The **Changes** panel (press `2`) lists every file you've modified in the working tree and lets you
open a side-by-side diff with additions and deletions highlighted — a quick way to see exactly what
the AI just touched before you accept it, without leaving the terminal.

## Annotate & dispatch

![review](asset/review.png)

In the code viewer, drag with the mouse to select one or more lines, then press `c` to write a
comment. The annotation is saved to a timestamped markdown file under `.cawd/` at the project root,
capturing the file path, line range, the selected code excerpt, and your note.

Once saved, the annotated lines are highlighted **directly in the code** — the range gets a
status-colored background and your comment is shown inline on the first line (amber = open, blue = in
progress, green = done) — so you can see at a glance which lines a note refers to while reading.

The **Review** panel (press `3`) is your task board: it lists every annotation with a status badge
(○ open · ◐ in progress · ● resolved) on top, and the live workers below. Resolved annotations are
hidden by default — the title shows how many are _done_ and `a` reveals them.

## Notion tickets

The **Notion** panel (press `5`) shows tickets pulled read-only from a Notion page, so you can keep
your task board next to the code without switching apps. Set `NOTION_TOKEN` in your environment (an
internal integration with the _Read content_ capability, shared with the page) and cawd fetches the
tickets on a background thread — the UI never blocks on the network.

The panel has three sections: the **ticket list** (top-left), the **workers** pane (bottom-left), and
the selected ticket's **detail** on the right — its properties (status, assignees, priority, dates…)
followed by the page body, fetched lazily as you move the cursor.

`Tab`/`Shift+Tab` cycle focus between the three sections (the focused one is outlined in cyan).
`j`/`k` move the cursor in the focused list, or scroll the content when the detail is focused; `Enter`
steps from the list into the detail, and `Esc`/`h` steps back. Open the ticket in your browser with
`o`. Focus the workers pane and press `Enter` on a worker to open its log in the code viewer.

Only **assigned** tickets are shown by default; press `a` to toggle unassigned ones, `/` to filter by
title, `r` to refresh. The assignee property
is auto-detected (it prefers lead/owner/assignee-style names) — override it with
`NOTION_ASSIGNEE_PROP`. Point the panel at a different page or database with `NOTION_PAGE_ID` (a raw
id or full URL). See `.env.example` for the full setup.

### Dispatch a worker on a ticket

Like the Review panel, you can fire a headless worker at a ticket: select it and press `w`. cawd
launches `claude` from the project root with a **structured prompt** built from the ticket (title,
properties, and page body) that tells the worker to:

1. **Spec** the task — and if it's under-specified, stop and write only a spec to `.cawd/specs/`.
2. **Implement** it, matching the repository's standards (CLAUDE.md / AGENTS.md if present).
3. **Verify** with the project's checks (`make lint` / `make test`, or clippy + nextest).
4. **Report** what it did.

The workers pane shows each running worker (elapsed time, pid) and a short history of finished ones
(done / failed). Output is streamed to `.cawd/logs/notion-<id>.log`. Notion stays **read-only** —
worker state is tracked only locally, nothing is written back to your board.

| Key   | Action                                       |
| ----- | -------------------------------------------- |
| j/k   | Navigate annotations                         |
| Enter | Open the annotated file at its lines         |
| w     | Dispatch a worker on the annotation          |
| s     | Cycle status (open → in progress → resolved) |
| a     | Show / hide resolved annotations             |
| d     | Delete the annotation                        |

Pressing `w` launches a **headless Claude Code worker** (`claude -p … --dangerously-skip-permissions`)
from the project root. It picks up a task built from your comment, the code excerpt and the line
range, and gets to work. The annotation moves to _in progress_; when the worker exits cleanly it is
marked _resolved_ automatically (otherwise it returns to _open_). Worker output is streamed to
`.cawd/logs/<id>.log`.

This is the core idea: cawd turns your review notes into dispatched work and tracks each one through
to done — an orchestration and verification cockpit for AI-assisted coding.

> Note: workers edit files directly in the repository without confirmation. They run as child
> processes of cawd, so quitting cawd stops any in-flight workers.

## Keybindings

| Key           | Action                                     |
| ------------- | ------------------------------------------ |
| 1 / 2 / 3 / 4 | Jump to Explorer / Changes / Review / file |
| j/k           | Navigate                                   |
| h/l           | Collapse/Expand                            |
| Enter         | Open file                                  |
| Ctrl+P        | Search files                               |
| /             | Filter/Search                              |
| Mouse drag    | Select lines (code viewer)                 |
| c             | Comment selected lines                     |
| Tab           | Switch panel                               |
| q             | Quit                                       |
