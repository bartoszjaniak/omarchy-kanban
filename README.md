# Kanban — osobista tablica zadań

`TODO` / `In Progress` / `DONE` + automatyczne archiwum. Prosty lokalny Core
(SQLite + HTTP API) i widget Omarchy. Bez sesji OpenCode, SSH i web UI.

To jest **główne źródło wiedzy o tym, co aktualnie robimy** — Ty i AI. Ty wpisujesz
i przeciągasz karty w widgecie; AI dodaje i przenosi zadania przez CLI.

## Kolumny

| Kolumna | Znaczenie |
|---|---|
| **TODO** | Rozpoznane, jeszcze nie zaczęte. |
| **In Progress** | Aktualny fokus. |
| **DONE** | Ukończone. Po **48 h** karta automatycznie przechodzi do `archived`. |

`archived` to osobny status, niewidoczny na tablicy. Retencją zajmuje się Core —
nic nie trzeba czyścić ręcznie.

## Widget

Widget pokazuje samą ikonę (bez liczby; liczby są w tooltipie). Klik otwiera trzy
kolumny.

- **Dodaj**: najedź na kolumnę — na jej dole pojawi się „＋ Nowe zadanie"
  (znika, gdy najedziesz na istniejącą kartę); kliknij je, otworzy się edytor
  (przycisk **Dodaj**). Kolumna się nie podświetla przy najechaniu.
- **Przenieś**: chwyć całą kartę i przeciągnij. Kolumna-cel się podświetla; karta
  wstawia się w miejscu upuszczenia (między sąsiadami). Puszczenie poza kolumną
  nic nie zmienia.
- **Szczegóły**: ikona `⋯` po prawej karcie otwiera edytor (tytuł, notatka,
  **Zapisz** / **Anuluj**). Klik w treść karty nic nie robi. Notatka jest widoczna
  na karcie.
- Ozdobniki: kolorowy pasek statusu na karcie, kropki w nagłówkach, akcent u góry;
  kursor `hand`/`chwyt` na klikalnych elementach.

## CLI (dla AI i terminala)

```sh
omarchy-kanban task list              # 3 kolumny + licznik archiwum
omarchy-kanban task list --all        # razem z archiwum
omarchy-kanban task list --json
omarchy-kanban task add "Tytuł" --status todo      # todo (domyślne) | doing | done
omarchy-kanban task add "Tytuł" --status doing --notes "kontekst"
omarchy-kanban task move <id> done
omarchy-kanban task edit <id> --title "…" --notes "…"
omarchy-kanban task rm <id>
omarchy-kanban todoist status [--json]      # enabled / last_sync / błąd / liczba linków
omarchy-kanban todoist candidates [--json] # dziś + zaległe z Todoist, do decyzji AI
omarchy-kanban todoist import <id>… [--status todo|doing|done]
omarchy-kanban todoist sync                # wymuś cykl synchronizacji
```

`<id>` może być unikalnym prefiksem UUID. CLI rozmawia z lokalnym Core po
`http://127.0.0.1:4765` z tokenem z katalogu danych.

## Todoist (opcjonalnie)

Włączasz jednym skryptem; token nie trafia do repo, argv ani logów:

```sh
python3 todoist-token.py            # wklej token z Todoist → Ustawienia → Integracje
python3 todoist-token.py --remove   # wyłącz integrację
```

Skrypt waliduje token, zapisuje go w `~/.config/agent-kanban/todoist.env` (`0600`)
i podłącza do usługi drop-inem systemd (`EnvironmentFile=`), potem restartuje Core.
Zmienna środowiskowa: `TODOIST_API_TOKEN`. Brak tokenu = integracja wyłączona,
tablica działa normalnie.

Co robi integracja:

- **Import**: AI czyta `todoist candidates` i wciąga na TODO tylko zadania, które da
  się zrobić przy komputerze. Core sam nic nie importuje.
- **Auto-import (opcjonalnie)**: timer systemd co 2 h uruchamia
  `omarchy-kanban-todoist-import` — skrypt poza Core, który pyta model przez
  `opencode run`, waliduje odpowiedź i woła `todoist import`. Błędy idą tylko do
  journald, bez powiadomień. Konfiguracja: `KANBAN_IMPORT_MODEL` w unicie
  (`systemctl --user edit agent-kanban-todoist-import.service`), domyślnie
  `opencode/glm-5.3-flash-free`. Podgląd: `omarchy-kanban-todoist-import --dry-run`
  oraz `journalctl --user -u agent-kanban-todoist-import -n 50`. Wyłączenie:
  `python3 install.py --no-timer --yes`.
- **Statusy w obie strony**: karta → DONE zamyka zadanie w Todoist; zamknięcie
  w Todoist przenosi kartę na DONE; cofnięcie karty z DONE otwiera zadanie ponownie.
  Cykl leci co 60 s; ręcznie: `omarchy-kanban todoist sync`.
- **Konflikt**: wygrywa ręczna zmiana — karta ruszona po ostatniej synchronizacji nie
  jest nadpisywana przez Todoist.
- **Usuwanie**: `task rm` usuwa tylko kartę; zadanie w Todoist zostaje.
- **Czego nie synchronizuje**: tytułów i notatek (tylko statusy), zadań cyklicznych
  (kolejne wystąpienie nie trafi na tablicę), reopen zrobiony w Todoist nie wskrzesza
  karty DONE; archiwum (48 h) jest tylko domykane po stronie Todoist.

Stan: `omarchy-kanban todoist status` (`enabled`, `last_sync`, `last_error`, liczba
linków).

## Skill dla AI

`kanban-triage` decyduje, gdy zaczynasz nową pracę, czy dodać zadanie i do której
kolumny. Instalowany do `~/.config/opencode/skills/`. Źródło: `skill/kanban-triage/SKILL.md`.

## Instalacja / aktualizacja

```sh
python3 install.py --yes                       # buduje z źródeł i restartuje usługę
python3 install.py --no-widget --yes           # tylko Core
```

Wymagania: Rust/Cargo ≥ 1.89. Widget wymaga Omarchy/Quickshell.

## API (dla integracji)

Wszystkie żądania wymagają nagłówka `Authorization: Bearer <token>`
(token: `~/.local/share/agent-kanban/token`) i Host = `127.0.0.1`.

| Metoda | Ścieżka | Treść |
|---|---|---|
| GET | `/api/state` | cała tablica + liczniki |
| POST | `/api/tasks` | `{"title","notes?","status?"}` |
| PATCH | `/api/tasks/{id}` | `{"title?","notes?","status?","position?"}` |
| DELETE | `/api/tasks/{id}` | — |

## Dane i testy

- Baza: `~/.local/share/agent-kanban/board.sqlite3` (WAL).
- Token Core: `~/.local/share/agent-kanban/token` (sekret).
- Token Todoist (opcjonalnie): `~/.config/agent-kanban/todoist.env` (`0600`, tylko
  przez `EnvironmentFile=` usługi).
- Logi: `journalctl --user -u agent-kanban.service -n 100 --no-pager`.

```sh
cargo test --all-targets --locked
cargo clippy --all-targets --locked -- -D warnings
cargo build --release --locked
python3 tests/todoist_e2e.py        # import → close → reopen → pull na fałszywym Todoist
python3 tests/todoist_autoimport.py # auto-import: 18 scenariuszy na stubach modelu i CLI
python3 tests/install_test.py       # instalator w sandboxie (jednostki, timer, uprawnienia)
```

Katalogi `web/`, `tests/` i `README-legacy.md` pochodzą z poprzedniej wersji
(obserwator sesji OpenCode) i nie są już używane.
