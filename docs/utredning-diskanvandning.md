# Utredning: diskanvändning per mapp

Mål: ett kommando som räknar storleken på alla mappar i den katalog man står i och visar en extra kolumn med en liten blå stapel och procent. Som Ubuntus Disk Usage Analyzer, fast enklare och inne i filpanelen.

## Läget idag

| Vad | Var | Kommentar |
|---|---|---|
| Mappstorlek | `src/sizes.rs` `folder_bytes` | Enkeltrådad, iterativ (explicit stack), följer inte symlänkar, avbryts via `AtomicBool`. Används bara av Egenskaper-dialogen (`src/ui/app/props.rs`). |
| Mappar i listan | `src/listing.rs` `read_dir_while` | Sätter `size: 0` för mappar, raden visar `<DIR>` (`src/ui/row.rs` `size_text`). |
| Kolumner | `src/ui/row.rs` `file_columns` | Sex kolumner via `SignalListItemFactory`; mallen för en ny kolumn finns i `column(...)`. |
| Bakgrundsjobb | `src/ui/app/jobs.rs`, `ops::Progress` | Tråd + `mpsc`, `poll_job`, popup visas först efter ca 200 ms, `progressbar.job` i `src/theme.rs`. |
| Förlopp i listan | `Msg::Listed(pane, tab, token, ...)` | `token` gör att svar från en gammal katalog slängs. Samma mönster behövs här. |
| Diskens storlek | `src/places.rs` `disk_info`, `DiskInfo::used_percent` | `statvfs` på monteringspunkten. |
| Tema | `src/theme.rs` | Palett och CSS. Stapeln ska ta sin färg härifrån. |

Det mesta finns alltså redan. Det som saknas är en snabb räknare, en kolumn och ett sätt att leverera delresultat till UI:t.

## Förslag i korthet

1. Ny modul `src/usage.rs` med en räknare som går igenom trädet **utan rekursion**, parallellt, och rapporterar delresultat per direkt underkatalog.
2. Nytt kommando `Act::DiskUsage` (F1-menyn, tangent att välja, förslagsvis Ctrl+U efter att ha kontrollerat att den är ledig) som startar en räkning för aktiv panels katalog.
3. En sjunde kolumn **Usage** i `file_columns` som visar stapel + procent. Storlekskolumnen visar mappens storlek i stället för `<DIR>` så snart den är känd.
4. Ingen popup vid snabba räkningar. Efter ca 300 ms visas en icke-modal förloppsruta med Avbryt. Panelen går att använda under tiden och staplarna växer allteftersom.

## 1. Räknaren (`src/usage.rs`)

### Vad som mäts

- **Allokerad storlek** (`st_blocks * 512`), inte filens längd. Det är det Disk Usage Analyzer och `du` visar, och det som faktiskt tar plats (glesa filer, klusterstorlek). Visa skenbar storlek (`meta.len()`) som alternativ i inställningarna.
- **Hårda länkar** räknas en gång: för filer med `nlink > 1` hålls en `HashSet<(dev, ino)>`. Vanliga filer har `nlink == 1` och rör aldrig mängden, så den förblir liten.
- **Symlänkar följs aldrig** (samma regel som `folder_bytes`). Länken själv räknas som sin egen lilla storlek.
- **Monteringsgränser**: gå inte in i en katalog vars `st_dev` skiljer från rotens. Då hoppar `/` över `/proc`, `/sys`, `/dev`, andra diskar och nätverksmontar, och räkningen kan inte råka starta en timeout mot en död SMB-mount. Mappen visas ändå, med en markering (t.ex. `⊘ monterad`) i stället för staplar.

### Effektivitet

- **Använd `DirEntry::metadata()`**, inte `fs::symlink_metadata(entry.path())` som `folder_bytes` gör idag. `DirEntry::metadata` går via katalogens fd (`fstatat`), så kärnan slipper slå upp hela sökvägen från `/` för varje fil. Det är den enskilt största vinsten, och `listing.rs` gör redan så.
- `DirEntry::file_type()` ger katalog/fil gratis ur `d_type` på de flesta filsystem. Statta bara det som behövs (filer för storlek). Valfritt senare steg: `libc::statx` med en snäv mask (`STATX_BLOCKS | STATX_NLINK | STATX_INO`), `libc` finns redan som beroende.
- **Parallellt med en liten trådpool** av ren `std`: delad arbetsstack `Mutex<Vec<Job>>` + `Condvar` + en `pending`-räknare. Räkningen är klar när `pending == 0`. Antal trådar `min(kärnor, 8)`. Inget nytt beroende (rayon/jwalk behövs inte).
- **Anpassa efter mediet**: läs `/sys/dev/block/<maj:min>/queue/rotational`. Roterande disk och nätverks-/FUSE-filsystem (`nfs`, `cifs`, `fuse.*` via `places.rs`/`/proc/mounts`) får 1–2 trådar, annars blir sökningen långsammare av parallellism.
- **Minne är begränsat**: inga poster per fil sparas. Varje arbetspost bär bara ett index (`slot`) till vilken direkt underkatalog den hör till, och trådarna adderar till `AtomicU64` för den sloten. Arbetsstacken är LIFO (djupet först), så den växer bara med bredden på den väg som just utforskas, inte med hela trädet.
- Andra körningen går snabbt tack vare sidcachen. Ytterligare cache av resultat sparas till steg 2 (se nedan).

