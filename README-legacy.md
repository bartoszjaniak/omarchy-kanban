# Archiwum: Agent Kanban task runner (0.2.0, obecnie tylko legacy-serve)

Rozwinięcie istniejącego POC widgetu. **Rust Core + SQLite + OpenCode ACP**, widget
Omarchy/Quickshell i pełny interfejs w przeglądarce. Core jest niezależny od UI.
To działający pierwszy pionowy wycinek, **nie komplet wszystkich funkcji specyfikacji**.

## Co działa

- Backlog → TODO → In Progress → Czeka na Ciebie / Błąd → DONE.
- Trwała baza, rewizje kart, idempotentne komendy i dziennik zdarzeń.
- Osobne sesje ze wskazanym agentem i katalogiem na Core; kontynuacja rozmowy.
- Postęp narzędzi i tekst, zgody oraz formularze ACP z walidacją JSON Schema.
- Równoległość (domyślnie 2), blokady pokrywających się katalogów/repozytoriów,
  limity czasu i ręcznych prób. Czekanie na człowieka nie zużywa aktywnego czasu.
- Harmonogram codzienny ze strefą IANA i interwał minutowy. Przestoje Core są
  scalane do jednego aktualnego wystąpienia. Otwarta karta poprzedniego wykonania
  (także błąd) blokuje następne — trzeba ją rozstrzygnąć lub anulować.
- Lokalny Core albo klient przez istniejący SSH, także po Tailscale.
- Wspólne dane na wielu monitorach; widget ma cache i trwałe lokalne szkice.
  Ich wysłanie do Backlogu jest osobną czynnością, także po powrocie online.
- Powiadomienia `notify-send` o nowych decyzjach, błędach i DONE, deduplikowane
  między klientami tego samego profilu. Brak odtwarzania lawiny starych popupów.
- Instalacja usługi konta użytkownika na Linux/macOS, backup przed aktualizacją
  oraz opcjonalna instalacja źródłowa Core przez SSH po zatwierdzeniu.

## Instalacja lokalna — Omarchy

Wymagania: **Rust/Cargo ≥ 1.89**, Python ≥ 3.9, OpenCode V2 skonfigurowany
i zalogowany u tego samego użytkownika, Quickshell/Omarchy dla widgetu.
Sprawdzony handshake i konfiguracja sesji z OpenCode **2.0.29**. Integracja
modeli/zewnętrznych narzędzi zależy od Twojej konfiguracji OpenCode.

```sh
python3 install.py --yes
```

Instalator buduje wersję release, robi backup, instaluje i uruchamia
`agent-kanban.service`, zapisuje prywatny profil klienta, aktualizuje istniejący
`bart.kanban` i dodaje aplikację **Agent Kanban** do menu. Zachowuje pozycję
dotychczasowego widgetu. Nie uruchamia agentów bez zadań dopuszczonych do TODO.
Nie modyfikuje `/usr/share/omarchy`, SSH, VPN ani uprawnień OpenCode.

Po instalacji kliknij widget → **Pełny widok**. Zaloguj się tokenem z:

```sh
cat ~/.local/share/agent-kanban/token
```

Token jest sekretem — nie publikuj go ani nie wklejaj do rozmowy z agentem.
W przeglądarce jest przechowywany w cookie HttpOnly/SameSite, nie w URL.
Alternatywnie: uruchom aplikację z menu lub `~/.local/bin/omarchy-kanban open`.
To ostatnie polecenie utrzymuje proces klienta/tunel do przerwania Ctrl+C.
Zamknięcie samej karty przeglądarki nie zamyka tunelu ani Core.

### Pierwsze zadanie

1. **＋ Zadanie** w pełnym widoku: tytuł, instrukcja, istniejący katalog na Core,
   agent (np. `plan`) i kryteria wyniku.
2. Zapisz do Backlogu. Otwórz kartę i sprawdź kontrakt.
3. **Dopuść → TODO** oznacza zgodę na rozpoczęcie pracy.
4. W karcie zobaczysz tekst, aktywność oraz formularze i zgody.
5. Po zakończeniu tury sprawdź wynik i wybierz **Akceptuj → DONE** lub napisz
   kolejną wiadomość w tej samej sesji.

Szybkie dodawanie w widgecie tworzy lokalny szkic. Po wysłaniu do Backlogu trzeba
uzupełnić instrukcję, katalog, agenta i kryteria w pełnym widoku. Edycja kontraktu
w Backlogu rozpoczyna nowy kontekst wykonania (nowa sesja i licznik prób);
wcześniejsze powiązania sesji pozostają w dzienniku.

