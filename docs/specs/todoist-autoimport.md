# Auto-import z Todoist (timer + AI)

## 1. Cel i uzasadnienie

Dziś import z Todoist wymaga człowieka: AI w sesji woła
`omarchy-kanban todoist candidates`, ocenia zadania i importuje wykonalne.
Użytkownik chce, żeby działo się to samo, co 2 godziny, bez udziału człowieka —
karty z zadaniami „do zrobienia przy komputerze" mają czekać na tablicy.

Push/pull statusów (Core, pętla 60 s) działa już bez zmian. Ten dokument
dodaje **wyłącznie** automatyczną decyzję o imporcie.

## 2. Opis problemu

- Decyzja „czy zadanie da się zrobić przy komputerze" wymaga osądu — nie da się
  jej zapisać regułą po etykiecie czy projekcie.
- Core nie może wołać modelu: sekrety, koszt, opóźnienia i awarie modelu nie mogą
  dotykać pętli synchronizacji (decyzja z `docs/specs/todoist-sync.md` §4).
- Ręczne uruchamianie AI działa, ale wymaga obecności użytkownika.
- Awaria musi być widoczna: „brak kandydatów" (normalny stan) i „integracja
  wyłączona / token nieważny / model padł" (błąd) nie mogą wyglądać tak samo.

## 3. Przyjęte rozwiązanie

Osobny składnik poza Core: **timer systemd użytkownika → skrypt orkiestrujący →
model przez `opencode run` → CLI `omarchy-kanban todoist import`**.

1. Timer `agent-kanban-todoist-import.timer` budzi usługę co 2 godziny.
2. Usługa `agent-kanban-todoist-import.service` (`Type=oneshot`) uruchamia
   `~/.local/bin/omarchy-kanban-todoist-import`.
3. Skrypt sprawdza `omarchy-kanban todoist status --json`:
   `enabled=false` → wyjście 0 (integracja wyłączona), `last_error` →
   ostrzeżenie w logu (synchronizacja zgłasza problem, import i tak próbuje).
4. Skrypt pobiera `omarchy-kanban todoist candidates --json` (CLI → HTTP Core;
   token Todoist zostaje w usłudze Core, skrypt go nie widzi).
5. Skrypt buduje prompt z listą kandydatów (maks. 100, w kolejności z API) i
   woła `opencode run --standalone --model <model> "<prompt>"`, żądając
   **wyłącznie** JSON-a: `{"import":[{"id":"…","reason":"…"}]}`.
6. Skrypt waliduje odpowiedź i woła `omarchy-kanban todoist import <id>…`
   dla poprawnych pozycji.
7. Błędy: jedna linia w journald, kod wyjścia ≠ 0. Bez powiadomień.

## 4. Rozważane strategie implementacji (bez kodu)

| Wariant | Ocena |
|---|---|
| Endpoint `POST /api/todoist/autoimport` w Core, który woła model | **Odrzucony.** Cofa decyzję ze specyfikacji sync (model w Core = drugi sekret, koszt i awarie w pętli), dodaje powierzchnię HTTP i testy Rust bez zysku. |
| Reguła deterministyczna (etykieta/projekt) | **Odrzucony.** Użytkownik chce osądu; reguła wymaga dyscypliny w Todoist i myli się przy zadaniach bez etykiety. |
| Skrypt + timer systemd (wybrany) | Zero zmian w pętli synchronizacji, zero nowych zależności, awaria nie dotyka Core, harmonogram widoczny w `systemctl --user list-timers`. |
| Cron użytkownika | **Odrzucony.** Ten projekt już używa systemd user; mieszanie dwóch mechanizmów. |
| Model wołany bezpośrednio po HTTP (klucz OpenRouter) | **Odrzucony.** Nowy sekret do przechowywania; `opencode run` używa istniejącej konfiguracji użytkownika. |

## 5. Wzorce architektoniczne

- **Sidecar / orkiestrator poza domeną** — Core pozostaje jedynym właścicielem
  danych; skrypt używa wyłącznie publicznego CLI.
- **Niezaufane źródło** — model nie ma żadnych uprawnień; jego odpowiedź jest
  walidowana (id ∈ kandydaci, limity), a decyzje wykonuje skrypt.
- **Idempotencja przez domenę** — powtórzony import kończy się `already_linked`
  w `add_linked`; skrypt nie utrzymuje własnego stanu.

## 6. Architektura

