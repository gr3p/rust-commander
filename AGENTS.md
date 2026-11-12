# Instruktioner för AI-agenter

Gäller Cursor, VS Code/Copilot, Claude och Codex. Projektet har en utvecklare, så alla verktyg delar samma gren och samma arbetsyta.

## Git-flöde

- **Arbeta alltid på `develop`.** Skapa inga egna grenar, worktrees eller agent-grenar om användaren inte ber om det.
- `master` är release: varje push dit bygger och publicerar. Committa aldrig direkt på `master`.
- Börja varje pass med `git fetch origin` och `git status -sb`. Står du på något annat än `develop`, byt dit (`git switch develop`) innan du ändrar något. Finns ocommittade ändringar, fråga användaren först.
- Är `origin/master` eller `origin/develop` före din `develop` (`git log develop..origin/master --oneline`), merga in dem (`git merge origin/master`) innan du börjar. Annars bygger du på gammal kod och versionsnumret blir fel.
- Committa små, avslutade steg på `develop` och pusha (`git push origin develop`), så att inget verktyg lämnar ocommittat arbete åt nästa.
- Släpp till `master` bara när användaren ber om det: `git switch master && git merge --ff-only develop && git push origin master && git switch develop`. Går det inte att fast-forwarda: stanna och fråga.
- Använd inte `git stash`, `reset --hard`, `checkout .`, `restore .`, force-push eller `clean` på arbete du inte själv skapade i detta pass.

## Formatering

Koden är medvetet inte formaterad med standard-`rustfmt`. Kör inte `cargo fmt` över hela trädet; det skapar hundratals rader brus och konflikter. Följ stilen i filen du redigerar.

## Innan commit

`cargo test` ska vara grönt. Aktivera versions-hooken en gång per klon: `git config core.hooksPath .githooks` (se `CONTRIBUTING.md`).

## Projektkarta

Filkarta och regler för ytan ligger i `.cursor/rules/*.mdc` (lokala, ej spårade). Läs dem vid behov oavsett verktyg.