### Skydd mot oändliga loopar och krascher

Det här är kärnan i din oro, så här är de konkreta försvaren:

| Risk | Åtgärd |
|---|---|
| Rekursion som spränger stacken | Ingen rekursion alls. Trädet traveseras med en explicit stack på heapen. |
| Symlänk-loopar | Följs aldrig. |
| Bind-mounts/loopar som ändå ger en cykel | Katalog-`(dev, ino)` läggs i en `HashSet` när katalogen öppnas. Redan sedd katalog hoppas över. Dessutom ett hårt djuptak (t.ex. 512 nivåer) som markerar grenen som "för djup" i stället för att fortsätta. |
| Slut på fildeskriptorer | Varje tråd har högst en öppen katalog åt gången (`read_dir` läses klart och släpps innan barnen tas). |
| Enorma kataloger (miljontals poster) | Avbrottsflaggan kollas var 1024:e post, inte bara per katalog. |
| Åtkomst nekad / fil försvann under räkningen | `EACCES` räknas ("N mappar kunde inte läsas") och visas i statusraden. `ENOENT` ignoreras tyst. Ingen `unwrap`. |
| Panik i en arbetstråd | `catch_unwind` runt varje tråds huvudloop. En trasig räkning blir ett felmeddelande, aldrig ett kraschat program. Kanalen som kopplas bort tolkas som "stoppad" precis som i `poll_job`. |
| Överflöd | `saturating_add` som i `folder_bytes`. |
| Gammalt resultat visas i fel katalog | Varje räkning får en `token`. UI:t slänger meddelanden vars token inte är aktuell (samma mönster som `Msg::Listed`). |
| Man navigerar bort mitt i | Räkningens `Arc<AtomicBool>` sätts till avbruten, trådarna avslutas inom millisekunder. |

### Gränssnitt mot UI:t

```rust
pub struct Options { pub root: PathBuf, pub apparent: bool, pub same_device: bool }
pub enum Event {
    // Delresultat ca 10 gånger per sekund: (slot, byte hittills)
    Progress { slots: Vec<(u32, u64)>, files: u64, bytes: u64, current: PathBuf },
    Finished { slots: Vec<u64>, unreadable: usize, cancelled: bool },
}
pub fn run(options: Options, slots: Vec<Box<OsStr>>, cancelled: impl Fn() -> bool, emit: impl FnMut(Event));
```

Samma form som `search::run`, så den går att testa utan fönster. `folder_bytes` kan skrivas om till att anropa samma kärna så att Egenskaper också blir snabbare (och använder samma skydd).

## 2. Kolumnen

- Lägg till `usage` i `EntryObject` (`src/ui/row.rs`): `usage-bytes: u64` och `usage-frac: f64`. Ändring avisas via `notify`, så en bunden rad uppdateras utan att listan byggs om. Samma mekanik som `marked`.
- Lagra **inte** resultatet i `Entry` (det ingår i `stamp` och kopieras vid varje listning). Håll en egen `HashMap<Box<str>, u64>` per panel i app-tillståndet, nycklad på namn och med räkningens `token`.
- Cellen är en `gtk::Overlay` med en `gtk::ProgressBar` (eller `LevelBar`) och en liten etikett ovanpå, t.ex. `37%`. Blå färg tas från temat: lägg en CSS-regel i `src/theme.rs` bredvid `progressbar.job` (`progressbar.usage trough/progress`) som använder paletten, så att den följer Omarchy-teman.
- Storlekskolumnen: `size_text` hanterar redan `is_dir && size != 0`. När en mapps storlek är känd sätts texten via `folder_size_text`/`format_size`. Under tiden står `…`. Kataloger i en räkning som inte är klar markeras med `~` så att ingen läser ett delresultat som slutvärde.
- Sortera på storlek ska sortera på `usage-bytes` för mappar medan läget är på. Det är den som ger en "vad tar plats"-vy.
- Filer i samma katalog får också stapel (deras storlek / summan), så att man ser filer och mappar jämförda.
- Kolumnen visas bara när läget är aktivt. Annars oförändrad vy, ingen kostnad.

### Vad är 100 %?

"Procent av diskspace" kan betyda två saker. Förslag: stapeln visar **andel av den öppna mappens totala innehåll** (det som Disk Usage Analyzer gör, och det som ger en användbar jämförelse när du står i en mapp), medan **andel av hela disken** visas i verktygstipset och i statusraden (`places::disk_info`, redan klar). Ett val i F12 kan byta. Se frågan längst ned.

## 3. Förloppsruta och tidsbudget

