# Contributing

Report bugs and ideas on the [issue tracker](https://github.com/gr3p/rust-commander/issues). Build instructions are in the [README](README.md#build-from-source).

## Version number

The version is major.minor from `Cargo.toml` and the commit count as patch, for example `0.2.108`. The release workflow and `build.rs` compute it the same way. Enable the hook once per clone so every commit also writes it into `Cargo.toml` and `Cargo.lock`, and `cargo build` prints the real number:

```bash
git config core.hooksPath .githooks
```

To start a new minor version, change major.minor in `Cargo.toml`; the hook keeps the patch.

## Testing a change

`bubblewrap` is only needed for `scripts/test-sandbox.sh`, not to build or start the program.

Try copy, move and delete in a scratch directory, for example `~/rust-commander-prov` with two subdirectories.

1. Open the left panel in one folder and the right panel in the other.
2. F5 copies a file and a folder with contents to the right.
3. F6 moves a file back.
4. F7 creates a folder. Shift+F6 renames it.
5. F8 moves a file to the trash. Restore it from the trash if you want to see that it is not really gone.
6. F3 shows a text file, or an image in its own window. Ctrl+Q previews in the other panel. Left and right change the image in that folder.
7. Close and start again. The window comes back.


## Music player tests

The audio tests (`cargo test --lib audio`) need no sound card. They play through a recording sink, and through ALSA's `null` device where the library exists. Tests that need libopus or ALSA skip themselves when the library is missing.

The test files live in `tests/fixtures/audio` and are committed, so ffmpeg is only needed to change them: `scripts/make-audio-fixtures.sh` rebuilds all of them. The GUI smoke test (`scripts/gui-smoke.sh`) also plays files through the whole app, on the `null` device: it starts the idle quick view with Enter, checks that the cursor does not steer the song and that Space marks again when the file panel has the keys, steps through three tracks with next and previous, sends and adds marked songs as a playlist, minimizes the player to the small bar, walks Tab and Ctrl+M through it, plays songs in it with Enter and Ctrl+Q, and expands it again. The playlist, the visualizer's analysis (`audio/fft.rs`, `audio/analyzer.rs`) and the folder cover search (`audio/cover.rs`) are plain Rust with unit tests, so they run without a window.
