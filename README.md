# Rust Commander

Standalone dual-pane file manager for Linux. Tokyo Night is used until an Omarchy theme is found on a verified path.

## Build on Linux

```bash
# Ubuntu / Debian
sudo apt install build-essential pkg-config libgtk-4-dev libvte-2.91-gtk4-dev

# Arch / Omarchy
sudo pacman -S --needed base-devel pkgconf gtk4 vte4
```

```bash
cargo run --release
```

Optional runtime tools (not required to build):

- 7-Zip for `.7z`: Ubuntu `p7zip-full`, Arch `p7zip` (binary `7z`)
- RAR pack: RARLAB `rar` (`unrar` does not pack)
- Sandbox tests: `bubblewrap` on Ubuntu and Arch

`bubblewrap` is only needed for `scripts/test-sandbox.sh`, not to start the program. Omarchy does not need to be installed; it uses the same Arch packages.

If `cargo build` fails on `vte-2.91-gtk4`, install the VTE GTK4 package for your distro (table above).

The log is written to `~/.local/state/rust-commander/rust-commander.log`.

Try copy, move, and delete in a scratch directory, for example `~/rust-commander-prov` with two subdirectories.

1. Open the left panel in one folder and the right panel in the other.
2. F5 copies a file and a folder with contents to the right.
3. F6 moves a file back.
4. F7 creates a folder. Shift+F6 renames it.
5. F8 moves a file to the trash. Restore it from the trash if you want to see that it is not really gone.
6. F3 shows a text file, or an image in its own window. Ctrl+Q previews in the other panel. Left and right change the image in that folder.
7. Close and start again. The window comes back.

## Keys

Tab switches panel. Up and down move the cursor. Space marks the row under the cursor. Insert marks and moves to the next row. F2 reloads. Ctrl+S filters file names in the active panel: plain text matches parts of names, while `*.md` or `edit?.txt` uses `*` and `?` against the whole name. Ctrl+Shift+S searches file and directory names from the active panel's directory, including subdirectories. The search window can limit the search to files, choose hidden names and case sensitivity, cancel a search, and go to a hit. Ctrl+H shows or hides dotfiles.

F3 shows text or an image. In the image window the arrow keys change the image in the folder where the window was opened. When Quick View or the image window shows an image, the buttons at the bottom change to S Fit or S 1:1. S on a text file is still ASCII. F4 editor, Shift+F4 new file, F5 copy, Shift+F5 copy with a new name, F6 move, Shift+F6 rename, F7 new folder, F8 trash, Shift+F8 delete permanently.

Special modes change the buttons at the bottom of the main window to commands for that mode. The row is turned off with Mode buttons in F12.

Alt+F5 packs selected files. The extension chooses the format: `.zip` is built in, `.7z` uses 7-Zip (`7z` or `7zz`, for example `/usr/bin/7z`) and `.rar` uses RARLAB `rar` (for example `/usr/bin/rar`). `unrar` does not pack. The paths are set in F12. An empty field searches PATH.

Alt+F9 unpacks a zip, rar or tar into the other panel and shows the file list. Shift+Alt+F9 unpacks the whole archive in the panel where the archive is. When the cursor is on a zip, rar or tar archive, the upper button row at the bottom shows `Alt+F9 Unpack to right panel` (or left, depending on the active panel) and `Shift+Alt+F9 Extract here`. The F keys are on the row below. The progress window shows which rar volume is being read (`From:`), the target folder (`To:`), and which file is being unpacked. Broken files are skipped and listed when unpacking finishes. If a volume is missing, it says which one. Enter on a `.sfv` checks CRC32. Shift+F9 creates a `.sfv` for the selection.

Ctrl+1 to Ctrl+9 selects a drive in the active panel. The number is shown before the icon. F9 opens a shell in the other panel in the active panel's directory. The bottom button reads F9 Shell, and F9 Shell Toggle while that shell is showing. F9 hides it and leaves it running. Ctrl+F9 opens another shell. Each shell is a numbered button to the right of the drives; Alt+1 to Alt+9 switches to it. Ctrl+W in the shell closes that shell and returns to the file list. `exit` does the same. Ctrl+W in the file list still closes a tab. Ctrl+F3 name, Ctrl+F4 extension, Ctrl+F5 time, Ctrl+F6 size. Alt+F1 and Alt+F2 are places. Alt+F3 is the external viewer. Ctrl+Q previews. There, left and right change the image in that folder. Ctrl+T new tab, Ctrl+W close tab, Alt+left and Alt+right are history.

## Tests

`scripts/test-sandbox.sh` builds on the host and runs the acceptance tests in bubblewrap. The writable area is a 256 MiB tmpfs. The real home directory is not mounted. Each file test applies landlock to its fixture before anything is changed.

`scripts/gui-smoke.sh` does the same against the window and binds only the Wayland socket.

## Checklist

MVP, with a green acceptance test: two panels, copy and move trees, conflict, Lister (text, binary, hex, windowed reading), Quick View, images in Quick View and F3, filter, sorting, history, bookmarks, and tabs.

Later, one at a time: multi-select rename (Ctrl+M), search in file contents and advanced search conditions, archives as folders, directory sync.

Left out: Windows builds, network neighborhood, parallel port, USB cable, UAC, Windows context menu, WLX/WCX/WDX/WFX, and IrfanView/XnView.

## Omarchy

Omarchy is Arch-based, so use the pacman build packages above. `omarchy/theme-set` takes the theme name as the first argument, writes `~/.config/rust-commander/active-theme`, and sends SIGUSR1 to Rust Commander. Themes are read from `/usr/share/omarchy/themes/<name>/colors.toml` or `~/.config/omarchy/themes/<name>/colors.toml`. If the file is missing, Tokyo Night is used.

The `.desktop` file is in `data/rust-commander.desktop`.
