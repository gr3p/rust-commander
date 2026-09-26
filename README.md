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
6. F3 visar en textfil, eller en bild i ett eget fönster. Ctrl+Q förhandsvisar i den andra panelen. Vänster och höger byter bild i den mappen.
7. Stäng och starta igen. Fönstret kommer tillbaka.

## Tangenter

Tab byter panel. Upp och ned flyttar markören. Space markerar raden under markören. Insert markerar och går till nästa rad. F2 laddar om. Ctrl+S filtrerar filnamn i aktiv panel: vanlig text matchar delar av namn, medan `*.md` eller `edit?.txt` använder `*` och `?` mot hela namnet. Ctrl+Shift+S söker fil- och katalognamn från aktiv panels katalog, även i undermappar. Sökfönstret kan begränsa sökningen till filer, välja dolda namn och skiftlägeskänslighet, avbryta en sökning och gå till en träff. Ctrl+H visar eller döljer punktfiler.

F3 visar text eller bild. I bildfönstret byter piltangenterna bild i mappen där fönstret öppnades. När Quick View eller bildfönstret visar en bild byts knapparna längst ner till S Fit eller S 1:1. S på en textfil är fortfarande ASCII. F4 editor, Shift+F4 ny fil, F5 kopiera, Shift+F5 kopiera med nytt namn, F6 flytta, Shift+F6 byt namn, F7 ny mapp, F8 papperskorg, Shift+F8 radera permanent.

Speciallägen byter knapparna längst ner i huvudfönstret till kommandon för det läget. Raden stängs av med Mode buttons i F12.

Alt+F5 packar markerade filer. Ändelsen väljer format: `.zip` byggs in, `.7z` använder 7-Zip (`7z` eller `7zz`, till exempel `/usr/bin/7z`) och `.rar` använder RARLAB `rar` (till exempel `/usr/bin/rar`). `unrar` packar inte. Sökvägarna sätts i F12. Tomt fält söker i PATH.

Alt+F9 packar upp ett zip eller rar till den andra panelen och visar fillistan. Ctrl+Alt+F10 packar upp hela arkivet i panelen där arkivet ligger. När markören står på ett zip- eller rar-arkiv visar den övre knappraden längst ner `Alt+F9 Unpack to right panel` (eller left, beroende på aktiv panel) och `Ctrl+Alt+F10 Extract here`. F-tangenterna ligger på raden under. Progressfönstret visar vilken rar-volym som läses (`From:`), målmappen (`To:`) och vilken fil som packas upp. Trasiga filer hoppas över och listas när uppackningen är klar. Saknas en volym står det vilken. Enter på en `.sfv` kontrollerar CRC32. Shift+F9 skapar en `.sfv` för det som är markerat.

Ctrl+1 till Ctrl+9 väljer disk i den aktiva panelen. Siffran står före ikonen. Ctrl+F3 namn, Ctrl+F4 ändelse, Ctrl+F5 tid, Ctrl+F6 storlek. Alt+F1 och Alt+F2 är platser. Alt+F3 är extern visare. Ctrl+Q förhandsvisar. Där byter vänster och höger bild i den mappen. Ctrl+T ny flik, Ctrl+W stäng flik, Alt+vänster och Alt+höger är historik.

## Tester

`scripts/test-sandbox.sh` bygger på värden och kör acceptanstesterna i bubblewrap. Skrivbar yta är en tmpfs på 256 MiB. Den riktiga hemkatalogen monteras inte. Varje filtest lägger landlock på sin fixtur innan något ändras.

`scripts/gui-smoke.sh` gör samma sak mot fönstret och binder bara Wayland-socketen.

## Checklista

MVP, med grönt acceptanstest: två paneler, kopiera och flytta träd, konflikt, Lister (text, binärt, hex, fönsterläsning), Quick View, bilder i Quick View och F3, filter, sortering, historik, bokmärken och flikar.

Senare, en i taget: flerval-namnbyte (Ctrl+M), sökning i filinnehåll och avancerade sökvillkor, arkiv som mappar, katalogsynk.

Utelämnat: Windows-byggen, nätverksgrannskap, parallellport, USB-kabel, UAC, Windows-kontextmeny, WLX/WCX/WDX/WFX och IrfanView/XnView.

## Omarchy

`omarchy/theme-set` tar temanamnet som första argument, skriver `~/.config/comarchy/active-theme` och skickar SIGUSR1 till Commander. Teman läses från `/usr/share/omarchy/themes/<namn>/colors.toml` eller `~/.config/omarchy/themes/<namn>/colors.toml`. Saknas filen används Tokyo Night.

`.desktop`-filen ligger i `data/comarchy.desktop`.
