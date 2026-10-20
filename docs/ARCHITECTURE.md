# VCE architecture

VCE is a Tauri 2 desktop app. It has a Rust backend, a React/TypeScript frontend, and a hidden PowerShell process. This document maps the four domains from the development plan to the code. It also records where the implementation deliberately differs from the plan, and why.

## Domains and where they live

| Plan domain | Backend (Rust, `src-tauri/src`) | Frontend (`src`) |
|---|---|---|
| 1. Presentation layer | – | `components/` (explorer, terminal, search, suggestions, ai), `a11y/`, `state/store.ts`, `shortcuts.ts` |
| 2. Shell controller | `shell/` (session, input tracking, quoting, integration script) | `components/terminal/TerminalPane.tsx`, `hooks/useBackendEvents.ts` |
| 3. Command index & semantic search | `index/` (catalog, inventory, embedding, search, fuzzy) | `components/search/CommandPalette.tsx` |
| 4. Lazy-loaded LLM | `ai/` (model, server, prompt) | `components/ai/AiPanel.tsx` |
| Cross-cutting | `assist/` (error rules, risk assessment, templates), `fs_ops.rs`, `settings.rs`, `error.rs` | `commands.ts` (insert/run with confirmation), `focusRegistry.ts` |

Each Rust module has unit tests next to the code (`cargo test`). Frontend logic and components are tested with Vitest (`npm test`).

## Shell controller: ConPTY plus a side channel

- **Pseudo-console.** PowerShell runs in a Windows pseudo-console (ConPTY, via `portable-pty`). It does not use plain pipes. With pipes, PowerShell's line editor, colours and every interactive program (python REPL, ssh, vim) break.
- **State comes through a private pipe, not the output.** ConPTY merges stdout and stderr and re-renders the screen, so neither "intercept stderr" nor "regex on stdout" (from the plan) is reliable. Instead, `shell/integration.ps1` runs inside the user's session after their profile and wraps their `prompt` function. On every prompt it sends one JSON line over a named pipe that only this app can connect to (`\\.\pipe\vce-<pid>-<uuid>`). The line holds the working directory, whether a command ran, `$?`, `$LASTEXITCODE` and the newest `ErrorRecord`. The script is passed with `-EncodedCommand`, so it works under any execution policy without changing that policy.
- **Directory sync.**
  - *Terminal → tree:* a prompt event with a new `cwd` refreshes the tree and is announced to screen readers.
  - *Tree → terminal:* the backend types a visible `cd` into the shell (`shell/quote.rs` builds it safely). The resulting prompt event then updates the tree. The terminal stays the single source of truth.
- **Never clobbering the user.** `shell/input_state.rs` tracks whether the shell is at an empty prompt. GUI-initiated commands are refused, with an explanation, while a command runs or the user has unsent text on the line. Terminal-generated reports (focus events, cursor reports) don't count as typing.
- **Failure capture.** Error text for native programs (npm, git, python) is read from xterm's rendered buffer between the command's start marker and the next prompt (`components/terminal/outputCapture.ts`). That text and the PowerShell `ErrorRecord` go to `assist/errors.rs`, which returns at most three fixes. Fixes are shown in the suggestion bar and are never run automatically.

## Command index

- **Inventory.** The PATH crawl (`index/inventory.rs`) is combined with the cmdlet, function and alias list from a hidden `powershell -NoProfile` (cached in the app cache folder, refreshed each launch).
- **Catalog.** The curated catalog (`resources/catalog.json`, 375 entries plus winget IDs for common tools) supplies plain-English descriptions, keywords and parameter help. An entry is shown only if every command it `requires` is installed.
- **tldr-pages examples.** About 5,000 bundled pages (generic and Windows) become example recipes, but only where they work in PowerShell on this machine. Rules in `index/search.rs`:
  - **Windows pages** apply to installed cmdlets and programs.
  - **Generic pages** describe Unix tools, so they apply only to installed programs that aren't Windows' own namesakes in System32. Allowed exceptions: Microsoft's ports of `curl`, `tar` and the ssh tools.
  - **cmd.exe built-in pages** (`dir /a`, `copy`) never apply: in PowerShell those names are aliases with different options.
  - **Alias rewriting.** When a PowerShell alias shadows a program (`curl` → Invoke-WebRequest in 5.1), examples are rewritten to the executable's name (`curl.exe`).
  - **Dropped examples:** ones using Unix-only syntax (`sudo`, `&&`, `$(...)`, `/dev/`), or piping into tools that aren't installed.
  - **Placeholders.** `{{...}}` placeholders become the same `<placeholder>` templates as the catalog, so values are quoted safely. Option alternatives (`{{[-m|--message]}}`) use the readable long form.
- **Cmdlet help.** Each cmdlet shows its default parameter set. Reading parameter sets needs every module loaded (about 50 s, several hundred MB in a hidden below-normal-priority PowerShell), so the result is cached in `ps-usage-v1.json`. It is rebuilt only when it is missing, more than a week old, or the user chooses "Check for newly installed programs"; the fast command list (1–2 s, no module loading) still runs every launch. It also shows its one-line description from installed help files, which `index/help.rs` reads straight from the MAML XML because `Get-Help` takes about 0.2 s per command. Windows PowerShell ships with little help, so Settings offers "Install PowerShell help", which runs `Update-Help` visibly in the terminal (in an administrator window on 5.1).
- **Embeddings.** Semantic matching uses static model2vec embeddings (`minishlab/potion-base-8M`). The WordPiece tokenizer is reimplemented in `index/embedding.rs`, so there is no tokenizer or ML runtime dependency. The table is quantised to i8, about 7.5 MB of RAM. Encoding a query takes microseconds.
- **No vector database.** About 3,000 documents fit in memory, so brute-force cosine search takes under a millisecond. sqlite-vss, named in the plan, is deprecated.
- **Ranking** = 0.6 × semantic + 0.4 × IDF-weighted keyword overlap, adjusted by:
  - a source prior (curated recipes above bare cmdlet and program names);
  - a bonus for exact command names;
  - a small simplicity prior that favours shorter commands for beginners.
