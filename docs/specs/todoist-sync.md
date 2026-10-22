# Synchronizacja tablicy z Todoist

## 1. Cel i uzasadnienie

Tablica `TODO / In Progress / DONE` jest jedynym źródłem prawdy o tym, co robimy — ale
część zadań użytkownika żyje w Todoist (telefon, współdzielone projekty, zadania
cykliczne). Ręczne przepisywanie ich na tablicę to praca, której nikt nie wykonuje
regularnie. Efekt: tablica kłamie, a Todoist rośnie w zaległości.

Cel: jedno miejsce decyzyjne (tablica) i automatyczne odzwierciedlenie statusów w obie
strony. Import nowych zadań pozostaje decyzją AI — tylko ona potrafi ocenić, czy
zadanie „kup mleko" da się zrobić na komputerze.

## 2. Opis problemu

- Zadania z Todoist na dziś i zaległe nie trafiają na tablicę; użytkownik nie wie,
  co realnie może zrobić przy komputerze.
- Ukończenie zadania w jednym systemie nie jest widoczne w drugim.
- Cofnięcie karty z DONE („jednak tego nie zrobiłem") nie przywraca zadania
  w Todoist.
- Ręczne kopiowanie tytułów, terminów i statusów jest kosztowne i nieaktualne
  w ciągu godziny.

## 3. Przyjęte rozwiązania

1. **Import przez AI.** Core udostępnia listę kandydatów (zadania Todoist na dziś
   i zaległe, jeszcze niezlankowane). AI (sesja OpenCode, skill `kanban-triage`)
   ocenia, które da się wykonać na komputerze, i importuje je przez CLI. Core nie
   importuje nic automatycznie.
2. **Link 1:1 karta ↔ zadanie Todoist** przez kolumnę `tasks.todoist_id`
   (unikalny indeks częściowy). Bez osobnej tabeli mapowań.
3. **Stan zdalny i znacznik zdarzeń na karcie:**
   - `tasks.todoist_state` (`open` / `closed`, NULL = brak linku) — ostatni znany
     stan zadania w Todoist, daje idempotencję `close`/`reopen`;
   - `tasks.todoist_seen_at` (epoch, 0 = brak) — znacznik ostatniego **rozliczonego**
     ukończenia w Todoist (`completed_at`), żeby nakładające się okna zapytań nie
     stosowały tego samego zdarzenia dwa razy.
4. **Sync w tle w Core** (pętla co 60 s, obok istniejącej pętli archiwizacji).
   Push przed pullem; sieć nigdy pod lockiem bazy; jeden cykl naraz (single-flight).
5. **Konflikt: wygrywa ręczna zmiana.** Jeżeli karta była modyfikowana po ostatniej
   udanej synchronizacji, Todoist nie nadpisuje jej statusu — zamiast tego stan
   lokalny jest wypychany (`reopen`), a zdarzenie ukończenia jest oznaczane jako
   rozliczone.
6. **Archiwum:** karty `archived` są tylko pushowane (`close` zaległego zamknięcia),
   nigdy pullowane — archiwum nie wskrzeszamy.
7. **Usuwanie tylko lokalne.** `task rm` linkowanej karty usuwa ją z tablicy i nie
   dotyka Todoist (brak mechanizmu usuwania w widgecie, więc to ścieżka CLI).
8. **Token wyłącznie ze zmiennej środowiskowej** `TODOIST_API_TOKEN`. Brak tokenu =
   integracja wyłączona, Core działa bez zmian.
9. **Bez nowych zależności.** Czas w epoch sekundach + własny formatter ISO 8601 UTC
   (~20 linii).
10. **Bez zmian w widgecie.** Link do Todoist nie ma reprezentacji wizualnej; stan
    integracji widać przez `omarchy-kanban todoist status`.

## 4. Rozważane strategie implementacji (bez kodu)

| Wariant | Zalety | Wady | Decyzja |
|---|---|---|---|
| Sync wyłącznie na żądanie (CLI) | Zero pracy w tle, prosto | Ukończenie w Todoist nie zmieni karty, dopóki ktoś nie odpali komendy | odrzucone |
| Core jako pętla w tle | Działa bez udziału użytkownika i AI | Trzeba pilnować locków i błędów sieci | **przyjęte** |
| Todoist Sync API (`/sync`) | Incrementalny, jeden request | Format komend, `sync_token`, więcej stanu klienta | odrzucone — REST v1 wystarcza |
| REST API v1 (filter + completed + close/reopen) | Prosty kontrakt, mało stanu | Brak push-notyfikacji; polling | **przyjęte** |
| Watermark globalny (`meta.todoist_sync_at`) jako mechanizm decyzyjny | Jeden licznik | Gubi zmiany po częściowej awarii; nie odróżnia „zamknięte" od „trzeba zamknąć" | odrzucone; zostaje wyłącznie jako okno zapytania o ukończenia |
| Tylko watermark per karta, bez `todoist_state` | Jeden mechanizm | Nie odróżnia „już zamknięte" od „trzeba zamknąć" → close w pętli | odrzucone; potrzebne oba |
| Automatyczny import wszystkich „dziś/zaległe" | Zero decyzji | Tablica zapełnia się zadaniami, których nie da się zrobić przy komputerze | odrzucone (decyzja użytkownika) |
| LLM wywoływany z Core | Pełna autonomia | Drugi klucz API, koszt, opóźnienia, brak kontroli | odrzucone |

## 5. Wzorce architektoniczne

- **Snapshot → sieć → zapis.** Jedno krótkie zajęcie `Mutex<Store>` na odczyt,
  żądania HTTP bez locka, jedno krótkie zajęcie na zapis. `std::sync::Mutex`
  i `rusqlite` są synchroniczne; guard nie może przeżyć `.await`.
- **Maszyna stanów per karta** zamiast porównywania zegarów:
  `(status lokalny, todoist_state)` → akcja; `todoist_seen_at` → czy zdarzenie
  zdalne już rozliczone.
- **Idempotentne przejścia.** Powtórzony cykl nie wysyła drugi raz tego samego
  `close`/`reopen`; powtórzone okno zapytania nie stosuje drugi raz ukończenia.
- **Compare-and-set na statusie.** Pull zapisuje status tylko wtedy, gdy karta nie
  zmieniła się od snapshotu (`WHERE updated_at = <snapshot>`). Zapis stanu zdalnego
  (`todoist_state`) nie jest CAS-owany — to metadana o Todoist, nie decyzja
  użytkownika.
- **Single-flight.** Pętla i endpoint `POST /api/todoist/sync` współdzielą
  `tokio::sync::Mutex`; drugi wywołujący dostaje `busy` zamiast drugiego cyklu.
- **Częściowy unikalny indeks** `tasks(todoist_id) WHERE todoist_id IS NOT NULL`
  blokuje podwójny import tego samego zadania.
- **Miękka degradacja.** Brak tokenu, brak sieci, 401, 429, 5xx: integracja milczy,
  Core i tablica działają dalej; błąd widoczny w `todoist status` i logu.

## 6. Architektura

```
┌──────────────────────────── Core (proces omarchy-kanban serve) ─────────────────────────┐
│  axum router ── /api/state, /api/tasks…  (istniejące)                                    │
│              └─ /api/todoist/{status,candidates,import,sync}  ← nowe                    │
│  retention_loop (60 s, istnieje)                                                          │
│  todoist_loop (60 s, nowy) ── snapshot ──► TodoistClient (reqwest, bez locka) ──► zapis  │
│  Store (SQLite): tasks + todoist_id, todoist_state, todoist_seen_at;                    │
│                  meta: todoist_sync_at, todoist_last_error                               │
└──────────────────────────────────────────────────────────────────────────────────────────┘
        ▲ HTTP + Bearer token Core                        │ HTTPS + Bearer TODOIST_API_TOKEN
        │                                                  ▼
  CLI `omarchy-kanban todoist …`                     api.todoist.com/api/v1
  (AI przez shell)                                    (filter, completed, close, reopen)
        ▲
  widget Omarchy (bez zmian)
```

Nowe/zmienione pliki:

| Plik | Zmiana |
|---|---|
| `src/todoist.rs` | nowy: klient REST v1, `iso8601`, `sync_once`, testy ze stubem |
| `src/board.rs` | migracja kolumn, pola w `Task`, `add()` z jawną listą kolumn, metody Store, 4 endpointy, pętla sync |
| `src/main.rs` | podkomendy `todoist status\|candidates\|import\|sync` |
| `todoist-token.py` | nowy skrypt: token → `EnvironmentFile` + drop-in systemd |
| `skill/kanban-triage/SKILL.md` | sekcja importu z Todoist |
| `README.md` | dokumentacja integracji, tokenu, ograniczeń |

## 7. API / Interfejs

### 7.1 API Core (wymaga `Authorization: Bearer <token Core>`, Host = loopback)

| Metoda | Ścieżka | Treść / odpowiedź |
|---|---|---|
| GET | `/api/todoist/status` | `{enabled, last_sync, last_error, linked}` |
| GET | `/api/todoist/candidates` | `{tasks:[{id, content, description, due, labels, priority, project_id}]}` — na dziś i zaległe, bez już zlinkowanych |
| POST | `/api/todoist/import` | `{"ids":["…"],"status":"todo"}` → `{imported:[task], skipped:[{id,reason}]}` |
| POST | `/api/todoist/sync` | wymusza cykl → `{closed, reopened, pulled, unlinked, errors:[…]}` lub `{busy:true}` |

### 7.2 API Todoist (v1, `TODOIST_API_URL` nadpisywalny dla testów)

| Metoda | Ścieżka | Użycie |
|---|---|---|
| GET | `/tasks/filter?query=today%20%7C%20overdue&limit=200&cursor=…` | kandydaci do importu |
| GET | `/tasks/{id}` | import: autorytatywne dane zadania |
| GET | `/tasks/completed/by_completion_date?since=…&until=…&limit=200&cursor=…` | pull ukończeń |
| POST | `/tasks/{id}/close` | karta → DONE |
| POST | `/tasks/{id}/reopen` | karta wraca z DONE |

Paginacja kursorowa: `limit=200`, dopóki `next_cursor != null`. Limit dostawcy:
1000 żądań / 15 min — nasz cykl to 1–3 żądania.

Kontrakt odpowiedzi: `close`/`reopen` mogą zwrócić `200` **lub `204` bez ciała** —
sukces czytamy ze statusu HTTP, ciało parsujemy tylko przy błędzie. `close` już
zamkniętego zadania (`400`) traktujemy jak sukces (`state='closed'`); `reopen`
nieaktywnego/cyklicznego (`400`) jak sukces (`state='open'`); `404` = zadanie
zniknęło → odlinkowanie. `401`/`429`/`5xx`/timeout = błąd cyklu.

### 7.3 CLI

```sh
omarchy-kanban todoist status                 # enabled / last_sync / błąd / liczba linków
omarchy-kanban todoist candidates [--json]    # dziś + zaległe, do decyzji AI
omarchy-kanban todoist import <id> [<id>…] [--status todo|doing|done]
omarchy-kanban todoist sync                   # wymuś cykl i pokaż wynik
```

### 7.4 Skrypt tokenu

```sh
python3 todoist-token.py            # pyta o token (getpass lub stdin), waliduje, instaluje
python3 todoist-token.py --remove   # usuwa integrację i restartuje usługę
```

Skutki: plik `~/.config/agent-kanban/todoist.env` (`0600`, katalog `0700`) z
`TODOIST_API_TOKEN=…`, drop-in `~/.config/systemd/user/agent-kanban.service.d/10-todoist.conf`
z `EnvironmentFile=%h/.config/agent-kanban/todoist.env`, `daemon-reload`, `restart`,
health-check i `omarchy-kanban todoist status`. Token nigdy nie trafia do argv,
unit file ani logów.

## 8. Przepływy danych

### 8.1 Import (AI)

1. AI: `omarchy-kanban todoist candidates --json`.
2. Core: `GET /tasks/filter?query=today | overdue`, wszystkie strony; odrzuca id już
   zlinkowane; zwraca listę.
3. AI ocenia każde zadanie: „da się zrobić przy komputerze?" (tak → import; nie →
   pomija i mówi użytkownikowi dlaczego).
