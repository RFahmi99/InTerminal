# Dev log

## 2026-10-19 — Project setup and toolchain
- **What:** Scaffolded Tauri 2 + React 19 + TypeScript at `D:\InTerminal`. The project was moved off OneDrive so build output doesn't sync. Installed rustup and the VS 2022 C++ Build Tools with winget.
- **Why:** Rust and MSVC were missing. OneDrive sync of `target/` and `node_modules` would slow builds and cause file-lock errors.
- **Files:** `package.json`, `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json`, `scripts/fetch-assets.ps1`.
- **Decisions:** Dependencies are pinned to releases at least two weeks old, Tauri JS and Rust on matching versions (2.12.1). Third-party binaries (llama.cpp b11397, potion-base-8M) are fetched by script with pinned SHA-256 and git-ignored.

## 2026-10-19 — Shell controller (plan phases 1–2)
- **What:** PowerShell runs in ConPTY (portable-pty). `integration.ps1`, passed with `-EncodedCommand`, wraps the user's prompt and sends cwd, `$?`, exit code and the newest ErrorRecord as JSON over a private named pipe. Input goes through an ordered writer thread. An input tracker refuses GUI commands while the shell is busy or the line has unsent text.
- **Why:** With plain pipes, PSReadLine and interactive programs break. ConPTY merges stderr into stdout, so the plan's "intercept stderr" and "regex cd on stdout" can't be done reliably. The side channel is exact.
- **Files:** `src-tauri/src/shell/*`.
- **Tried/rejected:** In-band OSC markers. ConPTY can reorder them relative to text, and older builds strip unknown sequences.

## 2026-10-19 — Command index and semantic search (phase 3)
- **What:** Search combines a PATH crawl, `Get-Command` (cached) and a 375-entry curated catalog (written by a sub-agent and verified against PS 5.1 and winget). The WordPiece tokenizer is reimplemented and the potion-base-8M table is quantised to i8 (~7.5 MB). Ranking is hybrid: semantic + IDF-weighted keywords, plus name bonus and simplicity prior. Each catalog tool also gets an "Install X with winget" entry.
- **Why:** Paraphrase queries ("decompress an archive") need semantics; exact names ("ffmpeg") need keywords. 3k vectors fit in memory, so no vector DB (sqlite-vss is deprecated).
- **Files:** `src-tauri/src/index/*`, `src-tauri/resources/catalog.json`.
- **Tests:** A real-model ranking eval (15 queries, expected command in the top 3) runs in `cargo test`.

