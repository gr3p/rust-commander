# Commander

Fristående dual-pane-filhanterare för Linux. Tokyo Night används tills ett Omarchy-tema finns på en verifierad sökväg.

## Köra på Ubuntu

```bash
sudo apt install build-essential pkg-config libgtk-4-dev
cargo run --release
```

`bubblewrap` behövs bara för `scripts/test-sandbox.sh`, inte för att starta programmet. Omarchy behöver inte vara installerat.

Loggen skrivs till `~/.local/state/comarchy/comarchy.log`.

Prova kopiera, flytta och radera i en skräpkatalog, till exempel `~/comarchy-prov` med två undermappar.

1. Öppna vänster panel i den ena mappen och höger i den andra.
2. F5 kopierar en fil och en mapp med innehåll till höger.
3. F6 flyttar en fil tillbaka.
4. F7 skapar en mapp. Shift+F6 byter namn på den.
5. F8 lägger en fil i papperskorgen. Återställ den från korgen om du vill se att den inte är borta på riktigt.
6. F3 visar en textfil. Ctrl+Q förhandsvisar den i den andra panelen.
7. Stäng och starta igen. Fönstret kommer tillbaka.

## Tangenter

Tab byter panel. Upp och ned flyttar markören. Space markerar raden under markören. Insert markerar och går till nästa rad. F2 laddar om. Ctrl+S filtrerar. Ctrl+H visar eller döljer punktfiler.

F3 Lister, F4 editor, F5 kopiera, Shift+F5 kopiera med nytt namn, F6 flytta, Shift+F6 byt namn, F7 ny mapp, F8 papperskorg, Shift+F8 radera permanent.

Ctrl+F3 namn, Ctrl+F4 ändelse, Ctrl+F5 tid, Ctrl+F6 storlek. Alt+F1 och Alt+F2 är platser. Alt+F3 är extern visare. Ctrl+T ny flik, Ctrl+W stäng flik, Alt+vänster och Alt+höger är historik.

## Tester

`scripts/test-sandbox.sh` bygger på värden och kör acceptanstesterna i bubblewrap. Skrivbar yta är en tmpfs på 256 MiB. Den riktiga hemkatalogen monteras inte. Varje filtest lägger landlock på sin fixtur innan något ändras.

`scripts/gui-smoke.sh` gör samma sak mot fönstret och binder bara Wayland-socketen.

## Checklista

MVP, med grönt acceptanstest: två paneler, kopiera och flytta träd, konflikt, Lister (text, binärt, hex, fönsterläsning), Quick View, filter, sortering, historik, bokmärken och flikar.

Senare, en i taget: flerval-namnbyte (Ctrl+M), sök (Alt+F7), arkiv som mappar, bilder i Lister, katalogsynk.

Utelämnat: Windows-byggen, nätverksgrannskap, parallellport, USB-kabel, UAC, Windows-kontextmeny, WLX/WCX/WDX/WFX och IrfanView/XnView.

## Omarchy

`omarchy/theme-set` tar temanamnet som första argument, skriver `~/.config/comarchy/active-theme` och skickar SIGUSR1 till Commander. Teman läses från `/usr/share/omarchy/themes/<namn>/colors.toml` eller `~/.config/omarchy/themes/<namn>/colors.toml`. Saknas filen används Tokyo Night.

`.desktop`-filen ligger i `data/comarchy.desktop`.