- De flesta kataloger blir klara på några hundra millisekunder. Då syns bara att staplarna fylls i, ingen ruta.
- Efter 300 ms visas en icke-modal ruta (samma utseende som `progressbar.job`, men egen liten struktur `UsageScan` i app-tillståndet, **inte** `self.job`, så att det inte krockar med en pågående kopiering) med antal filer, byte och aktuell mapp, samt **Avbryt**.
- Procent i rutan: om roten är en monteringspunkt (t.ex. `/` eller `/home`) kan `DiskInfo.used()` vara nämnare, och förloppet blir ärligt ("62 % av 118 GB"). I en vanlig undermapp finns ingen känd nämnare, så rutan använder pulserande stapel plus räknare. Hellre ärlig pulsering än en procent som hoppar.
- Esc, byte av katalog, byte av flik eller stängd panel avbryter pågående räkning.
- Vid rot (`/`) och nätverksmonteringar startar ingenting av sig själv. Räkningen är alltid ett uttryckligt kommando i första versionen.

## 4. Cache

Vinsten är stor, framför allt för att **en räkning av en övre mapp redan ger storleken på alla kataloger under den**. Går man ner i en undermapp behövs ingen ny skanning. Dessutom är kallstarten av `/` dyr (minuter), och sidcachen hjälper bara så länge metadatan ligger kvar i RAM.

- **Bara minne i första versionen.** `HashMap<(PathBuf, Alternativ), Usage>` med `bytes`, `scanned_at`, `complete`. Endast kataloger sparas, aldrig filer, och bara de över ca 1 MiB (mindre räknas om direkt ändå). Tak ca 50 000 poster (några MB), äldsta ut först. Poster går ut efter ca 10 min och cachen töms när läget varit av en stund.
- **Visa gammalt medan nytt räknas.** Vid refresh visas de gamla siffrorna dämpade med `~` medan en ny räkning strömmar in och ersätter dem. Staplarna blir aldrig tomma.
- **Invalidering:**
  - Katalogens `mtime` ändras bara av direkta barn och bevisar alltså inget om djupa ändringar. Den används inte som giltighetstest.
  - Egna filjobb (`jobs.rs`: kopiera, flytta, radera, packa, packa upp) justerar cachen med skillnaden för käll- och målkatalog och alla deras föräldrar.
  - Refresh räknar om den öppna mappen. Dessutom "räkna om markerad mapp": skannar bara det delträdet och uppdaterar föräldrarna med skillnaden (ny minus gammal).
  - Alternativ som allokerad/skenbar storlek och mountgräns ingår i nyckeln.
- **Disk (valfritt, senare):** en enda fil `~/.cache/rust-commander/usage.bin` med hårt tak (ca 5 MB) och 24 timmars giltighet, som skrivs över och därför aldrig växer. Mest nytta för att slippa minutskanningen av `/` efter omstart. Alltid märkt "beräknad för N timmar sedan".

## 5. Etapper

1. **Kärnan** `src/usage.rs` + enhetstester (se nedan) + låt `folder_bytes` använda den.
2. **Kommando + kolumn + förloppsruta + minnescache** (`commands.rs`, `ui/app/…`, `ui/row.rs`, `theme.rs`). Acceptanstest i `tests/acceptance.rs`.
3. **Efterarbete**: sticky läge (räkna automatiskt vid byte av katalog, men aldrig för `/`, nätverk, eller kataloger över en tröskel), diskcache, filter för dolda filer, dubbelklick för att gå ner i mappen.

## Tester

- Summerar rätt över nästlade mappar; hård länk räknas en gång; symlänk följs inte.
- Symlänk till en förälder (loop) och en mapp med `chmod 000` avbryter inte räkningen.
- Katalog på 100 000 tomma filer avbryts inom ~100 ms efter att flaggan satts.
- Djup på 2 000 nivåer ger "för djup", inte stack overflow (testtråd med liten stack bevisar att det inte är rekursion).
- Gammal `token` ger inget utslag i panelen (acceptanstest).
- `cargo test` ska vara grönt före push, som vanligt.

## Risker och avvägningar

| Risk | Kommentar |
|---|---|
| Hårddisk blir seg av trådarna | Rotational-detektion ger 1–2 trådar. Annars en flagga i F12. |
| Siffrorna avviker från `du` | Samma regler (allokerad storlek, hårda länkar en gång, samma enhet) ger samma resultat. Btrfs/ZFS-komprimering och reflinks ger ändå avvikelser, vilket Disk Usage Analyzer också har. |
| Delresultat misstolkas | `~` och dämpad stapel tills räkningen är klar. |
| GTK-prestanda med många rader | Uppdatera modellen i batchar högst 10 gånger per sekund. `ColumnView` binder bara synliga celler, så kostnaden följer antalet synliga rader. |
| Räkningen pekar på borttagen mapp | `ENOENT` på roten ger ett felmeddelande i statusraden. |

## Öppna frågor

1. Ska stapeln visa andel av **mappens innehåll** (förslag) eller andel av **hela disken**?
2. Ska läget vara ett **engångskommando** (förslag för första versionen) eller ett **sticky läge** som räknar vid varje katalogbyte?
3. Tangent: Ctrl+U om den är ledig, annars något annat?
4. Skenbar eller allokerad storlek som standard? Förslag: allokerad.