## 2026-10-19 — Error interception, risk assessment, AI generation (phases 3–4)
- **What:** `assist/errors.rs` maps failures to at most three fixes: command not found (`.\`, Linux equivalents, winget installs, typos), npm/node, pip, git, execution policy, `&&` on 5.1, Linux flags, path typos. `assist/risk.rs` rates every proposed command. AI starts `llama-server` per request inside a kill-on-close Job Object. Prompts are grounded (cwd listing, installed tools, top catalog recipes) and use JSON-schema decoding. Output is checked for unknown commands and risk-rated, then shown for review.
- **Why:** Lazy loading with guaranteed memory release, crash isolation, and no C++ toolchain needed for llama.cpp bindings.
- **Files:** `src-tauri/src/assist/*`, `src-tauri/src/ai/*`.
- **Measured:** Qwen2.5-Coder-0.5B Q4 writes a correct PS 5.1 command in about 16 s (load + generate), and the process is gone afterwards.

## 2026-10-19 — Frontend
- **What:** Toolbar, WAI-ARIA file tree (type-ahead, context menu, F2 rename, Recycle Bin, cut/paste moves, drag and drop), xterm terminal, suggestion bar, command palette with parameter form, AI panel with consent/download/review, settings, help, live-region announcer, F6 region cycling. Fonts are Atkinson Hyperlegible Next/Mono. Every proposed command appears on the "command tape".
- **Why:** The product is assistive, so every feature needs a keyboard path and screen-reader announcements. GUI file actions show their PowerShell equivalent to teach the CLI.
- **Files:** `src/**`.

## 2026-10-19 — Bugs found by driving the real app over CDP
- **What:**
  - Focus and cursor reports from xterm counted as typing (false "unsent text" refusals); they're now filtered.
  - The first key of a GUI-typed command was occasionally lost early in a session (conhost turns ESC + `c` into Alt+C). A leading-space guard fixes it; 5/5 fresh-session runs now pass.
  - The frontend dropped input while the shell showed as stopped. The new conhost (portable-pty forces `INHERIT_CURSOR`) then waited forever for xterm's cursor reply, hanging restarted sessions. Input is now always forwarded.
  - A normal `exit` was reported as a lost integration (race); fixed with a grace period, and the exit is now emitted before closing the pseudo-console.
  - Catalog `<placeholders>` read as `>` redirection, so every result showed "Caution".
  - "Did you mean" offered obscure aliases (`gci` for `gti`).
  - The AI hallucination check flagged method calls and hashtable keys.
- **Why:** None of these showed up in unit tests; all were reproduced in the running app first, then regression-tested.
- **Files:** `shell/input_state.rs`, `shell/session.rs`, `TerminalPane.tsx`, `assist/risk.rs`, `index/search.rs`, `ai/prompt.rs`.
- **Next:** Measure release-build memory against the 150 MB budget. Address review-agent findings.

## 2026-10-20 — Release build, memory budget, and review fixes
- **What:**
  - Switched ureq to native TLS (SChannel, Windows certificate store). This removed `ring`, whose freshly compiled build script Smart App Control blocked.
  - Release panics now unwind, so a background-thread panic can't kill the terminal.
  - Trimmed WebView2 via `additionalBrowserArgs` (no GPU process, one renderer, no background networking).
  - Fixed 33 findings from three review agents (Rust, frontend, silent failures).
- **Measured (release Rust + production bundle, idle):**
  - VCE Rust process 28.8 MB private.
  - WebView2 dropped from 199 to 120 MB private with the flags, giving 148.5 MB for the app (budget 150 MB).
  - PowerShell itself adds about 77 MB, the user's shell that any terminal carries.
  - Idle CPU about 0.5% of one core.
- **Security fixes:**
  - Refuse control characters in anything typed into ConPTY. DEL in a folder name could otherwise erase the quoting.
  - Never touch UNC paths taken from command output (NTLM leak).
  - Quote every value in fix commands and restrict regex captures to safe character sets.
  - No `&&` rewrite inside quotes.
  - Risk patterns now see commands after quotes and `=`.
  - Moves and renames use `MoveFileExW` without replace, and duplicate names are refused.
  - Run always re-assesses risk on the exact text, failing closed to the confirmation dialog.
- **Robustness:**
  - Pipe failures now end the session with a visible reason.
  - The shell starts even if the integration pipe can't be created.
  - A malformed status message no longer strands the input tracker.
  - Keys typed while a command runs count as unsent text.
  - Stale output from a replaced session is dropped.
  - Attach-or-start happens under one lock.
  - New shells and the command index read PATH from the registry, so new installs are found.
  - Stalled model downloads can be cancelled immediately.
  - A corrupt settings file is kept and reported.
  - The index has a Failed state.
- **Frontend:**
  - One notifier both speaks and shows every error.
  - Focus requests never steal focus from other areas.
  - Focus moves to a neighbour after a delete.
  - Cancel takes focus during AI generation.
  - Search Insert/Run rebuild from current values.
  - Shortcuts don't escape confirmation dialogs.
  - The danger dialog offers "Put on command line".
- **Open:** `npm run tauri build` (installer) is blocked on this machine by Smart App Control, which rejects the newly compiled Tauri build script (OS error 4551). Options: build in CI, or turn off Smart App Control (irreversible without reset; the user's call).

## 2026-10-20 — Cmdlet help and tldr-pages examples in search
- **What:**
  - Every cmdlet now shows its parameters, taken from its default parameter set via `Get-Command` (about 1 s for all 1,583 commands).
  - Cmdlets also show their one-line description from installed help files, parsed straight from the MAML XML.
  - Bundled tldr-pages v2.3 (pinned, SHA-256 verified, CC BY 4.0): generic and Windows pages become example recipes and describe programs and cmdlets.
  - Settings shows help coverage and offers "Install PowerShell help" (`Update-Help`, run visibly in the terminal).
- **Why:** Only the 375 curated recipes had real explanations. About 2,000 cmdlets and every program were bare names.
- **Applicability rules:**
  - Generic (Unix) pages only apply to installed non-System32 programs, plus curl/tar/ssh.
  - cmd.exe built-in pages are excluded.
  - Commands use `curl.exe` where an alias would hijack them.
  - Unix-only syntax and pipes into tools that aren't installed are dropped.
- **Files:** `index/tldr.rs`, `index/help.rs`, `index/inventory.rs`, `index/search.rs`, `index/mod.rs`, `ai/mod.rs` (AI grounding now includes tldr examples), `CommandPalette.tsx`, `SettingsDialog.tsx`, `HelpDialog.tsx`, `scripts/fetch-assets.ps1`.
- **Tried/rejected:**
  - Calling `Get-Help` per cmdlet: about 0.2 s each, so 5 minutes in total, and only 6 of 200 cmdlets had help installed on this machine.
  - Bundling Microsoft's docs: large, with no per-machine accuracy.
- **Tests:** 101 Rust tests. New:
  - applicability rules (curl.exe rewrite, ping, dir, grep and && excluded);
  - MAML parsing;
  - placeholder conversion;
  - the real bundle yields robocopy, curl.exe and tar examples;
  - the ranking eval still passes with tldr mixed in.

  `main.rs` test harness disabled (empty, and Smart App Control blocked it).

## 2026-10-20 — Cache cmdlet parameter lists
- **What:** Split the PowerShell query in two:
  - a fast command list every launch;
  - a slow parameter-usage query that loads every module, cached for 7 days and forced by "Check for newly installed programs".

  Parameter-less cmdlets no longer repeat their name as a "usage" line.
- **Why:** End-to-end testing showed usage lines were empty for most cmdlets. Parameter sets are empty until a command's module is loaded. Loading all modules gives 1,487 of 1,577 cmdlets real parameter lists, but takes about 50 s and about 580 MB in the hidden process, too heavy for every launch.
- **Also:** git-extras subcommand pages (`git magic`, …) now require their own executable, since they applied whenever git was installed. That removed 165 inapplicable examples.
- **Files:** `index/inventory.rs`, `index/mod.rs`, `index/tldr.rs`, `index/search.rs`, `lib.rs`.

## 2026-10-20 — Shrink the search index to fit tldr examples in the memory budget
- **What:** Index documents now store vectors as i8 with one scale each (`CompactVector`) and search terms as sorted ids into a shared `TermTable`, replacing f32 vectors and per-document `HashSet<String>`s. Also fixed two clippy warnings in `tldr.rs`.
- **Why:** With about 2,400 tldr examples indexed, the release build measured 155.4 MB private (Rust 40.7 + WebView2 114.7), over the 150 MB budget. Without the bundle, Rust measured 30.3 MB, so tldr cost about 10 MB. After the change: Rust 30.8 MB, total 145–148 MB (WebView2 varies 114–117 MB between runs).
- **Files:** `index/embedding.rs`, `index/search.rs`, `index/tldr.rs`, `docs/ARCHITECTURE.md`.
- **Tests:** 104 Rust tests pass, including the ranking evaluation with the real model and the tldr bundle, so ranking is unchanged. Smart App Control blocked the debug test binary (OS error 4551); `cargo test --release --lib` ran.
- **Next:** The margin is only 2–5 MB and depends mostly on WebView2. Watch it when adding frontend features.