- **Degradation.** Without the model, search falls back to keywords. Without the catalog, it falls back to names. The terminal is unaffected either way.

## AI command generation

- **Separate process.** `llama-server.exe` (prebuilt llama.cpp, pinned build, bundled in `resources/llama`) is started for a single request and killed when it answers, fails or is cancelled. Killing the process returns all model memory to the OS. A Windows Job Object with kill-on-close guarantees the server dies even if VCE crashes. It listens only on 127.0.0.1 with a random port and API key, and runs with `--offline`.
- **Model.** Qwen2.5-Coder-0.5B-Instruct Q4_K_M, downloaded only after the user agrees in the UI and checked against a pinned SHA-256. Users can pick any GGUF file instead.
- **Grounding.** The prompt (`ai/prompt.rs`) includes the OS, PowerShell version, current folder and its first 40 items, installed notable tools, and the four most relevant catalog recipes. JSON-schema constrained decoding guarantees a parseable reply.
- **Review.** The reply is post-processed:
  - flattened to one line;
  - `&&` rewritten for PowerShell 5.1;
  - each program name checked against the inventory, with a warning for possibly hallucinated commands;
  - risk-assessed.

  It is then shown in an editable review panel. Running it is a separate action.

## Safety model

- Every command the app proposes passes through `assist/risk.rs`, which uses conservative patterns. Permanent deletion, download-and-execute, registry and security changes and destructive git operations are rated **danger**.
- **Danger** commands need a second confirmation (`RunConfirmDialog`). Run always re-assesses the exact text being run, so an edit made after a stale rating can't skip the confirmation. If the assessment itself fails, the command is treated as dangerous.
- Anything VCE types into ConPTY with a control character in it is refused. Keystrokes like DEL (Backspace) or Esc in a folder name could otherwise undo the quoting.
- Paths taken from command output are only touched on the filesystem if they're local drive paths. Merely probing `\\host\share` makes Windows send the user's credentials to that host.
- Values lifted from output (git branch names, npm and pip package names) are restricted to their legitimate character sets before they're placed in a fix command.
- "Put on command line" is always offered as the review-first alternative.
- Template values are quoted (`shell/quote.rs`) so user input is always one literal argument.
- File tree deletes go to the Recycle Bin. Moves and renames use `MoveFileExW` without the replace flag, so Windows itself refuses to overwrite even if a file appears between VCE's check and the move. Moving two same-named items into one folder is refused.

## Accessibility

- Live regions (`a11y/`) announce folder changes (coalesced), failures with suggestion counts, search result counts and AI progress.
- The tree follows the WAI-ARIA tree pattern: roving tabindex, arrow keys, type-ahead, Shift+F10 context menu, F2 rename. Cut and paste (Ctrl+X, Ctrl+V) is the keyboard alternative to drag-and-drop.
- Search follows the combobox/listbox pattern. Dialogs trap focus and restore it when they close.
- Every feature has a keyboard path. F6 cycles between areas. All shortcuts are in `shortcuts.ts`, which also feeds the help dialog.
- The terminal's screen-reader mode turns on automatically when Windows reports a screen reader (`SPI_GETSCREENREADER`).
- Forced-colours (High Contrast) mode is supported. Risk is always stated in words, never by colour alone.

## Memory and CPU

Measured on the release build at idle, Windows 11, private bytes, with about 2,400 tldr examples indexed:

| Process | Private memory |
|---|---|
| VCE backend (Rust) | 30.8 MB |
| WebView2 (all processes) | 114–117 MB (varies between runs) |
| **VCE total** | **145–148 MB** (budget 150 MB) |
| PowerShell 5.1 + conhost (the user's shell) | 76.8 MB |

- **WebView2 flags.** `tauri.conf.json` passes WebView2 `--disable-gpu`, `--disable-gpu-compositing`, `--renderer-process-limit=1` and `--disable-background-networking`. That cuts WebView2 from about 199 MB to 110 MB private. The terminal uses xterm's DOM renderer, so it doesn't need the GPU process.
- **Idle CPU** is about 0.2% of one core. Everything is event-driven with no polling; the file tree refreshes after each command and when the window gains focus.
- **Backend memory.** The backend holds the embedding table (~7.5 MB, i8-quantised), the search index (a few MB) and a 256 KB terminal backlog. Index documents store their vectors as i8 with one scale each, and their search terms as ids into one shared term table; with f32 vectors and per-document string sets, the tldr examples alone cost about 10 MB.
- **AI memory.** The AI engine uses memory only during a generation: llama-server peaked at about 94 MB private, and the 491 MB model is memory-mapped from disk. It is gone as soon as the command is written.

## Robustness

- **Panics stay contained.** Release builds unwind instead of aborting, so a panic in a background thread (such as the command indexer) can't take down the terminal. The indexer's panic becomes a visible "search unavailable" state.
- **Broken shell connections end visibly.** If the shell's input or output pipe breaks, VCE stops the shell and shows why, with a Restart button. It doesn't leave a terminal that looks ready but ignores input.
- **Fresh PATH.** New shells and the command index read PATH from the registry, so programs installed while VCE is open are found after a restart or refresh.