```
systemd user ──timer (co 2 h)──> agent-kanban-todoist-import.service (oneshot)
                                        │
                                        ▼
                        ~/.local/bin/omarchy-kanban-todoist-import   (Python 3, stdlib)
                          │                    │                     │
                          │ status/candidates  │ opencode run        │ import <id>…
                          ▼                    ▼                     ▼
                   Core 127.0.0.1:4765    model (domyślnie     Core → Todoist API
                   (token Todoist tam)    opencode/glm-5.3-flash-free)
```

Nowe pliki:

| Plik | Rola |
|---|---|
| `todoist-autoimport.py` | skrypt orkiestrujący (źródło w repo) |
| `agent-kanban-todoist-import.service` | unit generowany przez `install.py` |
| `agent-kanban-todoist-import.timer` | unit generowany przez `install.py` |
| `tests/todoist_autoimport.py` | test procesowy ze stubami |

Zmieniane:

| Plik | Zmiana |
|---|---|
| `install.py` | instaluje skrypt do `~/.local/bin` (0755) i jednostki, `--no-timer` |
| `src/main.rs` | flaga `--json` w `todoist status` |
| `tests/install_test.py` | obejmuje timer shimem i asercją na treść unitu |
| `README.md` | sekcja o auto-imporcie, konfiguracja modelu, podgląd logów |

## 7. Interfejs

### 7.1 CLI

Nowa flaga istniejącej komendy:

```sh
omarchy-kanban todoist status --json   # {"enabled":bool,"last_sync":int|null,"last_error":str|null,"linked":int}
```

Skrypt:

```sh
omarchy-kanban-todoist-import            # pełny cykl (timer)
omarchy-kanban-todoist-import --dry-run  # pokaż decyzję modelu, nic nie importuj
```

Zmienne środowiskowe (domyślne w skrypcie, nadpisywane w unicie/drop-inie):

| Zmienna | Domyślnie | Znaczenie |
|---|---|---|
| `KANBAN_IMPORT_MODEL` | `opencode/glm-5.3-flash-free` | model dla `opencode run` |
| `KANBAN_IMPORT_LIMIT` | `10` | maks. liczba importów w jednym cyklu |
| `KANBAN_IMPORT_MAX_CANDIDATES` | `100` | ile kandydatów trafia do promptu |
| `KANBAN_BIN` / `OPENCODE_BIN` | `omarchy-kanban` / `opencode` | nadpisania ścieżek (testy, nietypowa instalacja) |

### 7.2 Prompt (stały tekst w skrypcie)

Tekst jest częścią kontraktu: zmiana treści = zmiana zachowania. Zmienne
wstawiane są tylko w miejscu `<<<ZADANIA>>>`.

```text
Jesteś filtrem zadań. Otrzymasz listę zadań z Todoist (JSON).
Zdecyduj, które z nich da się wykonać przy komputerze, siedząc przed nim.

Komputerowo wykonalne: pisanie, kodowanie, wysyłanie maili i ofert, zamawianie
rzeczy online, wypełnianie formularzy, research, rozmowy, płatności, rezerwacje.
Niewykonalne: prace fizyczne w domu i warsztacie (sprzątanie, mycie, malowanie,
odkamienianie, naprawy, montaż), wizyty, zakupy w sklepie stacjonarnym.
Brak opisu nie jest błędem — oceń po treści zadania.

Zasady odpowiedzi:
- Zwróć WYŁĄCZNIE obiekt JSON, bez prozy, komentarzy i bloków markdown.
- Format: {"import":[{"id":"<id>","reason":"<jedno zdanie, max 120 znaków>"}]}
- "id" kopiuj dosłownie z listy; nigdy nie wymyślaj ani nie modyfikuj id.
- Zadanie niewykonalne po prostu pomiń (nie wypisuj go).
- Gdy nic nie jest wykonalne: {"import":[]}

Zadania:
<<<ZADANIA>>>
```

Zadania wstawiane jako JSON, każde z polami: `id`, `content`, `description`
(przycięte do 300 znaków), `due`. Bez etykiet, priorytetu i projektu — nie
wpływają na decyzję, a zwiększają wariancję.

### 7.3 Kontrakt odpowiedzi modelu

Zasady parsowania i walidacji:

1. Znajdź pierwszy `{` w odpowiedzi i parsuj od niego
   `json.JSONDecoder().raw_decode` (stdlib) — proza wokół i nawiasy w stringach
   nie psują parsowania.