## Core na Macu / przez SSH

Najpierw skonfiguruj i sprawdź zwykłe `ssh mac-mini` (alias w `~/.ssh/config`),
klucz hosta w known_hosts i uwierzytelnianie przez klucz/ssh-agent. Tailscale
ma zapewniać osiągalność hosta; nie jest instalowany ani konfigurowany przez aplikację.

Na Macu muszą być Python, Rust/Cargo i OpenCode z dostawcą/agentami. Opcjonalna
zdalna instalacja **źródłowa** (nie uruchamialiśmy jej na Twoim Macu):

```sh
python3 install.py --ssh mac-mini --yes
```

Komenda wysyła źródła do osobnego katalogu staging na Macu, buduje natywną
binarkę, instaluje usługę użytkownika, sprawdza jej zdrowie i zapisuje lokalny
prywatny profil połączenia. Lokalny Core nie jest automatycznie zatrzymywany
ani kasowany przy przełączeniu profilu. Możesz go wyłączyć samodzielnie:

```sh
systemctl --user disable --now agent-kanban.service
```

macOS: instalacja używa **LaunchAgent w zalogowanej sesji użytkownika**. Nie jest
to jeszcze usługa działająca przed logowaniem. FileVault, Keychain i usypianie
Maca wymagają osobnego sprawdzenia. Nie obiecujemy wykonywania zadań podczas snu.

Można też zainstalować Core bezpośrednio na hoście:

```sh
python3 install.py --core-only --yes
```

Klient istniejącego Core można skonfigurować bez reinstalacji. `CORE_UUID`
odczytasz z pliku `identity.json` w katalogu danych Core, logu startowego
albo pola `core_id` autoryzowanego `/api/state`.
Skopiuj token bezpiecznie do prywatnego pliku lokalnego, a następnie:

```sh
~/.local/bin/omarchy-kanban connect --ssh mac-mini --core-id CORE_UUID --token-file /prywatna/sciezka/token
python3 install.py --widget-only --yes
```

Nie używamy agent forwarding, wyłączania kontroli klucza hosta ani publicznego
nasłuchu API. Zmiana/nieznany klucz hosta wymaga konfiguracji poza aplikacją.
Backendy, agenci, projekty i narzędzia są przy Core; nie synchronizujemy projektów.

## Uruchomienie bez instalacji / środowisko testowe

```sh
cargo build --locked
target/debug/omarchy-kanban serve --data-dir /tmp/opencode/kanban-local-test --port 4766 --opencode /pelna/sciezka/opencode
```

Otwórz `http://127.0.0.1:4766` i użyj tokenu z wybranego katalogu danych.
Wystawiony jest tylko loopback. Backend nie wymaga środowiska graficznego.
Opcje: `serve --help`, `bridge --help`, `connect --help`, `open --help`.

## Testy

```sh
cargo test --all-targets --locked
cargo clippy --all-targets --locked -- -D warnings
cargo build --locked
python3 tests/e2e.py
python3 tests/install_test.py
node --check web/app.js
# Opcjonalnie Chromium + Node ≥ 22:
node tests/ui.mjs
```

E2E uruchamia prawdziwy Rust Core z deterministycznym procesem testowym ACP:
uprawnienia/CSRF, idempotencja, walidacja gotowości i formularzy, zgody, postęp,
równoległość, kontynuacja, anulowanie, awaria, restart, cache wielu klientów
i szkice offline. **Nie używa modelu ani płatnych wywołań**. Podczas prac
osobno sprawdzono prawdziwy ACP: initialize, session/new i wybór trybu plan,
bez wysyłania promptu do modelu. Cały rzeczywisty przebieg z modelem sprawdzamy
dopiero na świadomie dodanym zadaniu.

Test instalatora używa tymczasowego HOME, prawdziwego Core do health-check
i symulowanych menedżerów usług/Omarchy. Sprawdza układ plików Linux/macOS,
prywatność tokenu i zachowanie kopii POC; nie jest testem prawdziwego launchd.

## Dane, aktualizacja i diagnostyka

