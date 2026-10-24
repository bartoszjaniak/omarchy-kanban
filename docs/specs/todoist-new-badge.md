# Todoist `[NEW]` badge

## 1. Cel i uzasadnienie

Łatwo rozpoznać świeżo zaimportowane karty Todoist na tablicy bez zmiany ich
tytułów ani notatek. Badge `[NEW]` ma być widoczny przez 24 godziny od importu.

## 2. Opis problemu

Zaimportowana karta wygląda tak samo jak pozostałe. Użytkownik nie widzi od razu,
które zadania pojawiły się niedawno dzięki integracji.

## 3. Przyjęte rozwiązanie

Widget rysuje mały, nieinteraktywny badge `[NEW]` na karcie pochodzącej z Todoist,
jeśli od jej utworzenia minęło mniej niż 24 godziny.

- Rozpoznanie karty: `todoist_id != null`.
- Czas rozpoczęcia: istniejące `created_at` (epoch w sekundach), ustawiane przy
  utworzeniu/importowaniu karty.
- Ważność: `0 <= now - created_at < 86400` sekund.
- Działa w każdej widocznej kolumnie TODO, In Progress i DONE; ruch karty nie
  resetuje ani nie kończy okresu.
- Karty starsze niż 24 h nie mają badge’a. Istniejąca karta Todoist młodsza niż
  24 h w chwili aktualizacji widgetu również go pokaże.
- Badge znika przy najbliższym odświeżeniu czasu w widgetcie (maksymalnie około
  minutę po upływie 24 h). Nie ma akcji ręcznego ukrywania.

## 4. Rozważane strategie implementacji (bez kodu)

| Wariant | Ocena |
|---|---|
| Osobna kolumna `imported_at` i migracja SQLite | Odrzucony: `created_at` już oznacza moment utworzenia karty i wystarcza dla importu. |
| Prefiks `[NEW]` w tytule lub notatce | Odrzucony: zmienia dane użytkownika i mógłby być synchronizowany/edytowany jako treść. |
| Badge w QML wyliczany z `todoist_id` i `created_at` | Wybrany: bez zmian API, bazy, CLI i kontraktu synchronizacji. |

## 5. Wzorce architektoniczne

- **Dane pochodne w warstwie prezentacji** — badge nie jest zapisywany; widget
  wylicza go z pól karty.
- **Jedno źródło prawdy** — `created_at` i `todoist_id` z Core, bez drugiego
  stanu „seen/dismissed”.

## 6. Architektura

Zmiana ograniczona do `plugin/Widget.qml`:

1. Współdzielony zegar widgetu aktualizowany raz na minutę, gdy istnieje świeża
   karta Todoist; zatrzymany, gdy nie ma żadnej.
2. Badge `[NEW]` jako mały wiersz metadanych w karcie, przed tytułem. Tytuł i
   notatka pozostają bez zmian.
3. Widoczność zależna od `todoist_id`, `created_at` i zegara. Karty lokalne bez
   `todoist_id` nigdy nie dostają badge’a.

## 7. Interfejs

### Widok karty

- Tekst badge’a: dokładnie `[NEW]`.
- Pozycja: osobny, krótki wiersz przed tytułem w istniejącym `contentColumn`.
- Mały rozmiar zgodny z typografią widgetu (`Style.space()`), kompaktowe tło i
  zaokrąglenie. Użyć istniejącego niebieskiego akcentu `#7aa2f7` jako delikatnego
  tła, a `Color.foreground` jako tekstu dla kontrastu w ciemnym i jasnym motywie;
  nie używać koloru statusu karty ani nie zmieniać paska statusu.
- Tekst badge’a niesie informację niezależnie od koloru; bez animacji, kliknięcia,
  licznika ani dodatkowej akcji.
- Zachować kontrast tekstu co najmniej 4.5:1.
- Nie zmieniać treści karty w edytorze ani zachowania drag-and-drop.

### Formuła ważności

`ageSeconds = floor(nowMs / 1000) - task.created_at`

Badge widoczny, gdy `task.todoist_id` istnieje i
`0 <= ageSeconds < 24 * 60 * 60`.

`nowMs` odświeża się co 60 sekund tylko wtedy, gdy co najmniej jedna karta ma
badge. Przesunięcie czasu systemowego wstecz nie pokazuje badge’a dla daty
utworzenia leżącej w przyszłości.

## 8. Przepływ danych

1. Core zwraca istniejące pola `todoist_id` i `created_at` w stanie tablicy.
2. Widget wylicza wiek karty i widoczność `[NEW]`; nie zapisuje zmian.
3. Po 24 godzinach kolejny tick zegara ukrywa badge.
4. Kolejne cykle sync nie zmieniają czasu utworzenia karty, więc nie odświeżają
   badge’a.

## 9. Wymagania niefunkcjonalne

- Bez zmian SQLite, HTTP API, Rust, CLI, importu ani synchronizacji Todoist.
- Bez nowych zależności.
- Zegar pracuje tylko, gdy istnieje świeża karta, i nie wykonuje żądań sieciowych.
- Badge nie przesłania tytułu/notatki i nie zmienia statusu ani kolejności kart.

## 10. Strategia testowania i przypadki testowe

1. Karta Todoist utworzona przed 24 h → badge `[NEW]` widoczny.
2. Karta Todoist utworzona dokładnie 24 h temu → badge ukryty.
3. Karta Todoist utworzona w przyszłości → badge ukryty.
4. Karta lokalna (`todoist_id = null`) utworzona przed chwilą → badge ukryty.
5. Przeniesienie świeżej karty do In Progress lub DONE nie ukrywa badge’a.
6. Upływ 24 h i następny minutowy tick ukrywa badge bez odświeżania Core.
7. Tytuł/notatka, paska statusu i drag-and-drop zachowują dotychczasowe działanie.
8. `python3 tests/todoist_badge.py` uruchamia dokładne ciało QML helpera w Node i
   sprawdza granice; `qmllint` (jeśli jest zainstalowany) oraz wizualna kontrola
   widgetu po instalacji sprawdzają składnię i render (test Node nie zastępuje
   testu runtime/layoutu QML).

## 11. Decyzje użytkownika

1. Użyć małego badge’a w UI, bez zmiany tytułu ani notatki.
2. Badge ma wygasnąć 24 godziny od pojawienia się.
3. Badge dotyczy kart zaimportowanych z Todoist; jego czas liczymy od utworzenia
   karty. Widoczność obejmuje wszystkie kolumny, a nie tylko TODO.

## 12. Otwarte pytania i decyzje

- Brak otwartych pytań.

## Konsultacja wstępna Rady

Wstępne rekomendacje UX/UI Rady uwzględniono w §7 i §10: badge ma być tekstowy,
nieinteraktywny, krótki, używać istniejącego akcentu i nie modyfikować danych.
Ponowna recenzja draftu nie powiodła się z powodu limitu kredytów dostępnych dla
podagentów; implementacja czeka na akceptację specyfikacji przez użytkownika.
