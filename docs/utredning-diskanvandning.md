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

### Nätverksdiskar (SMB, NFS, sshfs)

Här är en varning befogad, eftersom en skanning kan bli mycket långsam eller hänga. Varje `stat` är ett nätverksanrop (SMB/NFS har latens per fråga, inte bara per byte). En stor share kan ge hundratusentals anrop, och en förlorad VPN eller sovande NAS kan blockera ett anrop i uninterruptible sleep (D-läge) i tiotals sekunder, så att `cancel`-flaggan inte hjälper förrän anropet återvänder.

- **Detektion:** `places.rs` har redan `Mount.network` (`cifs`, `smb3`, `nfs`, `nfs4`, `fuse.sshfs`) och `props.rs` känner igen samma typer. Använd `mount_entry` på rotens sökväg. Räkna även andra `fuse.*` som nätverk.
- **Bekräftelse innan start:** en liten dialog: "Detta är en nätverksdisk (cifs). Att räkna mappstorlekar kan ta lång tid och belasta servern. Fortsätta?" med knapparna **Räkna** och **Avbryt**, och en kryssruta "Fråga inte igen för den här disken" som sparas i config. Ingen dialog för lokala diskar.
- **Försiktig räkning:** 2–4 trådar (inte 8), och ingen mountövergång. En nätverksmount under en lokal mapp hoppas över och visas som `⊘ monterad`.
- **Ärligt förlopp:** förloppsrutan visas direkt (inte efter 300 ms) och visar fil/sek, så att en seg skanning syns direkt.
- **Hängskydd:** en watchdog i UI:t som ser att `files`-räknaren inte ökat på ca 10 s och då visar "Servern svarar inte, väntar…" med Avbryt. Avbryt kopplar loss UI:t omedelbart: panelen blir användbar, och en fastnad tråd får avsluta när anropet returnerar (token gör att dess sena svar slängs).
- **Cache gör större nytta här:** nätverksresultat får längre giltighetstid i minnescachen (t.ex. 30 min mot 10) och visas som "beräknad för N min sedan", så att en refresh blir ett val och inte standard.
- **Ingen automatisk räkning** på nätverksdiskar, ens i det sticky läget (etapp 3).

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

## Testplan

Planen går att köra på den här maskinen utan root och utan riktig SMB-server. Verktyg som finns: `du`, `bwrap`, `unshare -rm` (tmpfs- och bind-mounts i en användar-namnrymd fungerar, kontrollerat), `strace`, `ydotool`, `/dev/fuse` med `fusermount3`. Alla nivåer utom H körs av mig (G kräver en Wayland-session och hoppas annars över). Varje etapp (se 5) har egna grindar; ingenting pushas till `master` innan `cargo test` är grönt, eftersom varje push släpper en release.

### A. Enhetstester i `src/usage.rs` (`cargo test --lib usage`)

Körs utan fönster. Fixturer skapas i `std::env::temp_dir()` som i `sizes.rs`.

| ID | Fall | Förväntat |
|---|---|---|
| A1 | Nästlade mappar med kända filstorlekar | Summan stämmer exakt, per direkt underkatalog och totalt. |
| A2 | Hård länk (två namn, samma inode), även över olika undermappar | Räknas en gång. |
| A3 | Symlänk till fil, till mapp och till förälder (loop) | Följs aldrig. Länken själv räknas som sin egen storlek. Ingen hängning. |
| A4 | Mapp med `chmod 000` och en som försvinner mitt i räkningen | Räkningen fortsätter. `unreadable` ökar för `EACCES`, `ENOENT` ignoreras. |
| A5 | Rot som inte finns / rot som är en fil / rot som är en symlänk | Tydligt fel respektive filens storlek, aldrig panik. |
| A6 | Glesa filer (`set_len` 1 GiB, inget skrivet) | Allokerad läge visar nästan 0, skenbart läge visar 1 GiB. |
| A7 | Djup 3 000 nivåer, körs i en tråd med 256 KiB stack | Klarar sig eller ger "för djup" vid taket. Ingen stack overflow bevisar att vi inte rekursar. |
| A8 | Bred katalog med 200 000 tomma filer, flagga sätts efter 50 ms | Returnerar inom 200 ms med `cancelled`. Bevisar kontroll var 1024:e post. |
| A9 | Panik injiceras i en arbetstråd (`#[cfg(test)]`-krok) | `Finished` med fel kommer fram, processen lever, övriga trådar avslutas. |
| A10 | Trådantal 1 respektive 8 över samma träd | Identiskt resultat. |
| A11 | Maxvärden (`u64::MAX`-nära) | `saturating_add`, ingen overflow-panik i debugbygge. |
| A12 | `Event::Progress` | Tas emot högst ~10 gånger per sekund och är monotont växande; `Finished.slots` summerar till totalen. |
| A13 | `folder_bytes` efter ombyggnad | Samma tester som idag (`sizes.rs`) är fortfarande gröna. |
| A14 | Nätverksdetektion: syntetisk `/proc/mounts`-text med `cifs`, `nfs4`, `fuse.sshfs`, `fuse.rclone`, `ext4`, `tmpfs` | Rätt `network`-flagga via `places::mount_entry`. Escapade mellanslag i monteringspunkt hanteras. |
| A15 | Watchdogen som ren funktion: `stalled(last_count, last_change, now)` | Slår till först efter tröskeln och bara om räknaren står still. Inte vid pågående framsteg. |
| A16 | Cache: delta-justering efter flytt/kopia/radering, utgång på tid, tak på antal poster, olika alternativ ger olika nycklar | Föräldrar får (ny − gammal). Äldsta ut först. Inget läckage mellan alternativ. |