4. AI: `omarchy-kanban todoist import <id>…`.
5. Core: dla każdego id `GET /tasks/{id}` (404 → `skipped`), tworzy kartę
   `status=todo` (lub wskazany), `notes` = opis + `Termin: …` gdy jest,
   `todoist_id`, `todoist_state='open'`, `todoist_seen_at=now`. Duplikat →
   `skipped: already_linked`.

### 8.2 Cykl synchronizacji (`sync_once(at)`)

Wejście: `at` (epoch), klient, snapshot linków
`(id, todoist_id, status, updated_at, todoist_state, todoist_seen_at)` oraz
`last_sync` z `meta`.

1. **Push** (karty `todo|doing|done`, oraz `archived` — tylko w tę stronę):
   - `status ∈ {done, archived}` + `state != closed` → `POST close` → `state='closed'`
   - `status ∈ {todo, doing}` + `state == closed` → `POST reopen` → `state='open'`
   - `404` → odlinkuj (karta zostaje), `errors` += wpis
   - `400` → odpowiednio `closed`/`open` (już w tym stanie), bez błędu
   - błąd jednej karty nie przerywa cyklu; reszta kart leci dalej
2. **Pull**, tylko jeśli istnieje choć jedna karta `status != done|archived`
   z `todoist_id` (nie ma czego pullować → zero żądań):
   `GET /tasks/completed/by_completion_date?since=<last_sync - 300 s>&until=<at>`,
   `since = at - 7 dni` gdy brak `last_sync`.
   Dla każdego ukończenia `(todoist_id, completed_at)`:
   - `completed_at <= todoist_seen_at` → już rozliczone, pomiń;
   - karta nietknięta (`updated_at < last_sync`, a przy braku `last_sync` — zawsze
     „nietknięta") → `status='done'`, `done_at=at`, `state='closed'`,
     `todoist_seen_at=completed_at`;
   - karta ruszona ręcznie po `last_sync` → wygrywa tablica: `POST reopen`,
     `state='open'`, `todoist_seen_at=completed_at` (zdarzenie rozliczone, nie wróci).
3. **Zapis** (jedna transakcja): statusy i `done_at` przez compare-and-set
   (`WHERE id=? AND updated_at=<snapshot updated_at>`); `todoist_state`
   i `todoist_seen_at` bezwarunkowo; `unlink`; `meta.todoist_sync_at = at` tylko gdy
   cykl nie zwrócił błędów; `meta.todoist_last_error` ustawiany/wyczyszczony.
4. Log tylko przy zmianach lub błędach (bez tokenu).

### 8.3 Przejścia i przypadki brzegowe

| Sytuacja | Zachowanie |
|---|---|
| Karta → DONE | close w Todoist, `state='closed'` |
| Karta DONE → TODO/In Progress | reopen w Todoist, `state='open'` |
| Zamknięte w Todoist, karta nietknięta | karta → DONE (retencja 48 h działa dalej) |
| Zamknięte w Todoist, karta ruszona po ostatnim sync | reopen w Todoist, zdarzenie oznaczone jako rozliczone |
| To samo ukończenie widoczne w kolejnym oknie | `todoist_seen_at` blokuje powtórne zastosowanie |
| Zadanie usunięte w Todoist | karta zostaje, link usunięty, wpis w błędach |
| Karta w archiwum | tylko push `close`; nigdy pull |
| Zadanie cykliczne | close zamyka wystąpienie; karta zostaje DONE, brak pętli dzięki `state='closed'`; reopen może przesunąć termin — ograniczenie w README |
| Podwójny import | unikalny indeks + `skipped: already_linked` |
| Brak tokenu | `enabled=false`, zero żądań wychodzących |
| 401 / 429 / 5xx / timeout / brak sieci | cykl kończy się błędem, `last_sync` nie rośnie, następny tick ponawia; tablica działa |
| Karta zmieniona w trakcie cyklu | zapis statusu pominięty (compare-and-set), stan zdalny zapisany |
| Ręczne `POST /api/todoist/sync` w trakcie cyklu | `{busy:true}`, bez równoległego cyklu |
| Brak tokenu w trakcie działania (usunięty drop-in) | po restarcie usługi `enabled=false`; linki zostają w bazie |

## 9. Wymagania niefunkcjonalne

- **Bezpieczeństwo:** token tylko w env usługi (drop-in + `EnvironmentFile` 0600);
  brak tokenu w argv, URL, logach i odpowiedziach HTTP; nowe endpointy rejestrowane
  przed `.layer(guard)`, więc podlegają Bearer + loopback Host.
- **Odporność:** żaden błąd sieci nie może zatrzymać Core ani pętli; timeout żądania
  10 s, połączenie 5 s; `MissedTickBehavior::Skip`.
- **Locki:** brak `await` pod `MutexGuard`; jedno zajęcie locka na odczyt, jedno na zapis.
- **Współbieżność:** jeden cykl naraz (`tokio::sync::Mutex` + `try_lock`).
- **Wydajność:** jeden cykl = 1–3 żądania HTTP poza push/pull; bez N+1.
- **Zależności:** zero nowych w `Cargo.toml` (`Cargo.lock` bez zmian).
- **Zgodność wstecz:** `tasks` dostaje kolumny przez `ALTER TABLE` przy `Store::open`;
  stare karty nietknięte, `todoist_*` NULL/0; `add()` przechodzi na jawną listę
  kolumn; `row()`/`SELECT` obejmują nowe pola; widget bez zmian (nieznane pola
  ignoruje).
- **Język:** komunikaty CLI po polsku, kod i testy po angielsku (jak reszta repo).
- **Formatowanie:** `cargo fmt --check` oraz `cargo clippy -D warnings` czyste.

## 10. Strategia testowania i przypadki testowe

Stub Todoist jako serwer axum na `127.0.0.1:0` w `#[cfg(test)] mod tests`
w `src/todoist.rs` (crate jest bin-only, więc `tests/*.rs` nie zaimportuje modułów).
Base URL wstrzykiwany do `TodoistClient::new`; stub waliduje metodę, ścieżkę,
`Authorization` i ciało żądania. Czas sterowany wartością `at` przekazywaną do
`sync_once` oraz SQL-em w testach (jak `retention_only_archives_old_done`).

| # | Przypadek | Poziom |
|---|---|---|
| 1 | Migracja istniejącej bazy: kolumny dodane, dane zachowane, `add()` działa | Rust, `board` |
| 2 | Import tworzy kartę z linkiem, notatką i `state='open'` | Rust, `todoist` |
| 3 | Powtórzony import → `already_linked`, brak duplikatu | Rust, `todoist` |
| 4 | Karta → DONE wysyła `close` raz; asercja `state='closed'`; drugi cykl bez żądania | Rust, `todoist` |
| 5 | Karta z DONE → TODO wysyła `reopen` | Rust, `todoist` |
| 6 | Ukończenie w Todoist → karta DONE, `done_at=at`, `state='closed'` | Rust, `todoist` |
| 7 | Konflikt: karta ruszona po `last_sync` → `reopen`, status lokalny zachowany, zdarzenie nie wraca w kolejnym cyklu | Rust, `todoist` |
| 8 | Sekwencja na jednym linku: import → close → reopen (trzy cykle) | Rust, `todoist` |
| 9 | Dwie karty: jedna `done`, druga nietknięta → jedno żądanie `close`, zero dla drugiej | Rust, `todoist` |
| 10 | `404` na jednej karcie nie przerywa cyklu: druga dostaje `close`, `errors` ma 1 wpis | Rust, `todoist` |
| 11 | `400` na reopen (cykliczne) → `state='open'`, brak błędu | Rust, `todoist` |
| 12 | `429`/`5xx`/connection refused → błąd zapisany, `last_sync` bez zmian, Core żyje | Rust, `todoist` |
| 13 | Karta archiwalna + `state='open'` → `close`, ale pull jej nie tyka | Rust, `todoist` |
| 14 | Brak tokenu → `enabled=false`, zero żądań | Rust, `todoist` |
| 15 | Paginacja kandydatów: 2 strony zebrane | Rust, `todoist` |
| 16 | `iso8601`: 0, epoka, rok przestępny (3 asercje) | Rust, `todoist` |
| 17 | Pull pominięty, gdy nie ma kart do pullowania → zero żądań | Rust, `todoist` |
| 18 | Tytuł/notatki nie są synchronizowane (zmiana w Todoist nie rusza karty) | Rust, `todoist` |
| 19 | Reopen w Todoist nie wskrzesza karty DONE (brak żądania, karta bez zmian) | Rust, `todoist` |
| 20 | Token nieobecny w `cmdline`, stderr/logu i `todoist status` | proces, `tests/e2e.py` |
| 21 | Bonus: migracja na kopii realnej `board.sqlite3` | ręcznie, przed commitem |

Bramka jakości (komendy CI, nie przypadki testowe):

```sh
cargo fmt --check
cargo test --all-targets --locked
cargo clippy --all-targets --locked -- -D warnings
cargo build --release --locked
git diff --exit-code Cargo.lock          # zero nowych zależności
```

Świadomie nie testujemy: serwera Todoist, ich silnika zadań cyklicznych, każdego
wariantu 5xx osobno, 60-sekundowej pętli (testujemy `sync_once`), widgetu, składni
filtra `today | overdue` (ryzyko zewnętrzne).

## 11. Decyzje użytkownika

1. **Import tylko przez AI/CLI** — Core nie importuje automatycznie; AI decyduje,
   które zadania „da się zrobić na komputerze".
2. **Konflikt: wygrywa ręczna zmiana** — karta ruszona po ostatniej synchronizacji
   nie jest nadpisywana przez Todoist.
3. **Usuwanie:** brak mechanizmu usuwania w widgecie; `task rm` linkowanej karty
   usuwa ją tylko lokalnie, bez DELETE w Todoist.
4. **Repozytorium:** projekt nie był w gicie; założone repo, stan wyjściowy
   w commicie bazowym, praca na `feature/todoist-sync`.
5. **Klucz API:** przez zmienną środowiskową, ustawianą przygotowanym skryptem
   (`todoist-token.py`), bez wpisywania sekretu do repozytorium.

## 12. Otwarte pytania i decyzje

- **Zadania cykliczne:** po `close` Todoist przesuwa termin wystąpienia. Karta
  zostaje DONE, więc kolejne wystąpienie nie trafi na tablicę automatycznie.
  Świadome ograniczenie (test #11 pilnuje braku błędu i pętli).
- **Edycja tytułu/notatek** nie jest synchronizowana w żadną stronę — tylko statusy
  (test #18).
- **Reopen wykonany w Todoist** nie wskrzesza karty DONE — tablica jest
  autorytatywna dla stanu ukończenia (test #19).
- **Składnia filtra `today | overdue`** jest zewnętrznym kontraktem dostawcy; 400
  z API trafi do `todoist status`, kandydaci będą puste. Nie testujemy.

## Runda recenzji: 2
