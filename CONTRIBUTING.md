# Contributing

Report bugs and ideas on the [issue tracker](https://github.com/gr3p/rust-commander/issues). Build instructions are in the [README](README.md#build-from-source).

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