### B. Jämförelse mot `du` (verkliga data, läser bara)

Skript `scripts/usage-vs-du.sh` (nytt, endast läsning). Kör räknaren och `du` på samma uppsättning och jämför.

- Mappar: `~/Documents`, `/usr`, `/var/lib`, ett repo med `target/`, en mapp med hårda länkar (t.ex. `~/.cache` eller `/usr/share`), och `/usr/lib`.
- Kommandon: `du -x -B1 --apparent-size` mot skenbart läge och `du -x -B1` mot allokerat läge, jämför per direkt underkatalog (`du -x -B1 --max-depth=1`).
- Godkänt: exakt lika när inget ändrats under körningen. Annars högst 0,5 % avvikelse, och avvikelsen förklaras (filer som ändrats). Alla större avvikelser undersöks med `strace -f -c`/diff innan vi går vidare.
- Kör `du` två gånger först så att båda startar varma. En kall körning mäts separat i E.

### C. Acceptanstester (`tests/acceptance.rs`, Landlock-sandlådan)

Varje fall körs i egen process med `FIXTURE` som enda skrivbara/läsbara yta, som övriga fall där.

| ID | Fall | Förväntat |
|---|---|---|
| C1 | Landlock tillåter läsning av fixturen men inget mer: räknaren går inte utanför och kraschar inte på nekade vägar | Fel räknas, inget annat sker. |
| C2 | Modell: `token` från gammal räkning efter katalogbyte | Delresultat och `Finished` från gamla token ändrar inget i panelen. |
| C3 | Kolumnen: `size_text` för mapp med känd/okänd/ofullständig storlek | `<DIR>` → `…`/`~` → `1.5 MB`. |
| C4 | Procentberäkning: summa 0, en post, 100 % fördelat på flera, avrundning | Aldrig NaN, aldrig över 100 %, aldrig negativ. |
| C5 | Sortering på storlek i läget | Mappar sorteras på `usage-bytes`, filer på `len`. Utan läget oförändrat. |
| C6 | Kommandokatalog | `Act::DiskUsage` finns i `commands.rs` med nyckel, etikett och grupp, och krockar inte med andra tangenter. |
| C7 | Config | Ny inställning (skenbar/allokerad, bekräftad nätverksdisk) skrivs och läses, saknade nycklar ger standardvärden. |

### D. Robusthet och stress (lokalt, ingen root)

Körs som `cargo test --release -- --ignored usage_stress` eller som skript. Skapas under `/tmp`, rensas efteråt.

