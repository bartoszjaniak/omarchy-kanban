---
name: kanban-triage
description: "Use when starting work on a new project, a new feature, or any sizable piece of work, to decide whether it belongs on the personal kanban board and which column it should start in. Also use when asked to add, move, or update a task on the board, or when the user says 'dodaj do kanbana', 'co robimy', 'board', 'TODO'. Owns the board CLI."
---

# Kanban triage

The personal kanban board (`TODO` / `In Progress` / `DONE`) is the single source of
truth for what the user and the AI are currently working on. Keep it accurate and
small. A board with noise is worse than an empty one.

The board lives in a local Core. Drive it only through the CLI:

```sh
omarchy-kanban task list              # aktualny stan (3 kolumny + licznik archiwum)
omarchy-kanban task list --all        # razem z ukrytym archiwum
omarchy-kanban task list --json       # dla parsowania
omarchy-kanban task add "Tytuł" --status todo
omarchy-kanban task add "Tytuł" --status doing --notes "kontekst"
omarchy-kanban task move <id> doing
omarchy-kanban task edit <id> --title "Nowy tytuł" --notes "…"
omarchy-kanban task rm <id>
```

`<id>` może być prefiksem UUID (pierwsze 4–8 znaków z `task list`).

## Kiedy rozpoczynasz nową pracę

1. Uruchom `omarchy-kanban task list` — sprawdź, czy zadanie już istnieje.
2. Zdecyduj, czy warto je dodać. Dodaj zadanie **tylko** gdy spełnia oba warunki:
   - to realny, wieloetapowy kawałek pracy (feature, projekt, migracja, naprawa
     wymagająca kilku kroków), nie pojedyncze polecenie ani szybkie pytanie;
   - jego stan ma znaczenie dłużej niż jedną turę — użytkownik chciałby zobaczyć
     je na tablicy, gdy wróci później.
   Nie dodawaj: pytań, jednorazowych komend, drobnych poprawek jednej linii,
   pracy, którą kończysz w tej samej turze.
3. Wybierz status startowy:
   - **todo** — pracę rozpoznajemy, ale jej jeszcze nie zaczynamy w tej turze.
   - **doing** — zaczynamy ją teraz; to aktualny fokus tej sesji.
   - **done** — używaj sporadycznie, tylko gdy praca była już ukończona w tej
     sesji przed dodaniem (np. naprawione przy okazji). Normalnie prowadź przez
     `doing` → `done`.
4. Jeśli dodałeś zadanie, powiedz o tym w jednej linii: id, kolumnę i tytuł.

## W trakcie pracy

- Gdy faktycznie zaczynasz zadanie: `task move <id> doing`.
- Gdy je kończysz: `task move <id> done`.
- Tytuł: krótki, rozkazujący, jeden konkretny rezultat. Nie scalaj kilku rzeczy
  w jedno zadanie i nie duplikuj istniejących.

## Retencja

`DONE` po 48 godzinach automatycznie przechodzi do statusu `archived` — niewidocznego
na tablicy. Archiwum nie trzeba czyścić ręcznie. Nie ustawiaj `archived` sam; to
 dzieje się automatycznie.

## Zasady

- Nie edytuj bazy ani plików danych bezpośrednio. Tylko CLI.
- Nie dodawaj zadania bez powodu „na wszelki wypadek". Cisza na tablicy jest OK.
- Gdy użytkownik mówi „nie dodawaj do tablicy", uszanuj to dla danej pracy.
