# InTerminal

InTerminal is a PowerShell terminal for Windows 10 and 11 that helps people learn and use the command line.

- **A file tree that follows the terminal.** `cd` in the terminal moves the tree. Clicking a folder types the matching `cd` for you.
- **Command search in plain English.** Type "extract zip file" or "kill port 3000" to find commands that are installed on this computer, with help for each value.
- **Fixes for failed commands.** When a command fails, InTerminal suggests the command that fixes it. Fixes are never run without you.
- **An on-demand local AI.** A small model writes a PowerShell command from a description, and you review it before it runs. The model is in memory only while it writes.

Everything works from the keyboard and with screen readers. Press **F1** in the app for all shortcuts.

## Install

There are two ways to get InTerminal. Both give you the same app.

### Option 1: the installer

Download `InTerminal_<version>_x64-setup.exe` from the [latest release](https://github.com/RFahmi99/InTerminal/releases/latest) and run it. InTerminal then offers new versions itself, and installs them only when you choose.

**The installer isn't code-signed yet.** InTerminal is a new project, and free code signing for open-source projects is only given to established ones. Until it is signed, Windows warns about it:

- **SmartScreen** shows "Windows protected your PC". Choose **More info**, then **Run anyway**.
- **Smart App Control**, if it's turned on, blocks unsigned apps and has no "run anyway" option. Use option 2 instead.

Every release is built from this repository's source by GitHub Actions, and each update is checked against the project's own key before it installs.

### Option 2: build it yourself

This builds the same installer on your own PC from the source code. It needs about 6 GB of disk space for the build tools, and the first build takes 10–20 minutes.

**1. Install the tools** (once). In PowerShell:

```powershell
winget install Git.Git
winget install OpenJS.NodeJS.LTS
winget install Rustlang.Rustup
winget install Microsoft.VisualStudio.2022.BuildTools --override "--wait --passive --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
```

Then **close PowerShell and open a new window**, so it finds the new tools.

**2. Download and build InTerminal:**

```powershell
git clone https://github.com/RFahmi99/InTerminal.git
cd InTerminal
npm ci
npm run fetch-assets
npm run tauri build
```

`fetch-assets` downloads the search model and the AI engine, and checks each file against a pinned SHA-256.

**3. Install it** by running the installer the build made:

```powershell
.\src-tauri\target\release\bundle\nsis\InTerminal_0.1.0_x64-setup.exe
```

(The version number in the file name matches the version you built.) A file you build yourself isn't marked as downloaded from the internet, so SmartScreen doesn't warn about it.

**To update later**, run `git pull` in the `InTerminal` folder and repeat steps 2 and 3, or accept the update InTerminal offers.

**If the build fails with *"An Application Control policy has blocked this file (os error 4551)"***, Smart App Control blocked one of the files the build creates. It judges each file by its exact bytes, so building that part slightly differently usually gets past it. Create `src-tauri\.cargo\config.toml` with a section for the package named in the error, for example:

```toml
[profile.release.package.native-tls]
opt-level = 1
debug = 1
```

then run `npm run tauri build` again. If the blocked file is the finished `interminal.exe` or the installer, Smart App Control won't run it however it's built; use a PC without Smart App Control until InTerminal is code-signed.

## Development

```powershell
npm ci
npm run fetch-assets
npm run tauri dev      # runs the app with hot reload
```

`fetch-assets` puts third-party files in `src-tauri/resources/embedding` and `src-tauri/resources/llama`, which are git-ignored. The tldr-pages examples in `src-tauri/resources/tldr` are committed instead, because tldr-pages replaces its download whenever pages change; `npm run fetch-assets -- -UpdateTldr` refreshes them, and the change is reviewed with `git diff`. The AI model itself (~490 MB) is downloaded from inside the app, and only after the user agrees.

### Checks

```powershell
npm test            # frontend unit and component tests (Vitest)
npm run test:rust   # backend tests, including a search-ranking check against the real model
npm run typecheck
```

### Building and releasing

`npm run tauri build` produces the NSIS installer in `src-tauri/target/release/bundle/nsis`. CI builds it on every push to `main` and keeps it as a workflow artifact. Releases with in-app updates are described in [RELEASING.md](RELEASING.md).

Smart App Control can also block dev and test builds (os error 4551). The same per-package fix as above works, using `[profile.dev.package.<name>]` for dev builds and tests.

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
  updates.rs         checking for and installing new versions
src-tauri/resources/catalog.json   curated command catalog (375 entries)
scripts/fetch-assets.ps1            pinned third-party downloads
docs/ARCHITECTURE.md                design and how the plan's four domains map to code
```

## Third-party components

| Component | Licence | Where it comes from |
|---|---|---|
| [llama.cpp](https://github.com/ggml-org/llama.cpp) `llama-server` (build b11397) | MIT | `fetch-assets` |
| [potion-base-8M](https://huggingface.co/minishlab/potion-base-8M) static embeddings | MIT | `fetch-assets` |
| [tldr-pages](https://tldr.sh) command examples (common + Windows pages, as of October 2026) | CC BY 4.0, credited in the app's help dialog | committed in `src-tauri/resources/tldr` |
| [Qwen2.5-Coder-0.5B-Instruct](https://huggingface.co/Qwen/Qwen2.5-Coder-0.5B-Instruct-GGUF) | Apache-2.0 | downloaded in the app, with consent |
| Atkinson Hyperlegible Next and Mono fonts | SIL OFL 1.1 | npm (`@fontsource-variable/*`) |

## Code signing policy

Free code signing provided by [SignPath.io](https://about.signpath.io), certificate by [SignPath Foundation](https://signpath.org).

- **Committers and reviewers:** [Rayed Fahmi](https://github.com/RFahmi99)
- **Approvers:** [Rayed Fahmi](https://github.com/RFahmi99)

Only InTerminal's own files, built from this repository by GitHub Actions, are signed. The bundled third-party programs listed above keep their authors' signatures, or none.

## Privacy

InTerminal doesn't collect or send any information about you, your files or your commands. It connects to the internet only:

- to check GitHub for a new InTerminal version when it starts (you can turn this off in Settings), and to download an update when you choose to install it;
- to download the AI model, when you ask it to;
- to install PowerShell's help from Microsoft, when you ask it to.

Each request is an ordinary download from that site, which can see your IP address as with any download.

## Licence

[MIT](LICENSE).