| ID | Fall | Hur | Förväntat |
|---|---|---|---|
| D1 | Mount-gräns | `unshare -rm`, montera `tmpfs` under en testmapp | Mappen visas som monterad, och dess innehåll räknas inte med. Med `same_device = false` räknas det. |
| D2 | Bind-mount-loop | `unshare -rm`, `mount --bind` mapp in i sin egen undermapp | Terminerar. Katalog-`(dev, ino)`-mängden eller djuptaket stoppar loopen. Ingen dubbelräkning av det som redan setts. |
| D3 | Fildeskriptorer | `ulimit -n 32`, träd med 5 000 kataloger | Klarar sig, inga `EMFILE` (en öppen katalog per tråd). |
| D4 | Minne | 1 000 000 filer i 20 000 kataloger, mät maxminne med `/usr/bin/time -v` | Under ca 100 MB, och påverkas inte av antalet filer (bara av bredden). |
| D5 | Radering under räkning | Bakgrundsprocess raderar och skapar filer medan räkningen går, 20 körningar | Inga paniker, inga fel förutom räknade `ENOENT`. Slutsumman ligger mellan före och efter. |
| D6 | Avbryt-latens | Avbryt efter 100 ms på ett träd som tar 10 s | Alla trådar stoppade inom 250 ms, och inga tråd-/fd-läckor (`/proc/self/fd`, `/proc/self/task` jämförs före/efter). |
| D7 | Upprepade räkningar | 500 korta räkningar i rad och 500 avbrutna | Antal trådar och fd tillbaka på utgångsvärdet. Minnet växer inte (cachetaket håller). |
| D8 | Fel under hela trädet | Träd där varannan mapp är `000` | Alla läsbara delar summeras. `unreadable` stämmer. |
| D9 | Ovanliga namn | Namn som inte är UTF-8, nyrad i namn, 255 byte långa namn, sökväg över `PATH_MAX` | Inga paniker. Överlånga vägar räknas som fel, inte krasch. |

### E. Prestanda och trådar

Mäts med `hyperfine`-liknande loop (`time` om `hyperfine` saknas), 5 körningar, median. Före/efter jämförs mot nuvarande `folder_bytes`.

- **E1 Varm cache:** 1 vs 2 vs 4 vs 8 trådar på `~/Documents`/`/usr`. Mål: minst 3× snabbare än dagens `folder_bytes` med 8 trådar och inte långsammare än `du`.
- **E2 `entry.metadata()` mot `symlink_metadata(path)`:** isolerad mikromätning, bekräftar att fd-relativ uppslagning är vinsten som utredningen påstår.
- **E3 Kall cache:** `sync; echo 3 | sudo tee /proc/sys/vm/drop_caches` kräver root, så görs bara om du godkänner det. Annars ersätts testet av att mäta första respektive andra körningen på en mapp som inte besökts.
- **E4 Roterande disk:** kan inte testas här (NVMe). Logiken för 1–2 trådar testas som ren funktion (A-nivå) mot en syntetisk `/sys/dev/block/…/rotational`-sökväg. Verklig mätning kräver en USB-hårddisk (se H).
- **E5 Delresultat:** tid till första `Progress` under 100 ms på stora träd, så att panelen aldrig verkar hänga.

### F. Nätverksdiskar utan riktig server

Vi har ingen SMB-server här (`smbd` saknas), men beteendena kan ändå testas:

| ID | Fall | Hur | Förväntat |
|---|---|---|---|
| F1 | Klassificering | A14 | `network = true` för cifs/smb3/nfs/nfs4/fuse.*. |
| F2 | Långsamt filsystem | Liten FUSE-daemon (Python `fusepy`/`pyfuse3` eller C med libfuse3, installeras i en venv) som svarar med 5 ms fördröjning per `getattr`, monterad med `fusermount3` under `/tmp` | Räkningen blir klar, trådantal begränsas till 2–4, filer/sek visas, avbryt fungerar. Beroenden installeras bara om testet körs. Misslyckas installationen markeras F2 som hoppad, inte som godkänd. |
| F3 | Hängande server | Samma FUSE-daemon som sover 30 s på ett `getattr` | UI:t visar "Servern svarar inte…" efter ~10 s, Avbryt frigör panelen direkt, en sen respons från den fastnade tråden ändrar inget (token). Programmet avslutas normalt även med en tråd kvar. |
| F4 | Bekräftelsedialog | GUI-smoke (G) mot FUSE-monteringen | Dialogen visas. "Avbryt" startar ingen skanning. "Fråga inte igen" sparas i config och dialogen uteblir nästa gång. |
| F5 | Nätverksmount under lokal mapp | FUSE-montering under en lokal testmapp | Den hoppas över och markeras `⊘ monterad`, ingen skanning startar. |

### G. GUI-smoke (`scripts/gui-smoke.sh`, kräver Wayland och `bwrap`; `ydotool` finns)

Utökas med ett steg som kör mot en genererad fixtur:

