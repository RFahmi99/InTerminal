# VCE: Visual Command Environment

VCE is a PowerShell terminal for Windows 10 and 11 that helps people learn and use the command line.

- **A file tree that follows the terminal.** `cd` in the terminal moves the tree. Clicking a folder types the matching `cd` for you.
- **Command search in plain English.** Type "extract zip file" or "kill port 3000" to find commands that are installed on this computer, with help for each value.
- **Fixes for failed commands.** When a command fails, VCE suggests the command that fixes it. Fixes are never run without you.
- **An on-demand local AI.** A small model writes a PowerShell command from a description, and you review it before it runs. The model is in memory only while it writes.

Everything works from the keyboard and with screen readers. Press **F1** in the app for all shortcuts.

## Prerequisites

| Tool | Install |
|---|---|
| Rust (MSVC toolchain) | `winget install Rustlang.Rustup` |
| Visual Studio 2022 Build Tools, C++ workload | `winget install Microsoft.VisualStudio.2022.BuildTools --override "--wait --passive --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"` |
| Node.js 20+ | `winget install OpenJS.NodeJS.LTS` |
| WebView2 | Preinstalled on Windows 10/11 |

## Getting started

```powershell
npm install
npm run fetch-assets   # downloads the pinned search model and llama.cpp server, verifying SHA-256
npm run tauri dev      # runs the app with hot reload
```

`fetch-assets` puts third-party files in `src-tauri/resources/embedding`, `src-tauri/resources/llama` and `src-tauri/resources/tldr`. These folders are git-ignored. The AI model itself (~490 MB) is downloaded from inside the app, and only after the user agrees.

## Checks

```powershell
npm test            # frontend unit and component tests (Vitest)
npm run test:rust   # backend tests, including a search-ranking check against the real model
npm run typecheck
```

## Building an installer

```powershell
npm run tauri build   # produces an NSIS installer in src-tauri/target/release/bundle/nsis
```

If Windows 11 **Smart App Control** is on, it can block the freshly compiled Tauri build script with *"An Application Control policy has blocked this file (os error 4551)"*. If that happens, either:

- build in CI, for example a GitHub Actions `windows-latest` runner, or
- build on a machine with Smart App Control off. Turning it off is permanent unless Windows is reset.

`npm run tauri dev` usually still works, because debug build scripts are allowed.

## Project layout

```
src/                 React + TypeScript frontend
  components/        explorer, terminal, search, suggestions, ai, dialogs
  a11y/              screen-reader announcements (live regions)
  ipc/               typed wrappers for every backend command and event
src-tauri/src/
  shell/             PowerShell in ConPTY, prompt integration, safe quoting
  index/             PATH and cmdlet inventory, embeddings, search ranking
  assist/            error fix rules, risk assessment, command templates
  ai/                model download, llama-server lifecycle, prompt and parsing
  fs_ops.rs          file tree operations
src-tauri/resources/catalog.json   curated command catalog (375 entries)
scripts/fetch-assets.ps1            pinned third-party downloads
docs/ARCHITECTURE.md                design and how the plan's four domains map to code
```

## Third-party components

| Component | Licence | Where it comes from |
|---|---|---|
| [llama.cpp](https://github.com/ggml-org/llama.cpp) `llama-server` (build b11397) | MIT | `fetch-assets` |
| [potion-base-8M](https://huggingface.co/minishlab/potion-base-8M) static embeddings | MIT | `fetch-assets` |
| [tldr-pages](https://tldr.sh) v2.3 command examples (common + Windows pages) | CC BY 4.0, credited in the app's help dialog | `fetch-assets` |
| [Qwen2.5-Coder-0.5B-Instruct](https://huggingface.co/Qwen/Qwen2.5-Coder-0.5B-Instruct-GGUF) | Apache-2.0 | downloaded in the app, with consent |
| Atkinson Hyperlegible Next and Mono fonts | SIL OFL 1.1 | npm (`@fontsource-variable/*`) |