2. Brak JSON-a, brak klucza `import`, `import` nie jest listą, albo lista
   istnieje, lecz **żaden** element nie przeszedł walidacji → **błąd cyklu**:
   log + exit 1 (to awaria warstwy AI, nie „brak zadań"). Wyjątek: `import: []`
   to poprawna odpowiedź „nic nie jest wykonalne" → exit 0.
3. Element niebędący obiektem, `id` niebędący stringiem albo `id` spoza
   kandydatów z **tego samego** wywołania → element odrzucony, log z powodem
   (`spoza_kandydatow`, `duplikat`, `zly_ksztalt`).
4. Duplikaty `id` → jedno wystąpienie (pierwsze).
5. Kolejność zachowana jak w odpowiedzi modelu; po deduplikacji lista
   przycinana do `KANBAN_IMPORT_LIMIT` (nadmiar logowany jako `ponad_limit`).
6. `reason` jest opcjonalne (brak = `""`), przycinane do 120 znaków, trafia
   tylko do logu. Nie jest przekazywane do Core.
7. Odrzucenie jest **per element**: poprawne id są importowane, reszta logowana.
   Gdy model zwrócił same niepoprawne elementy → brak wywołania `import`,
   exit 1 (§7.3 pkt 2). Pusta lista `import: []` → exit 0.

### 7.4 Jednostki systemd

```ini
# agent-kanban-todoist-import.service
[Unit]
Description=Kanban: auto-import z Todoist (AI)
After=network.target
ConditionPathExists=%h/.local/bin/omarchy-kanban-todoist-import

[Service]
Type=oneshot
ExecStart=%h/.local/bin/omarchy-kanban-todoist-import
Environment="PATH=…"
Environment=OPENCODE_BIN=…
Environment=KANBAN_IMPORT_MODEL=opencode/glm-5.3-flash-free
StandardOutput=journal
StandardError=journal
SyslogIdentifier=kanban-todoist
TimeoutStartSec=15min
UMask=0077

# agent-kanban-todoist-import.timer
[Unit]
Description=Kanban: auto-import z Todoist co godzinę (07:00–23:00)

[Timer]
OnCalendar=*-*-* 07..23:00:00
Persistent=true
AccuracySec=1min
RandomizedDelaySec=5min
Unit=agent-kanban-todoist-import.service

[Install]
WantedBy=timers.target
```

- `PATH` — ta sama wartość, którą `install.py` wpisuje do usługi Core.
- `OPENCODE_BIN` — `shutil.which("opencode")` w chwili instalacji; gdy nie
  znaleziony, wpisujemy `opencode` i polegamy na `PATH`.
- Service nie ma `[Install]` (aktywuje go timer). `TimeoutStartSec` jest
  jedynym działającym limitem czasu: `RuntimeMaxSec` systemd ignoruje dla
  `Type=oneshot` (ostrzeżenie w `systemd-analyze verify`).
- Token Todoist **nie** trafia do tych jednostek.
- Timer uruchamia cykl o pełnych godzinach lokalnego czasu od 07:00 do 23:00
  (włącznie); między 00:00 a 06:59 nie ma planowych uruchomień.
- `Persistent=true` nadrabia **jedno** pominięte uruchomienie, gdy timer zostaje
  aktywowany po starcie komputera lub wznowieniu. Jeśli to nastąpi w porze nocnej,
  skrypt pomija cykl i następne uruchomienie następuje o 07:00. Nie tworzy się
  kolejka zaległych wywołań.

### 7.5 Instalacja (`install.py`)

1. `private_write` skryptu do `~/.local/bin/omarchy-kanban-todoist-import` +
   `chmod 0755`.
2. `private_write` obu jednostek do `~/.config/systemd/user/`.
3. `systemctl --user daemon-reload`.
4. `systemctl --user enable --now agent-kanban-todoist-import.timer`.
5. `systemctl --user restart agent-kanban-todoist-import.timer` (przeliczenie
   harmonogramu po zmianie `OnCalendar`).
6. `loginctl enable-linger "$USER"` — bez tego timer staje po wylogowaniu.
   Nieudane wywołanie (brak `loginctl`, polkit) → ostrzeżenie, nie błąd instalacji.
7. `--no-timer`: pomija kroki 2–6 i wykonuje
   `systemctl --user disable --now agent-kanban-todoist-import.timer` (jeśli
   timer istnieje), żeby wyłączony nie został aktywny z poprzedniej instalacji.

Drop-in `10-todoist.conf` (token) jest osobnym plikiem i przeżywa reinstalację.

## 8. Przepływy danych

### 8.1 Cykl (co godzinę, 07:00–23:00)

1. Poza lokalnym oknem 07:00–23:00 zaloguj pominięcie i wyjdź 0 (chroni przed
   nocnym uruchomieniem nadrabiającym po starcie/wznowieniu).
2. `flock -n` na `$XDG_RUNTIME_DIR/kanban-todoist-import.lock`; gdy zmienna
   pusta — `/run/user/<uid>`, a gdy katalog niedostępny — katalog tymczasowy.
   Zajęty → log „poprzedni cykl trwa", exit 0.
3. `todoist status --json` (timeout 30 s). `enabled=false` → log, exit 0.
   `last_error` → log ostrzeżenia, kontynuuj.
4. `candidates --json` (timeout 30 s). Pusto → log, exit 0 bez wołania modelu.
5. Prompt (maks. `KANBAN_IMPORT_MAX_CANDIDATES` zadań; nadmiar → log
   `kandydaci=N do_promptu=M`) → `timeout 180 opencode run --standalone
   --model <model> <prompt>`. Błąd/timeout → log, exit 1.
6. Parsowanie i walidacja (§7.3). Zero poprawnych id przy niepustej odpowiedzi
   modelu → log, exit 1; `import: []` → log, exit 0.
7. `--dry-run`: wypisz decyzję, exit 0, bez importu.
8. `import <id>…` (timeout 120 s, jedno wywołanie). Wynik CLI logowany.
9. Podsumowanie w jednej linii:
   `todoist-import: cykl kandydaci=N do_promptu=M import=K odrzucone=J`.

Każda linia logu ma prefiks `todoist-import:` (journald dodaje jeszcze
`SyslogIdentifier=kanban-todoist`).

### 8.2 Awarie

| Sytuacja | Zachowanie |
|---|---|
| Core nie działa (`connection refused`) | log, exit 1 |
| Brak tokenu Todoist (`enabled=false`) | log, exit 0 |
| Token nieważny (`last_error` z 401) | log ostrzeżenia, próba `candidates`; błąd HTTP → exit 1 |
| `candidates` zwraca błąd HTTP (5xx, DB locked) | log, exit 1, brak modelu |
| `opencode` nieobecny / brak credentials / nieznany model | log, exit 1 |
| Model: timeout, 429, błąd procesu | log, exit 1 |
| Odpowiedź bez JSON-a / zły kształt | log, exit 1 |
| Model zwrócił nieznane id | import poprawnych, log odrzuconych, exit 0 |
| Model zwrócił wyłącznie niepoprawne elementy | log powodów, exit 1 |
| Model zwrócił pustą listę | log, exit 0 |
| Import częściowo się nie udał (np. `already_linked`) | log raportu CLI, exit 0 |
| Import: błąd HTTP | log, exit 1 |

Kody wyjścia CLI, na których opiera się skrypt: `0` gdy operacja się odbyła
(także gdy wszystkie pozycje pominięto), `≠0` przy błędzie transportu/HTTP.

Brak powiadomień desktopowych. Jedyny kanał: journald.

## 9. Wymagania niefunkcjonalne

- **Zero nowych zależności** — Python 3 stdlib; w Rust tylko flaga `--json`
  (bez nowych crate'ów).
- **Sekrety** — skrypt nie czyta tokenu Todoist ani tokenu Core (CLI robi to sam);
  token nie trafia do argv, unitu ani logu. `reason` z modelu nie trafia do Core.
- **Izolacja** — awaria skryptu nie dotyka `agent-kanban.service`.
- **Odporność** — `flock` + `TimeoutStartSec` + timeouty na każde wywołanie
  zewnętrzne; brak kumulacji po suspendzie (`Persistent=true` nadrabia jedno
  uruchomienie).
- **Koszt** — 12 wywołań modelu dziennie, prompt ≤ 100 kandydatów × ~300 znaków.
- **Limity Todoist** — kilka żądań na cykl, daleko od 1000/15 min.
- **Observability** — jedna linia podsumowania na cykl; podgląd
  `journalctl --user -u agent-kanban-todoist-import -n 50`.

## 10. Strategia testowania i przypadki testowe

Test procesowy `tests/todoist_autoimport.py` (Python, stdlib, jak
`tests/todoist_e2e.py`): podmienia `KANBAN_BIN` i `OPENCODE_BIN` na stuby
w katalogu tymczasowym; realny model nie jest wołany.

| # | Scenariusz | Oczekiwanie |
|---|---|---|
| 1 | Dwóch kandydatów, model wskazuje jednego | jedno wywołanie `import` z tym id |
| 2 | Mieszanka: jedno id poprawne, jedno spoza kandydatów | import tylko poprawnego, log odrzucenia, exit 0 |
| 3 | Model zwraca prozę + JSON w środku | JSON sparsowany, import wykonany |
| 4 | `reason` zawiera nawiasy/escape | JSON sparsowany poprawnie |
| 5 | Pusta lista kandydatów | zero wywołań `opencode`, exit 0 |
| 6 | `enabled=false` | zero wywołań `opencode` i `candidates`, exit 0 |
| 7 | `candidates` kończy się kodem ≠0 | exit 1, brak `opencode` |
| 8 | `opencode` kończy się kodem 1 | brak importu, exit 1 |
| 9 | Model zwraca więcej id niż limit | import przycięty do limitu |
| 10 | Duplikaty id w odpowiedzi | jedno wywołanie `import` z jednym id |
| 11 | Ponad `KANBAN_IMPORT_MAX_CANDIDATES` kandydatów | prompt zawiera limit, log pominiętych |
| 12 | `--dry-run` | brak wywołania `import`, decyzja wypisana |
| 13 | Zajęty lock | exit 0, brak wywołań |
| 14 | Odpowiedź bez JSON-a | brak importu, exit 1 |
| 15 | `import` kończy się kodem ≠0 | exit 1, log raportu |
| 16 | `reason` nie trafia do argv `import` | asercja na argumenty stuba |
| 17 | Wszystkie id odrzucone walidacją | brak `import`, exit 1, log powodów |
| 18 | `last_error` w statusie, kandydaci OK | ostrzeżenie w logu, cykl kończy się importem |
| 19 | Godzina lokalna poza 07:00–23:00 | exit 0 przed CLI/model; brak importu |
| 20 | Timer aktywowany po starcie/wznowieniu po pominiętym ticku | jedno catch-up przez `Persistent=true`; bez kolejki zaległych |

`tests/install_test.py`: shim `systemctl`/`loginctl` obejmuje `enable`, `restart`
i `enable-linger`; asercje na treść `.timer` i `.service` oraz `chmod 0755`
skryptu.

Weryfikacja ręczna po instalacji: `omarchy-kanban-todoist-import --dry-run`,
potem `journalctl --user -u agent-kanban-todoist-import -n 20`.

## 11. Decyzje użytkownika

1. Auto-import ma działać **co godzinę od 07:00 do 23:00 lokalnego czasu**;
   w nocy nie wykonuje cykli.
2. Po starcie/wznowieniu timer ma jednorazowo nadrobić pominięty cykl
   (`Persistent=true`), bez nadrabiania wielu pominiętych ticków.
3. Przy błędzie **bez powiadomień** — tylko log w journald.
4. Model konfigurowalny; domyślnie darmowy z opencode, bo `typesafe/jev-router`
   z OpenRouter nie ma aktywnych endpointów (sprawdzone: `endpoints: []`,
   `~typesafe/jev-latest` nie istnieje w tej instalacji opencode).
5. Decyzja o imporcie pozostaje poza Core (skrypt + timer), zgodnie z decyzją
   ze specyfikacji sync.

## 12. Otwarte pytania i decyzje

- **Niedeterminizm.** `opencode run` nie udostępnia `temperature`/`seed`;
  stabilność decyzji zapewnia stały prompt (§7.2) i przypięty model, nie
  powtarzalność. Zmiana modelu = zmiana zachowania.
- **Zadania cykliczne.** Kandydaci to `today | overdue`; zadanie cykliczne po
  zamknięciu wraca z nowym id i może wrócić na tablicę po imporcie. Świadome:
  karta w DONE nie jest duplikowana, ale ten sam tytuł może pojawić się drugi raz.
- **Odrzucone zadania wracają do promptu co 2 h** — koszt tokenów, nie
  poprawność. Główna motywacja przyszłego stanu „seen/skipped" w `meta`.
- **Follow-up (świadomie poza zakresem):** trwały stan odrzuconych, powiadomienia,
  fallback między modelami, filtry per projekt, `is_recurring` w kandydatach.
- Model domyślny można zmienić jedną linią w unicie; nie blokuje wdrożenia.

## Runda recenzji: 2