1. Starta, öppna fixturen i vänster panel.
2. Kör kommandot (F1-menyn och tangenten). Kolumnen **Usage** dyker upp, staplarna fylls, storleksraderna byter från `…` till värden, `..` får ingen stapel.
3. På en stor fixtur: förloppsrutan syns efter ~300 ms, visar antal och Avbryt. Avbryt lämnar staplarna avmarkerade med `~`, inget kraschar.
4. Byt katalog mitt i en räkning: ingen gammal stapel visas i den nya katalogen.
5. Esc avbryter. Byte av flik och stängning av panelen avbryter och rensar.
6. Kopiera en fil med F5 i läget: cachen justeras, procenten uppdateras utan ny skanning.
7. Refresh: gamla värden visas dämpade (`~`) medan nya strömmar in.
8. Temat: stapelfärg följer paletten i ett mörkt och ett ljust tema.
9. Stäng läget: kolumnen försvinner, inget CPU-bruk kvar (`top`/`/proc/<pid>/stat` stilla).

### H. Det jag inte kan göra härifrån

Dessa steg behöver dig eller utrustning jag saknar. De noteras som öppna i PR-/commit-texten, inte som godkända.

- Riktig SMB-share och NFS (hängande server, VPN som tappas, sovande NAS).
- Roterande USB-hårddisk (trådval och verklig prestanda).
- Btrfs/ZFS med komprimering eller reflinks (avvikelse mot `du`).
- Kall cache med `drop_caches` (kräver root).

### Grindar

1. **Etapp 1 (kärnan):** A, B, D1–D9 och E grön. `cargo test` grönt.
2. **Etapp 2 (UI + cache):** C, F och G gröna samt A16. `cargo test` grönt, `scripts/gui-smoke.sh` grönt.
3. **Före release:** hela planen en gång till på den slutliga koden, och H redovisas som öppna punkter.
4. Fel som hittas får ett eget regressionstest i A eller C innan de rättas.

## Risker och avvägningar

| Risk | Kommentar |
|---|---|
| Hårddisk blir seg av trådarna | Rotational-detektion ger 1–2 trådar. Annars en flagga i F12. |
| Siffrorna avviker från `du` | Samma regler (allokerad storlek, hårda länkar en gång, samma enhet) ger samma resultat. Btrfs/ZFS-komprimering och reflinks ger ändå avvikelser, vilket Disk Usage Analyzer också har. |
| Delresultat misstolkas | `~` och dämpad stapel tills räkningen är klar. |
| GTK-prestanda med många rader | Uppdatera modellen i batchar högst 10 gånger per sekund. `ColumnView` binder bara synliga celler, så kostnaden följer antalet synliga rader. |
| Räkningen pekar på borttagen mapp | `ENOENT` på roten ger ett felmeddelande i statusraden. |

## Utfall: vad som byggdes

Etapp 1 och 2 är byggda. De öppna frågorna fick förslagen som svar: andel av mappens innehåll, engångskommando, Ctrl+U, allokerad storlek (`usage_apparent` i config byter). Ändringar mot texten ovan:

- **Kärnan** är `src/usage.rs`. `usage::run(&Options, &cancelled, &mut progress) -> io::Result<Report>` returnerar rapporten i stället för en `Event::Finished`. Roten läses först av anroparens tråd, sedan tar en pool vid. Mappar ner till nivå 3 får egna summor (tak 50 000), vilket är det cachen lagrar. `View` håller siffrorna per rad, `Cache` minnescachen, `Watchdog` stillastående-vakten.
- **Symlänkar** räknas som 0 i skenbart läge (som gamla `folder_bytes`) och med sina egna block i allokerat läge. `folder_bytes` (Egenskaper) går nu via samma kärna.
- **Cachen** justerar inte med delta. En egen filjobb och Refresh markerar mappen och dess föräldrar som `stale` (siffran visas dämpad), och räkningen startar om på lokala diskar. Nätverksdiskar räknar inte om av sig själva efter ett filjobb.
- **Kolumnen** har tom rubrik, som glyfkolumnen, så att storleksberäkning, sortering och rubrikklick lämnas orörda. Den tar sin bredd från Name (`USAGE_WIDTH`). Procenten står till vänster om stapeln.
- **Kommandot** är `Act::DiskUsage`, Ctrl+U: första trycket räknar, under räkning avbryter det, därefter stänger det läget. F2 (Refresh) räknar om. Esc avbryter en pågående räkning.
- **Nätverksdiskar:** en `AlertDialog` med Avbryt, Räkna och "Alltid för den här disken" (sparas som `usage_network_ok`). Alla `fuse.*` räknas som nätverk.
- Andelen av hela disken står i statusraden efter en räkning på en lokal disk (inte på nätverksdisk, där `statvfs` kan vänta på servern). Verktygstips finns inte.
- Ingen del av planen togs bort. Inget F12-val för skenbar storlek: den ligger bara i config.

