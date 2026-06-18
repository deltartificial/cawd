# cawd

```
   ██████╗ █████╗ ██╗    ██╗██████╗
  ██╔════╝██╔══██╗██║    ██║██╔══██╗
  ██║     ███████║██║ █╗ ██║██║  ██║
  ██║     ██╔══██║██║███╗██║██║  ██║
  ╚██████╗██║  ██║╚███╔███╔╝██████╔╝
   ╚═════╝╚═╝  ╚═╝ ╚══╝╚══╝ ╚═════╝
```

Code Aware Workspace Display - Terminal file explorer with syntax highlighting.

![cawd](asset/image.png)

## Why cawd?

cawd is designed for **code reading**, not code editing. The idea is simple: when you're using AI coding assistants like Claude Code, Cursor, or Copilot, you need a way to visually verify the code being generated in real-time.

Instead of constantly switching between your terminal and an IDE, cawd gives you a lightweight, fast, and always-visible window into your codebase. Run it in a split terminal alongside your AI tool and watch the changes as they happen.

**Use case:** Split your terminal in two — one side for Claude Code generating code, the other side for cawd to inspect what's being written.

## Install

```bash
cargo install --path .
```

## Usage

```bash
cawd [path]
```

## Keybindings

| Key | Action |
|-----|--------|
| j/k | Navigate |
| h/l | Collapse/Expand |
| Enter | Open file |
| Ctrl+P | Search files |
| / | Filter/Search |
| Mouse drag | Select lines (code viewer) |
| c | Comment selected lines |
| Tab | Switch panel |
| q | Quit |

## Annotations & Review

In the code viewer, drag with the mouse to select one or more lines, then press
`c` to write a comment. The annotation is saved to a timestamped markdown file
under `.cawd/` at the project root, capturing the file path, line range, the
selected code excerpt, and your comment — handy for reviewing AI-generated code.

Once saved, the annotated lines are highlighted directly in the code viewer:
the line range gets a status-colored background and the comment is shown inline
on the first line (amber = open, blue = in progress, green = done), so you can
see at a glance which lines a comment refers to while reading the file.

The **Review** tab (cycle with `Tab`, or press `3`) lists every annotation with
a status badge (○ open · ◐ in progress · ● resolved). Resolved annotations are
hidden by default — the title shows how many are *done* and `a` reveals them:

| Key | Action |
|-----|--------|
| j/k | Navigate annotations |
| Enter | Open the annotated file at its lines |
| w | Dispatch a worker on the annotation |
| s | Cycle status (open → in progress → resolved) |
| d | Delete the annotation |

Pressing `w` launches a **headless Claude Code worker** (`claude -p … --dangerously-skip-permissions`)
from the project root that picks up the task built from the comment, code excerpt
and line range. The annotation moves to *in progress*; when the worker exits
cleanly it is marked *resolved* automatically (otherwise it returns to *open*).
Worker output is streamed to `.cawd/logs/<id>.log`.

> Note: workers edit files directly in the repository without confirmation. They
> run as child processes of cawd, so quitting cawd stops any in-flight workers.
