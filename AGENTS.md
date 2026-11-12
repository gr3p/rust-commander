# Instruktioner för AI-agenter

Gäller Cursor, VS Code/Copilot, Claude och Codex. Projektet har en utvecklare, så alla verktyg delar samma gren och samma arbetsyta.

## Git-flöde

- **Det finns bara en gren: `master`.** Arbeta direkt på den. Skapa inga andra grenar, worktrees eller agent-grenar, och skapa ingen `develop`.
- Varje push till `master` bygger och publicerar en release (`.github/workflows/release.yml`). Pusha därför bara testad kod (`cargo test` grönt). Ändringar som bara rör `*.md` publicerar inget.
- Börja varje pass med `git fetch origin` och `git status -sb`. Står du på något annat än `master`, byt dit (`git switch master`) innan du ändrar något. Finns ocommittade ändringar, fråga användaren först.
- Ligger `origin/master` före dig (`git log master..origin/master --oneline`), hämta det med `git pull --ff-only` innan du börjar. Annars bygger du på gammal kod och versionsnumret blir fel.
- Committa små, avslutade steg och pusha direkt (`git push origin master`), så att inget verktyg lämnar ocommittat arbete åt nästa. Blir pushen avvisad: `git pull --rebase`, kör testerna och pusha igen.
- Använd inte `git stash`, `reset --hard`, `checkout .`, `restore .`, force-push eller `clean` på arbete du inte själv skapade i detta pass.

## Formatering

Koden är medvetet inte formaterad med standard-`rustfmt`. Kör inte `cargo fmt` över hela trädet; det skapar hundratals rader brus och konflikter. Följ stilen i filen du redigerar.

## Innan commit

`cargo test` ska vara grönt. Aktivera versions-hooken en gång per klon: `git config core.hooksPath .githooks` (se `CONTRIBUTING.md`).

## Projektkarta

Filkarta och regler för ytan ligger i `.cursor/rules/*.mdc` (lokala, ej spårade). Läs dem vid behov oavsett verktyg.
