# workuum

Docuum for git worktrees. workuum finds every linked git worktree on your machine and removes the ones that are merged, stale or least recently used. It also deletes stale project build folders (`target/`, `node_modules/` and friends) and, under disk pressure, package-manager caches, so every volume keeps a free-space floor. It never forces a git removal.

## Install

macOS and Linux:

```sh
curl -fsSL https://raw.githubusercontent.com/Glubiz/workuum/main/install.sh | sh
```

Windows (PowerShell):

```powershell
irm https://raw.githubusercontent.com/Glubiz/workuum/main/install.ps1 | iex
```

Add `--service` (`-Service` on Windows) to also register the background service, see [Running in the background](#running-in-the-background):

```sh
curl -fsSL https://raw.githubusercontent.com/Glubiz/workuum/main/install.sh | sh -s -- --service
```

```powershell
& ([scriptblock]::Create((irm https://raw.githubusercontent.com/Glubiz/workuum/main/install.ps1))) -Service
```

The scripts install the latest release, or the version you pass (`sh -s -- 0.1.0`, `-Version 0.1.0`), and verify its sha256. `WORKUUM_INSTALL_DIR` sets the folder (`/usr/local/bin`, or `%LOCALAPPDATA%\Programs\workuum` on Windows). They refuse to overwrite a Homebrew or Chocolatey install unless `WORKUUM_INSTALL_FORCE=1` (`-Force`).

Homebrew (macOS and Linux), using the `Glubiz/homebrew-tap` tap:

```sh
brew install Glubiz/tap/workuum
```

Chocolatey (Windows):

```sh
choco install workuum
```

From source:

```sh
cargo install --path crates/workuum
```

## Updating

`workuum update` checks the latest GitHub release and does nothing when already current.

When Homebrew or Chocolatey owns the binary, it runs `brew upgrade workuum` or `choco upgrade workuum -y` (Chocolatey needs an elevated shell). On failure it prints the command to run.

Otherwise it downloads the release for your OS and CPU, verifies its sha256, checks the new binary runs, and replaces the running binary in place. When the binary lives in a folder that needs admin rights, such as `/usr/local/bin`, run it with `sudo`.

## Quick start

```sh
workuum list              # show everything workuum finds and its verdict, change nothing
workuum clean --dry-run   # show what a pass would remove
workuum clean             # run one pass
workuum watch             # run a pass every hour, forever
```

`list --size` measures the folders and `list --json` prints a JSON array.

## How it decides

Worktrees are judged in this order, the first match wins. Only linked worktrees are ever touched, never the main checkout.

| Verdict | Meaning | Action |
| --- | --- | --- |
| locked | `git worktree lock` is set | kept |
| dirty | modified or untracked files | kept |
| orphaned | detached HEAD with commits on no branch, remote or tag | kept, removing it would lose those commits |
| kept | matches `--keep`, or you are standing in it | kept |
| active | used more recently than `--min-age` | kept |
| prune | the folder no longer exists | `git worktree prune` |
| merged / squashed / gone | merged into a base, squash-merged, or its upstream branch was deleted | removed |
| stale | unmerged and idle past `--stale-after` | removed |
| idle | unmerged and idle | removed only under disk pressure |

Build folders are only deleted when git ignores them and they hold no tracked files, they sit beside a marker file, and their checkout is not locked, kept or about to be removed. A build folder's last use is the newer of its checkout's git activity and the modification times of the folder and its direct children, so a folder that is only read (an app run without rebuilds, no git activity) counts as unused.

| Ecosystem | Folders | Marker files |
| --- | --- | --- |
| Rust | `target` | `Cargo.toml` |
| Maven | `target` | `pom.xml` |
| Node | `node_modules`, `.next`, `.nuxt`, `.svelte-kit`, `.turbo`, `.parcel-cache` | `package.json` |
| Angular | `.angular` | `angular.json` |
| Python | `.venv`, `venv`, `.tox`, `.nox`, `.mypy_cache`, `.pytest_cache`, `.ruff_cache` | `pyproject.toml`, `setup.py`, `setup.cfg`, `requirements.txt`, `Pipfile`, `tox.ini` |
| Gradle | `build`, `.gradle` | `build.gradle`, `build.gradle.kts`, `settings.gradle`, `settings.gradle.kts` |
| .NET | `bin`, `obj` | `*.csproj`, `*.fsproj`, `*.vbproj` |
| Swift | `.build` | `Package.swift` |
| PHP | `vendor` | `composer.json` |
| Elixir | `_build`, `deps` | `mix.exs` |
| Dart | `.dart_tool`, `build` | `pubspec.yaml` |
| Zig | `zig-cache`, `.zig-cache`, `zig-out` | `build.zig` |

Build folders idle past `--cache-stale-after` are deleted by the rules. Global package-manager caches are only cleared under disk pressure, by their own tool:

| Cache | Located with | Cleared with |
| --- | --- | --- |
| npm | `npm config get cache` | `npm cache clean --force` |
| bun | `bun pm cache` | `bun pm cache rm` |
| pnpm | `pnpm store path` | `pnpm store prune` |
| yarn | `yarn cache dir` | `yarn cache clean` |
| go build | `go env GOCACHE` | `go clean -cache` |
| go modules | `go env GOMODCACHE` | `go clean -modcache` |
| pip | `pip cache dir` | `pip cache purge` |

When a volume has less free space than `--min-free`, workuum frees more after the rules have run, oldest first, in this order: idle build folders, global caches, idle worktrees. It stops as soon as the floor is met and warns if it cannot reach it.

## Safety

- It never passes `--force`. Git refuses to remove a worktree it considers unsafe and workuum leaves it alone.
- Branches survive removal, unless you opt in to `--delete-branch` for merged worktrees, and even then it uses `git branch -d`.
- Ignored files, including `.env`, live in the worktree folder and go with it when it is removed.
- Protect a worktree with `git worktree lock`, or with `--keep`. A worktree you are standing in is never removed.
- It runs as your user and only touches what that user can.
- Use `clean --dry-run` first.

## Options

Every option is global and can also be set through its environment variable. List options take comma-separated values in the environment. Globs match against `/`-normalised paths, `*` also matches across `/`.

| Flag | Environment | Default | Meaning |
| --- | --- | --- | --- |
| `--root <path>` | `WORKUUM_ROOTS` | home directory | folders searched for repositories |
| `--exclude <glob>` | `WORKUUM_EXCLUDE` | caches and trash folders | folders the search skips, added to the built-in excludes; changing it triggers a rescan |
| `--keep <glob>` | `WORKUUM_KEEP` | none | worktree paths, branch names or build folder paths never removed |
| `--base <ref>` | `WORKUUM_BASE` | `origin/HEAD`, `origin/main`, `origin/master`, `main`, `master` | refs a branch counts as merged into |
| `--min-age <dur>` | `WORKUUM_MIN_AGE` | `1d` | nothing used more recently than this is touched |
| `--stale-after <dur\|off>` | `WORKUUM_STALE_AFTER` | `30d` | unmerged worktrees idle this long are removed |
| `--cache-stale-after <dur\|off>` | `WORKUUM_CACHE_STALE_AFTER` | `14d` | build folders idle this long are deleted |
| `--min-free <size\|%>` | `WORKUUM_MIN_FREE` | `10%` | free space to keep per volume, e.g. `20GB` or `15%` |
| `--fetch` | `WORKUUM_FETCH` | off | `git fetch --prune` each repository before judging merges |
| `--delete-branch` | `WORKUUM_DELETE_BRANCH` | off | delete the branch of a merged worktree after removing it |
| `--skip-global-caches` | `WORKUUM_SKIP_GLOBAL_CACHES` | off | never clear npm, bun, pnpm, yarn, go or pip caches |
| `--rescan <dur>` | `WORKUUM_RESCAN` | `24h` | search the roots again when the stored index is older than this |
| `--session <name>` | `WORKUUM_SESSION` | `default` if saved | saved settings session to use, see [Settings](#settings) |
| `watch --interval <dur>` | `WORKUUM_INTERVAL` | `1h` | time between passes |
| `clean --dry-run` | | off | print what would happen, change nothing |

Durations look like `90s`, `30m`, `24h`, `1d` or `2w`.

## Settings

Run `workuum settings` for a full-screen editor of saved settings, so `workuum` and the background service run with your options and no flags. It needs an interactive terminal and works from the keyboard alone.

| Key | Action |
| --- | --- |
| `↑` `↓` or `k` `j` | move between options |
| `Enter` | edit the value; lists are comma-separated, `Esc` cancels. On a boolean, `Enter` or `Space` toggles it |
| `r` | reset the option to its default (removes it from the session) |
| `←` `→` | switch session |
| `n` | new session, starting as a copy of the current one |
| `d` | delete the session, after a `y`/`n` confirm |
| `s` | save |
| `q` or `Esc` | quit; with unsaved changes it asks to save, discard or cancel |

The editor shows each option's value, its default, and `*` where the session sets it. Input is checked with the same parser the flag uses, so an invalid value is refused with the parser's message.

Settings live in `settings.toml` in the OS config directory:

- macOS: `~/Library/Application Support/workuum/settings.toml`
- Linux: `~/.config/workuum/settings.toml`
- Windows: `%APPDATA%\workuum\settings.toml`

Set `WORKUUM_CONFIG_DIR` to use another directory. The file has one table per session, keyed by the long flag names. Lists are arrays, booleans are booleans and everything else is a string in the flag's own syntax. A leading `~` in `root` entries is the home directory.

```toml
[sessions.default]
min-free = "15%"
root = ["~/code"]
fetch = true

[sessions.aggressive]
min-age = "0s"
stale-after = "7d"
```

Without `--session` (or `WORKUUM_SESSION`) workuum uses the session named `default` if it is saved, otherwise the built-in defaults. An unknown session name or an invalid saved value exits with status 2. `workuum settings` and `workuum update` ignore the saved values, so a broken setting never stops you fixing it.

For every option the order is: flag, then environment variable, then session, then built-in default. A flag turns a boolean on, and an environment variable such as `WORKUUM_FETCH=false` turns it off over a session that sets it.

## Running in the background

The files in `service/` run `workuum watch` with no flags, so the service uses your saved `default` settings session. The simplest way to register it is the installer with `--service` (`-Service` on Windows). It fetches the service file for the installed version, points it at the installed binary and replaces any earlier registration, so re-running is safe:

- macOS: a launchd agent at `~/Library/LaunchAgents/local.workuum.plist`.
- Linux: a systemd user unit at `~/.config/systemd/user/workuum.service`. This needs a systemd user session.
- Windows: a scheduled task named `workuum` that runs at logon.

If registration fails the binary is still installed, and the installer prints the manual steps and exits non-zero. To do it by hand, edit the binary path in the file (the files assume `/usr/local/bin/workuum`; the Windows task runs `workuum.exe` from your `PATH`) and then:

macOS:

```sh
cp service/local.workuum.plist ~/Library/LaunchAgents/
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/local.workuum.plist
```

Linux:

```sh
mkdir -p ~/.config/systemd/user
cp service/workuum.service ~/.config/systemd/user/
systemctl --user enable --now workuum
```

Windows:

```bat
schtasks /create /xml service\workuum.xml /tn workuum
```

## State

The repository index and the pass lock live in the OS data directory (for example `~/Library/Application Support/workuum` on macOS, `~/.local/share/workuum` on Linux). Set `WORKUUM_STATE_DIR` to put them elsewhere. Only one pass runs at a time.

## Development

```sh
cargo nextest run --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

## License

MIT