- Linux Core: `$XDG_DATA_HOME/agent-kanban` lub `~/.local/share/agent-kanban`.
- macOS Core: `~/Library/Application Support/agent-kanban` (lub XDG override).
- Profil UI: `$XDG_CONFIG_HOME/agent-kanban/connection.json` — zawiera sekret.
- Stan klienta (cache/szkice widgetu): `$XDG_STATE_HOME/agent-kanban/client/<core-id>.sqlite3`
  lub `~/.local/state/agent-kanban/client/…`. Niewysłane szkice nie są traktowane
  jak dane cache do automatycznego czyszczenia.
- Rozmowy i sesje pozostają także w danych OpenCode. Backup samej bazy kanbana
  nie jest pełnym backupem projektów i historii OpenCode.
- Linux log: `journalctl --user -u agent-kanban.service -n 100 --no-pager`.
- macOS log: katalog danych Core, `core.stderr.log`.

Ponowna instalacja robi spójny backup SQLite i kopie konfiguracji. Przed restartem
ustawia barierę maintenance, zatrzymując podejmowanie nowych zadań, i odmawia
aktualizacji przy aktywnych wykonaniach/decyzjach. Nie przerywa ich w ciemno.
Nie ma jeszcze automatycznego rollbacku aktualizacji. Jeżeli instalacja zakończy
się błędem po barierze, sprawdź usługę i log; restart Core usuwa barierę. Nie
restartuj Core z aktywnymi operacjami bez sprawdzenia ich skutków.

## Świadome ograniczenia tej wersji

- **Zawsze ręczna akceptacja wyniku.** `end_turn` nie jest automatycznym DONE.
- Osobny proces ACP/prywatny serwer na aktywne zadanie, zamiast wspólnego poola.
  Łatwiejsza izolacja anulowania, większy RAM. Do zmierzenia na Macu M1/8 GB.
- Po awarii Core/ACP stare zgody tracą ważność, a run trafia do Błędu. Nie ma
  automatycznego retry skutków ubocznych. Ręczne „Ponów” wraca do zapisanej sesji.
- Anulowanie próbuje najpierw ACP. Po 10 s bez potwierdzenia zamyka izolowany
  proces i oznacza błąd wymagający sprawdzenia, **nie gwarantuje cofnięcia ani
  zakończenia wszystkich zewnętrznych skutków/procesów narzędzi**.
- Widget synchronizuje pełne snapshoty co 2 s. Dziennik jest trwały i dostępny
  po kursorze w API, ale przesyłanie wyłącznie różnic to następny etap.
- Offline: widget ma trwały cache; załadowana strona zachowuje ostatni odczyt
  w pamięci i szkice w localStorage. Web UI nie jest jeszcze PWA do uruchamiania
  od zera bez dostępu do Core. Cache/szkice przeglądarki i widgetu są osobne.
- Pełny widok jest na razie aplikacją webową otwieraną z widgetu/menu, nie Tauri.
  Pierwsze logowanie wymaga jawnego tokenu. Tunel musi pozostać otwarty.
- Przycisk rozmowy/retro tworzy normalne zadanie z własną sesją. Nie ma jeszcze
  osobnego czatu dyspozytora, NLP do harmonogramów ani samodzielnego zbierania
  historii do retro. Informacje do analizy podajesz w instrukcji/plikach Core.
- Brak wdrożeń własnego kodu, e-maili, zespołów, routingu wielu silników,
  limitów kosztów, retencji logów i synchronizacji katalogów projektów.
- Instalator źródłowy, nie gotowe podpisane paczki/repozytorium wydań. Prebuilt
  binarkę można podać przez `--binary`. `python3 package.py` tworzy natywny bundle
  z binarką, UI, instalatorem i sumą SHA-256; po rozpakowaniu instalator wykrywa
  `bin/omarchy-kanban` i nie wymaga Rust. To nie podpisane wydanie uniwersalne:
  zgodność bibliotek systemowych i publiczna dystrybucja wymagają dalszych testów.
- Instalacja macOS i tunel do prawdziwego Maca wymagają testu na tym hoście.

**Bezpieczeństwo:** agenci mają uprawnienia użytkownika Core i polityki OpenCode.
To nie jest sandbox. Domyślny agent `plan` jest ostrożnym punktem startu, ale nie
zastępuje konfiguracji dostępu. Sprawdź szczególnie shell, sieć, odczyt sekretów
i uprawnienia własnych agentów. Nie uruchamiaj Core jako root. Nie włączamy
`--auto`. Zgoda „always” może zapisać szersze uprawnienia w OpenCode.