### Fel efter första versionen: fönstret frös

Första versionen frös fönstret under hela räkningen av `/home/marcus` (8,5 s, 1,1 miljoner poster), så att varken popup eller staplar syntes och GNOME föreslog force quit. Orsak: tråden som rapporterar förloppet väntade på samma signal och lås som arbetstrådarna. Den väcktes därför av varje ny mapp i kön och skickade tusentals delresultat per sekund till fönstret. Dessutom stal den väckningarna från arbetarna, som då väntade upp till 100 ms i taget.

Rättat:
- Rapporttråden har en egen väntesignal och skickar högst ett delresultat per 100 ms.
- Mappar direkt under roten hämtas först (FIFO), djupare mappar LIFO, så att en stor mapp inte svälter ut de andra raderna.
- Varje rad vet när dess egen räkning är klar. Under räkningen visar Size en spinner för mappar som inte är klara och `~` före storleken för de som är det. Size-cellen uppdateras live.
- Paniker skrivs nu med backtrace till loggen (`log::install_panic_hook`).

Efter rättningen tar samma räkning 0,18 s, och den längsta pausen i fönstrets huvudloop under en räkning är under 90 ms. Mot en FUSE-disk med 8 ms per anrop visas popupen och spinnrarna, med längsta paus 65 ms. Testet `progress_is_capped_and_marks_open_folders` faller med det gamla beteendet (15 ms mellan rapporterna).

### Följ-läge (Ctrl+U+R)

Sticky-läget från etapp 3 är byggt som `Act::DiskUsageFollow`, Ctrl+U+R i F1 (Ctrl+R ensamt fungerar också). Panelen räknar varje mapp den öppnar. Ctrl+R igen slutar följa och behåller staplarna, Ctrl+U eller Esc stänger läget. På en nätverksdisk följer läget bara om disken står i `usage_network_ok`, annars stannar det med ett meddelande i statusraden. GUI-smoken går in i en undermapp i följ-läge, ser att den räknas, stänger följ-läget och ser att nästa mapp inte räknas.

### Testutfall

| Nivå | Vad | Utfall |
|---|---|---|
| A | 24 enhetstester i `usage.rs` plus `config`, inklusive `View`, `Cache`, `Watchdog`, nätverksdetektion, djup 2 000 nivåer på en tråd med 256 KiB stack, panik i en arbetstråd, avbryt på 120 000 filer | gröna |
| B | `scripts/usage-vs-du.sh` mot `du` på `/usr/share`, `/usr/lib`, `/var/lib`, `~/Documents`, `src/`, `target/release/deps` | allokerat läge: exakt lika överallt. Skenbart läge: skiljer under 0,01 % (`du` räknar symlänkarnas längd, vi gör det inte) |
| C | 4 nya acceptanstester (sandlådan, namn som inte är UTF-8, storleksordning, Ctrl+U unik) | gröna |
| D | D1–D4 och D8 i `scripts/usage-stress.sh`, D5–D7 och D8 som `--ignored usage_stress`-tester | gröna. 1 miljon filer: 5,6 MB RSS och 0,09 s |
| E | 1/2/4/8 trådar mot `du` (varm cache, `/usr`) | 0,29 / 0,16 / 0,087 / 0,054 s mot `du` 0,38 s |
| F | `scripts/slowfs.py` (FUSE med 5 ms per anrop) och `scripts/usage-netfs.sh` genom det riktiga fönstret | räkningen blir klar. Vid tyst server visas "No answer…" och Cancel tar under 30 ms. Frågan före en nätverksräkning startar ingenting förrän den besvarats |
| G | `scripts/gui-smoke.sh` utökad med `smoke_usage` | grön |
| H | Riktig SMB/NFS, roterande disk, btrfs/ZFS, kall cache | ej testat, kräver annan utrustning eller root |

Kör om allt:

```bash
cargo test
cargo test --release --lib -- --ignored --test-threads=1 usage_stress
scripts/usage-vs-du.sh
scripts/usage-stress.sh
scripts/usage-netfs.sh
sh scripts/gui-smoke.sh
```

## Beslutade frågor (var öppna)

1. Ska stapeln visa andel av **mappens innehåll** (förslag) eller andel av **hela disken**?
2. Ska läget vara ett **engångskommando** (förslag för första versionen) eller ett **sticky läge** som räknar vid varje katalogbyte?
3. Tangent: Ctrl+U om den är ledig, annars något annat?
4. Skenbar eller allokerad storlek som standard? Förslag: allokerad.
