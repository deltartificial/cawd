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
| Tab | Switch panel |
| q | Quit |
