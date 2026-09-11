# FerroEdit — Technical Specification

## 1. Мета проєкту

**Назва продукту:** FerroEdit  
**CLI-команда:** `ferroedit`


Розробити сучасний кросплатформний консольний текстовий редактор на Rust, орієнтований на користувачів, які не хочуть вивчати Vim/Neovim або modal editing.

Редактор має працювати як звичайний desktop-style editor всередині terminal:

- меню у верхній частині;
- робота мишкою;
- вкладки відкритих файлів;
- файлове дерево проєкту;
- syntax highlighting;
- search/replace;
- Git sidebar/panel;
- підтримка стандартних клавіатурних shortcut'ів;
- один standalone executable після компіляції.

За UX редактор має бути ближчим до:

- Microsoft Edit;
- Sublime Text;
- VS Code у спрощеному вигляді;

але працювати повністю всередині terminal.

Проєкт не повинен намагатися стати повноцінною IDE у першій версії.

---

## 2. Основні принципи

### 2.1 Простота

Користувач повинен мати змогу запустити:

```bash
ferroedit .
```

і одразу отримати зрозумілий UI без необхідності знати спеціальні команди.

Назва продукту та CLI-команди: `ferroedit`.

### 2.2 Non-modal editing

Не використовувати Vim-style режими.

Typing має одразу вводити текст.

Навігація повинна працювати стандартними клавішами:

```text
Arrow keys
Home
End
PageUp
PageDown

Ctrl+C
Ctrl+V
Ctrl+X
Ctrl+Z
Ctrl+Y

Ctrl+A
Ctrl+F
Ctrl+H

Ctrl+S
Ctrl+O
Ctrl+W
Ctrl+N
```

На macOS необхідно також підтримувати відповідні Cmd shortcuts там, де terminal може коректно їх передавати.

Не робити підтримку Cmd обов'язковою для MVP, якщо terminal input робить її ненадійною.

---

## 3. Target platforms

Обов'язкові:

- Linux x86_64;
- Linux ARM64;
- macOS Apple Silicon;
- macOS Intel, якщо toolchain дозволяє підтримувати без значного ускладнення.

Windows не є частиною MVP.

---

## 4. Distribution

Результатом збірки повинен бути один executable.

Linux production build бажано робити через musl:

```text
x86_64-unknown-linux-musl
aarch64-unknown-linux-musl
```

Не повинно бути runtime dependencies на:

- Python;
- Node.js;
- JVM;
- dynamic application libraries.

Для Git допустима залежність від встановленого системного:

```bash
git
```

Редактор НЕ повинен включати власну реалізацію Git.

---

## 5. Technology stack

Основна мова:

```text
Rust
```

Рекомендований stack:

```text
ratatui
crossterm
ropey
syntect
ignore
serde
toml
unicode-width
unicode-segmentation
```

За необхідності можна використовувати додаткові crates, але:

- не додавати важкі framework dependencies без потреби;
- не використовувати native C libraries, якщо цього можна уникнути;
- намагатися зберегти можливість static musl build.

Git integration реалізувати через:

```rust
std::process::Command
```

та системний `git`.

Не використовувати `libgit2` у MVP.

---

## 6. Загальна UI структура

Базовий layout:

```text
┌──────────────────────────────────────────────────────────────┐
│ File  Edit  Selection  Search  View  Git  Help              │
├────────────────┬─────────────────────────────────────────────┤
│ Explorer       │ main.rs │ editor.rs │ README.md            │
│                ├─────────────────────────────────────────────┤
│ ▼ src          │                                             │
│   main.rs      │  1  fn main() {                            │
│   editor.rs    │  2      println!("hello");                 │
│   git.rs       │  3  }                                      │
│ ▶ tests        │                                             │
│ Cargo.toml     │                                             │
│ README.md      │                                             │
│                │                                             │
├────────────────┤                                             │
│ Git            │                                             │
│ M main.rs      │                                             │
│ ? new.rs       │                                             │
├────────────────┴─────────────────────────────────────────────┤
│ main.rs  Ln 12, Col 8   UTF-8   Rust   main                 │
└──────────────────────────────────────────────────────────────┘
```

Основні UI zones:

```text
MenuBar
Sidebar
TabBar
EditorView
StatusBar
Dialogs / Popups
```

Sidebar має переключатися між режимами:

```text
Explorer
Git
```

У майбутньому можуть бути додані:

```text
Search
Outline
```

але вони не входять у MVP.

---

## 7. Architecture

Застосунок повинен мати чітке розділення між:

```text
UI
State
Editor core
Filesystem
Git
Syntax highlighting
Input handling
```

Рекомендована структура:

```text
src/
    main.rs

    app.rs

    event/
        mod.rs
        keyboard.rs
        mouse.rs

    ui/
        mod.rs
        layout.rs
        menu.rs
        tabs.rs
        editor.rs
        explorer.rs
        git.rs
        statusbar.rs
        dialog.rs

    editor/
        mod.rs
        document.rs
        cursor.rs
        selection.rs
        history.rs
        search.rs

    filesystem/
        mod.rs
        tree.rs

    git/
        mod.rs
        service.rs
        parser.rs
        models.rs

    syntax/
        mod.rs
        highlighter.rs

    config/
        mod.rs
        settings.rs

    commands/
        mod.rs
```

Claude Code може змінити структуру, якщо запропонує кращу, але boundaries між компонентами необхідно зберегти.

---

## 8. Global application state

Базова модель:

```rust
struct App {
    workspace: Workspace,
    tabs: Vec<Tab>,
    active_tab: Option<usize>,

    sidebar: SidebarState,
    menu: MenuState,

    focus: FocusTarget,

    git: GitState,

    dialog: Option<DialogState>,

    should_quit: bool,
}
```

Не створювати глобальний mutable singleton.

State має змінюватися контрольовано через commands/events.

---

## 9. Event loop

Застосунок має працювати через один основний event loop:

```text
terminal event
      ↓
map to AppEvent
      ↓
handle_event()
      ↓
update App state
      ↓
render()
```

Приклад:

```rust
loop {
    terminal.draw(|frame| ui::render(frame, &app))?;

    match event::read()? {
        ...
    }

    if app.should_quit {
        break;
    }
}
```

Не змішувати application logic безпосередньо з rendering.

UI rendering повинен бути максимально pure.

---

## 10. Workspace

CLI invocation:

```bash
ted
```

відкриває current directory.

```bash
ferroedit .
```

відкриває current directory як workspace.

```bash
ferroedit /path/to/project
```

відкриває directory.

```bash
ferroedit file.rs
```

відкриває конкретний файл.

Workspace model:

```rust
struct Workspace {
    root: PathBuf,
}
```

Workspace root використовується для:

- file explorer;
- Git repository detection;
- relative file paths.

---

## 11. Tabs

Потрібна підтримка декількох відкритих файлів.

Модель приблизно:

```rust
struct Tab {
    document: Document,
    dirty: bool,
}
```

Tab bar:

```text
main.rs | editor.rs * | README.md
```

Позначення:

```text
* = unsaved changes
```

Функції:

- click tab;
- Ctrl+Tab;
- Ctrl+Shift+Tab;
- Ctrl+W;
- middle mouse button для закриття, якщо підтримується;
- confirm dialog при закритті modified file.

При відкритті вже відкритого файлу новий tab не створювати.

---

## 12. Document model

Не використовувати один `String` як основне mutable storage для документа.

Використовувати:

```text
ropey::Rope
```

Модель:

```rust
struct Document {
    path: Option<PathBuf>,
    buffer: Rope,

    cursor: Cursor,
    selection: Option<Selection>,

    dirty: bool,

    history: History,
}
```

---

## 13. Cursor

Cursor має коректно працювати із:

- UTF-8;
- Unicode;
- emoji;
- wide terminal characters;
- tabs;
- multiline text.

Не можна вважати:

```text
byte offset == character == terminal column
```

Це різні поняття.

Потрібно чітко розділяти:

```text
byte index
char index
line index
visual column
terminal column
```

Cursor movement:

```text
Left
Right
Up
Down
Home
End
Ctrl+Left
Ctrl+Right
PageUp
PageDown
```

При Up/Down бажано зберігати preferred visual column.

---

## 14. Selection

Підтримати:

```text
Shift + Arrow
Shift + Home
Shift + End
Shift + PageUp
Shift + PageDown
Ctrl+A
mouse drag
```

Selection може бути лише linear.

Column/block selection не входить у MVP.

---

## 15. Editing operations

Підтримати:

- inserting characters;
- newline;
- backspace;
- delete;
- delete selection;
- delete line (whole line, or every line a selection touches);
- replace selection;
- copy;
- cut;
- paste.

Clipboard integration повинна використовувати максимально portable підхід.

Якщо прямий system clipboard ненадійний у terminal environment, зробити abstraction:

```text
ClipboardProvider
```

і дозволити fallback на terminal clipboard / OSC52.

Не блокувати MVP через clipboard edge cases.

---

## 16. Undo / Redo

Обов'язково:

```text
Ctrl+Z
Ctrl+Y
```

або відповідний альтернативний redo shortcut.

History має працювати на semantic edit operations.

Не потрібно зберігати повну копію документа після кожного символу.

Потрібна структура на кшталт:

```rust
enum EditOperation {
    Insert { ... },
    Delete { ... },
}
```

Бажано coalesce послідовне typing одного слова/фрагмента в одну undo operation.

Для MVP допустима простіша реалізація, але вона не повинна призводити до O(document_size) memory на кожен символ.

---

## 17. File open/save

Потрібно підтримати:

```text
Ctrl+O
Ctrl+S
Save As
```

При збереженні:

- не псувати line endings без потреби;
- підтримувати UTF-8;
- коректно показувати помилки filesystem.

Для MVP не обов'язкова підтримка довільних legacy encodings.

Default encoding:

```text
UTF-8
```

---

## 18. File Explorer

Ліва панель повинна показувати дерево workspace.

Приклад:

```text
▼ src
    app.rs
    main.rs
    editor.rs
▶ tests
  Cargo.toml
  README.md
```

Потрібні:

- expand/collapse;
- mouse click;
- keyboard navigation;
- open file;
- active file highlighting.

Filesystem не потрібно рекурсивно обходити повністю під час запуску.

Використовувати lazy directory loading.

---

## 19. .gitignore support

File explorer повинен за замовчуванням приховувати файли, які ігноруються Git.

> **Змінено (ADR-061).** Насправді explorer за замовчуванням показує **всі** файли —
> і git-ignored, і ті, чиї імена починаються з крапки. Приховати їх можна через
> View → Hidden and Ignored Files. `.git/` не показується ніколи.

Рекомендовано використовувати crate:

```text
ignore
```

Бажано також ігнорувати:

```text
.git/
```

Пізніше можна додати:

```text
show ignored files
show hidden files
```

але це не обов'язково для MVP.

---

## 20. Explorer file operations

Підтримати через context menu або commands:

```text
New File
New Directory
Rename
Delete
```

Delete повинен вимагати confirmation.

Операції повинні показувати filesystem errors у dialog/status notification.

---

## 21. Syntax highlighting

Для першої версії використовувати:

```text
syntect
```

Не використовувати tree-sitter у MVP.

Мінімально підтримати:

```text
Rust
Python
Go
JavaScript
TypeScript
JSX
TSX
JSON
YAML
TOML
Markdown
HTML
CSS
Shell
Dockerfile
```

Syntax визначати за:

- extension;
- filename.

Highlighting має виконуватися тільки для видимих рядків або із кешуванням.

Не перераховувати весь великий файл на кожен keystroke.

---

## 22. Search

Shortcut:

```text
Ctrl+F
```

Показувати search box у нижній або верхній частині editor.

Функції:

```text
next match
previous match
case sensitive toggle
```

Для MVP regex не потрібен.

### Великі файли

До 5000 рядків збіги шукаються по мірі набору. Понад — поле чекає на `Enter`
(ADR-075): пошук проходить кожен рядок, і на 200 000 рядків це ~73 мс на кожне
натиснення клавіші, тобто поле, яке перестає приймати введення.

Поки запит не запущено, збіги не підсвічені, а лічильник праворуч показує
`Enter` кольором попередження — `0/0` там означало б «шукали й не знайшли», що
неправда. Причина пояснюється один раз, повідомленням при відкритті поля:

```text
Large file — press Enter to search
```

`Enter`, Find Next, Find Previous, Replace і Replace All працюють завжди —
обмеження стосується лише автоматичного пошуку між кадрами.

### Індикатор пошуку

Прохід по буферу нарізаний по кадрах: 8 мс роботи за кадр, далі малюється кадр і
прохід продовжується з того рядка, де спинився (ADR-076). Редактор не завмирає —
приймає клавіші, `Esc` закриває поле й обриває прохід.

Поки прохід триває, ті самі вісім клітинок показують спінер і кількість уже
знайденого:

```text
⠹ 1274
```

На файлі, який проходиться за один кадр, спінер не з'являється взагалі — мигання
на один кадр гірше за його відсутність. Крок до збігу, який просив `Enter`,
робиться, коли прохід завершився.

Це єдиний годинник анімації в редакторі: цикл подій перестає блокуватись лише
поки прохід у польоті, і починає знову, щойно той завершився. Простий термінал
ніколи не будиться заради перемальовки.

---

## 23. Replace

Shortcut:

```text
Ctrl+H
```

Потрібно:

```text
Replace
Replace All
```

Regex можна додати пізніше.

---

## 24. Menu Bar

Основне меню:

```text
File
Edit
Selection
Search
View
Git
Help
```

Меню має працювати:

- клавіатурою;
- стрілками;
- Enter;
- Escape;
- мишкою.

Приклад:

```text
File
 ├─ New File
 ├─ Open
 ├─ Save
 ├─ Save As
 ├────────────
 ├─ Close Tab
 └─ Quit
```

---

## 25. Command abstraction

Menu items та keyboard shortcuts не повинні мати окремі незалежні implementations.

Використовувати abstraction:

```rust
enum Command {
    NewFile,
    OpenFile,
    Save,
    SaveAs,
    CloseTab,
    Quit,

    Undo,
    Redo,
    Copy,
    Cut,
    Paste,

    Find,
    Replace,

    GitCommit,
    GitPull,
    GitPush,
}
```

І:

```text
keyboard
menu
mouse/context menu
       ↓
Command
       ↓
execute_command()
```

Це важлива архітектурна вимога.

---

## 26. Focus system

UI повинен мати поняття focus.

Наприклад:

```rust
enum FocusTarget {
    Editor,
    Explorer,
    GitPanel,
    Menu,
    Dialog,
}
```

Keyboard events мають маршрутизуватися відповідно до focus.

---

## 27. Mouse support

Mouse є обов'язковою функціональністю.

Потрібно підтримати:

### Editor

- place cursor;
- drag selection;
- scroll;
- click.

### Tabs

- switch tab;
- close tab, якщо є button;
- middle click за можливості.

### Explorer

- select file;
- open file;
- expand directory;
- collapse directory;
- scroll.

### Menu

- open menu;
- select command.

### Panels

- focus panel.

Resizable panes не обов'язкові для MVP, але architecture не повинна унеможливлювати їх додавання.

---

## 28. Git integration

Git integration реалізувати тільки через installed Git CLI.

Перевірка:

```bash
git --version
```

Repository root:

```bash
git rev-parse --show-toplevel
```

Якщо workspace не Git repository:

```text
Git panel should display:

Not a Git repository
```

Під цим рядком панель показує кнопку, яка створює репозиторій:

```text
[ git init ]
```

Кнопка активується мишкою або `Enter`, коли фокус у Git-панелі. Той самий
пункт є в меню — `Git → Initialize Repository`.

Якщо workspace лежить **усередині** репозиторія, але не є його коренем,
панель показує цей репозиторій (як і сам `git`), а під заголовком — рядок
із його назвою:

```text
 Git — main (73)
 in Projects/
  ? .DS_Store
```

Рядок з'являється тільки в цьому випадку; коли workspace і є коренем
репозиторія, панель не витрачає на нього рядка.

Пункти меню `Git`, для яких у поточній папці немає репозиторія, показуються
сірими: вони лишаються на своїх рядках, але виділення їх пропускає і клік по
них нічого не робить. `Initialize Repository` сіріє навпаки — щойно репозиторій
з'явився. `Refresh` активний завжди: це повторний `rev-parse`, яким editor
знаходить репозиторій, створений у іншому вікні.

---

## 29. Git service

Зробити окремий:

```rust
struct GitService
```

API приблизно:

```rust
impl GitService {
    fn repository_root(...) -> Result<PathBuf>;
    fn status(...) -> Result<Vec<GitFileStatus>>;
    fn current_branch(...) -> Result<String>;
    fn branches(...) -> Result<Vec<GitBranch>>;

    fn stage(...) -> Result<()>;
    fn unstage(...) -> Result<()>;

    fn commit(...) -> Result<()>;

    fn switch_branch(...) -> Result<()>;
    fn create_branch(...) -> Result<()>;

    fn pull(...) -> Result<()>;
    fn push(...) -> Result<()>;

    fn merge(...) -> Result<()>;

    fn diff(...) -> Result<String>;
}
```

Не запускати shell string через:

```bash
sh -c
```

Передавати arguments через `Command::arg`.

Це важливо для безпеки та filenames з пробілами.

---

## 30. Git status

Використовувати machine-readable output:

```bash
git status --porcelain=v2
```

Не парсити human-readable `git status`.

UI:

```text
Git: main

Changes
 M src/main.rs
 M src/editor.rs
 ? src/git.rs
```

Статуси:

```text
modified
added
deleted
renamed
untracked
conflicted
```

---

## 31. Stage / Unstage

Потрібно підтримати:

```text
stage file
unstage file
stage all
unstage all
```

Через Git CLI.

Наприклад:

```text
git add -- path
git restore --staged -- path
```

---

## 32. Commit UI

Git panel повинен дозволяти commit.

Приклад:

```text
┌ Commit ─────────────────────────────────────────┐
│                                                │
│ Add Git sidebar                                │
│                                                │
│                                                │
│                    [Commit] [Cancel]            │
└────────────────────────────────────────────────┘
```

Не створювати власний credential management.

Git повинен використовувати existing user configuration.

---

## 33. Branch UI

Повинен бути branch picker.

Наприклад:

```text
Switch branch

  main
> feature/editor
  feature/git

[Enter] switch
[n] new branch
[Esc] close
```

Git operations:

```text
list branches
switch branch
create branch
```

Delete branch можна відкласти.

---

## 34. Git pull/push

Підтримати:

```text
Pull
Push
```

Операції можуть бути network-bound.

Тому вони НЕ повинні блокувати UI event loop.

Необхідно зробити background worker/thread/task для довгих Git commands.

Поки operation виконується:

```text
Pushing...
```

Після завершення:

```text
Push completed
```

або показати error dialog.

---

## 35. Merge

Підтримати:

```text
Git → Merge Branch
```

Flow:

```text
select branch
       ↓
git merge <branch>
       ↓
success OR conflicts
```

При конфліктах Git panel повинен показати conflicted files.

Не потрібно автоматично вирішувати conflicts.

---

## 36. Git diff

Потрібен basic diff viewer.

При виборі modified file:

```text
View Diff
```

можна викликати:

```bash
git diff -- path
```

Показати результат у read-only internal viewer.

Для MVP syntax-aware side-by-side diff не потрібен.

Unified diff достатній.

---

## 37. Asynchronous operations

Такі операції як:

```text
git pull
git push
git fetch
filesystem scanning
```

не повинні freeze UI.

Рекомендовано:

```text
std::thread
+
channel
```

або інший простий concurrency mechanism.

Не використовувати Tokio лише заради декількох subprocess operations, якщо std threads достатньо.

Якщо Tokio значно спрощує architecture, його використання допускається, але Claude Code повинен пояснити причину.

---

## 38. Status Bar

Bottom status bar:

```text
README.md  Ln 12, Col 8   UTF-8   Markdown   main
```

Може містити:

```text
filename
cursor line
cursor column
encoding
syntax/language
Git branch
dirty state
```

---

## 39. Notifications

Потрібна система short-lived status messages:

```text
Saved main.rs

Push completed

Failed to save file: Permission denied
```

Архітектура:

```rust
struct Notification {
    message: String,
    kind: NotificationKind,
}
```

Для MVP timer-based disappearing не обов'язковий.

---

## 40. Dialog system

Усі modal UI windows повинні використовувати спільну abstraction.

Наприклад:

```rust
enum DialogState {
    Confirm(...),
    Input(...),
    FileOpen(...),
    Commit(...),
    BranchPicker(...),
    Error(...),
}
```

Не створювати окрему event architecture для кожного popup.

---

## 41. Terminal cleanup

Це критично.

При будь-якому нормальному завершенні програма повинна:

- leave alternate screen;
- restore cursor;
- disable mouse capture;
- restore terminal mode.

При panic бажано також відновити terminal.

Не залишати terminal у raw mode після crash.

Встановити appropriate panic hook.

Те саме при завершенні сигналом — `SIGINT`, `SIGTERM`, `SIGHUP`, `SIGQUIT`: ні `Drop`,
ні panic hook там не виконуються, тож потрібен окремий обробник (ADR-073).

Якщо terminal дістався програмі вже зламаним — попередній запуск убили раніше, ніж він
встиг щось відновити — привести його до ладу на старті, до входу в raw mode. Інакше
зламаний стан зберігається як «оригінал» і повертається назад при кожному наступному
нормальному виході.

---

## 42. Configuration

Config file можна підтримати в MVP у мінімальному вигляді.

Приклад:

```toml
tab_width = 4
show_line_numbers = true
mouse = true
syntax_highlighting = true
```

Можливе розташування:

Linux:

```text
~/.config/ferroedit/config.toml
```

macOS:

```text
~/Library/Application Support/ferroedit/config.toml
```

Але якщо cross-platform config path потребує зайвого коду, можна використати відповідний Rust crate.

---

## 43. Themes

Не створювати складний theme engine у першій версії.

Потрібно мати internal theme struct:

```rust
struct Theme {
    ...
}
```

щоб colors не були hardcoded у десятках UI components.

Достатньо одного default dark theme.

---

## 44. Performance requirements

Редактор повинен комфортно працювати з файлами:

```text
1–10 MB
```

і залишатися usable для більших файлів.

Не робити операції O(total document size) на кожен keystroke.

Особливо уникати:

```text
buffer.to_string()
```

на кожен render.

Rendering повинен працювати переважно з visible viewport.

---

## 45. Error handling

Не використовувати:

```rust
unwrap()
expect()
```

у runtime paths без сильної причини.

Допустимо в tests та invariants.

Errors повинні доходити до UI як:

```text
Result<T, AppError>
```

або еквівалент.

Користувач повинен бачити зрозуміле повідомлення.

---

## 46. Logging

Не писати debug output у stdout, тому що stdout використовується TUI.

Logging має йти у файл або бути вимкнене.

Наприклад:

```text
~/.local/state/ferroedit/ferroedit.log
```

або temporary log file.

---

## 47. Testing

Потрібні unit tests для editor core.

Особливо:

```text
cursor movement
insert
delete
selection
undo
redo
Unicode
line navigation
Git parser
file tree
```

UI pixel-perfect tests не є пріоритетом.

Editor core має бути достатньо ізольованим, щоб тестувати його без terminal.

---

## 48. Unicode tests

Обов'язково додати fixtures/tests із:

```text
Hello
Привіт
Україна
日本語
🙂
👨‍👩‍👧
é
```

Перевірити:

- movement;
- delete;
- selection;
- line/column calculation.

---

## 49. CLI

Підтримати:

```bash
ted
ferroedit .
ferroedit file.rs
ferroedit /path/to/project
ferroedit --help
ferroedit --version
```

Додатково можна:

```bash
ferroedit +42 file.rs
```

для відкриття на конкретному рядку, але це не MVP requirement.

---

## 50. Non-goals для MVP

Не реалізовувати:

```text
LSP
autocomplete
IntelliSense
debugger
embedded terminal
plugin system
Tree-sitter
AI
GitHub API
GitLab API
remote SSH editing
collaborative editing
project indexing
symbol search
code refactoring
code formatter integration
DAP
multi-cursor
column selection
minimap
split editor
```

Якщо якась із цих функцій потрібна для внутрішньої architecture — спочатку пояснити чому.

---

## 51. Порядок реалізації

Не намагатися реалізувати весь продукт одним великим commit.

Розробляти по phases.

### Phase 0 — Project bootstrap

Створити:

```text
Cargo project
basic modules
CI
formatting
linting
```

Налаштувати:

```bash
cargo fmt
cargo clippy
cargo test
```

Acceptance:

```text
cargo build
cargo test
cargo clippy
```

проходять.

### Phase 1 — TUI shell

Реалізувати:

```text
alternate screen
raw mode
main event loop

menu bar
sidebar
tab bar
editor area
status bar
```

Поки можна використовувати fake/mock data.

Acceptance criteria:

- app запускається;
- Ctrl+Q завершує;
- terminal restore працює;
- mouse capture працює;
- layout коректно resize'иться.

### Phase 2 — Basic editor

Реалізувати:

```text
Document
Rope
Cursor
viewport
insert
backspace
delete
newline
open
save
```

Acceptance:

```bash
ferroedit test.txt
```

дозволяє редагувати та зберігати файл.

### Phase 3 — Selection and clipboard

Реалізувати:

```text
selection
Shift navigation
Ctrl+A
copy
cut
paste
mouse selection
```

Acceptance:

standard editing behavior без modal mode.

### Phase 4 — Undo/redo

Реалізувати history.

Acceptance:

```text
Ctrl+Z
Ctrl+Y
```

коректно відновлюють зміни.

### Phase 5 — Tabs

Реалізувати:

```text
open multiple files
switch
close
dirty indicator
save confirmation
```

Acceptance:

одночасно відкрито >= 10 файлів без проблем.

### Phase 6 — Explorer

Реалізувати:

```text
file tree
lazy directories
open file
.gitignore
mouse
keyboard
```

Acceptance:

```bash
ferroedit .
```

дозволяє працювати з project tree.

### Phase 7 — Syntax highlighting

Додати syntect.

Acceptance:

Rust/Python/JS/JSON/Markdown файли мають highlighting.

Scrolling і typing не повинні помітно lag'ати.

### Phase 8 — Search / replace

Реалізувати:

```text
Ctrl+F
Ctrl+H
next
previous
replace
replace all
```

### Phase 9 — Menus

Підключити command abstraction.

Menu commands та shortcuts мають використовувати ті самі:

```text
Command
```

objects.

### Phase 10 — Git status

Реалізувати:

```text
repo detection
current branch
status parser
Git sidebar
```

Acceptance:

modified/untracked/staged files правильно відображаються.

### Phase 11 — Git actions

Реалізувати:

```text
stage
unstage
commit
pull
push
```

Network actions не повинні freeze UI.

### Phase 12 — Branches and merge

Реалізувати:

```text
branch picker
switch
new branch
merge
conflict indication
```

### Phase 13 — Diff viewer

Реалізувати unified diff viewer.

### Phase 14 — Polish

Після функціонального MVP:

```text
better shortcuts
help screen
better errors
responsive layout
theme cleanup
performance profiling
Unicode bug fixes
```

---

## 52. Development workflow для Claude Code

Claude Code повинен працювати ітеративно.

Перед початком кожної великої phase:

1. Переглянути поточну architecture.
2. Запропонувати короткий implementation plan.
3. Визначити modules/files, які будуть змінені.
4. Реалізувати phase.
5. Запустити:

```bash
cargo fmt
cargo test
cargo clippy -- -D warnings
```

6. Виправити всі errors/warnings.
7. Перевірити manual scenario, якщо можливо.
8. Оновити progress document.

---

## 53. Не генерувати великий monolith

Не створювати:

```text
main.rs на 5000 рядків
```

і не складати всю application logic у:

```text
app.rs
```

Використовувати невеликі cohesive modules.

Водночас не потрібно створювати abstraction заради abstraction.

---

## 54. Code quality

Перевага:

```text
simple
explicit
testable
idiomatic Rust
```

над:

```text
clever
generic
framework-like
over-engineered
```

Не потрібно будувати framework для editor plugins, якщо plugins не потрібні.

---

## 55. Comments

Коментарі мають пояснювати:

```text
why
```

а не очевидне:

```text
what
```

Особливо документувати:

- Unicode indexing assumptions;
- Rope coordinate systems;
- Git status parsing;
- terminal cleanup;
- async worker communication.

---

## 56. Documentation

Створити:

```text
README.md
docs/ARCHITECTURE.md
docs/SHORTCUTS.md
docs/ROADMAP.md
```

README повинен містити:

```text
screenshot або terminal mockup
installation
build
usage
features
keyboard shortcuts
```

---

## 57. Progress tracking

Створити:

```text
docs/PROGRESS.md
```

Формат:

```markdown
# Current Phase

Phase 4 — Undo/Redo

## Completed

- Rope editor
- cursor
- selection

## In progress

- undo history

## Known issues

- emoji selection width
- OSC52 clipboard not implemented

## Next

- redo
- history coalescing
```

Claude Code має оновлювати цей документ після кожної phase.

---

## 58. Architecture decisions

Створити:

```text
docs/DECISIONS.md
```

Важливі рішення записувати коротко.

Приклад:

```markdown
## ADR-001: Git CLI instead of libgit2

We use the system Git CLI because:

- credentials continue to work;
- SSH config works;
- signing works;
- fewer native dependencies;
- easier static builds.
```

---

## 59. Git workflow самого проєкту

Робити невеликі commits.

Приклади:

```text
feat: add terminal event loop

feat: implement rope-backed document

feat: add cursor movement

feat: add file explorer

feat: add git status parser

fix: handle wide unicode cursor movement
```

Не робити один giant commit на весь MVP.

---

## 60. Acceptance criteria MVP

MVP вважається готовим, коли можна виконати:

```bash
ferroedit ~/Projects/example
```

і користувач може:

1. побачити файлове дерево;
2. клікнути файл;
3. відкрити його у вкладці;
4. редагувати текст;
5. користуватися мишкою;
6. використовувати стандартні shortcuts;
7. бачити syntax highlighting;
8. відкрити декілька tabs;
9. знайти та замінити текст;
10. зберегти файл;
11. побачити Git changes;
12. stage/unstage файл;
13. написати commit message;
14. зробити commit;
15. pull;
16. push;
17. переключити Git branch;
18. створити branch;
19. merge branch;
20. переглянути diff.

Після виходу terminal повинен бути повністю відновлений.

---

## 61. UX requirement

Головний UX принцип:

> Користувач, який знає Sublime Text, VS Code або звичайний desktop editor, але ніколи не користувався Vim, має зрозуміти базову роботу редактора без документації.

Тому:

- стандартні shortcuts;
- видиме меню;
- mouse-first UX;
- зрозумілі dialogs;
- мінімум прихованих commands;
- ніяких modal editing states.

---

## 62. Future roadmap

Після MVP architecture повинна дозволяти потенційно додати:

```text
LSP
Tree-sitter
file search
command palette
split editor
integrated terminal
Git history/log
blame
Git stash
formatters
external commands
themes
sessions/workspaces
```

Але не реалізовувати їх зараз.

---

## 63. Перша задача для Claude Code

Після прочитання цього документа НЕ починай одразу реалізовувати всі функції.

Спочатку:

1. Проаналізуй specification.
2. Запропонуй final project architecture.
3. Запропонуй dependency list і поясни призначення кожного crate.
4. Познач потенційні technical risks:
   - Unicode;
   - mouse mapping;
   - clipboard;
   - syntax highlighting performance;
   - static Linux build;
   - asynchronous Git commands.
5. Створи початковий project skeleton.
6. Створи:

```text
README.md
docs/ARCHITECTURE.md
docs/ROADMAP.md
docs/PROGRESS.md
docs/DECISIONS.md
```

7. Реалізуй тільки Phase 0 та Phase 1.
8. Запусти:

```bash
cargo fmt
cargo test
cargo clippy -- -D warnings
```

9. Не переходь до Phase 2, доки Phase 1 не є clean і architecture не виглядає придатною для подальшого розвитку.

---

## 64. Кінцева продуктова ідея

Продукт не повинен конкурувати з Neovim у складності чи extensibility.

Його позиціонування:

```text
A modern desktop-like text editor that happens to run in a terminal.
```

або простіше:

```text
Sublime Text for the terminal.
```

Ключові відмінності:

```text
mouse
menus
tabs
file explorer
Git
non-modal editing
single binary
SSH-friendly
```

Ці принципи мають пріоритет над додаванням максимальної кількості функцій.

---

## 58. Word wrap, horizontal scrolling, Go to Line

Довгий рядок треба вміти прочитати двома способами, і користувач обирає який.

**Word wrap** — перемикач (`Alt+Z`, меню *View → Word Wrap*), який запам'ятовується
у config-файлі разом із темою:

```text
off (default) — рядок є одним drawn row і виходить за правий край
on            — рядок ламається на кілька drawn rows по ширині pane
```

Перенос — по межі слова, з hard break всередині слова, довшого за pane. Колонки
рядка при цьому не переобчислюються: другий row починається з тієї visual column,
на якій обірвався перший, тож tab зберігає свій tab stop, а renderer малює row
тим самим вікном, яким малює горизонтально прокручений рядок.

Коли wrap увімкнено, вертикальна навігація рахує drawn rows, а не lines:

```text
Up / Down          — сусідній drawn row
PageUp / PageDown  — сторінка drawn rows
Home / End         — початок і кінець drawn row
```

Preferred visual column (§13) зберігається як зміщення всередині row.

**Horizontal scrolling** — коли wrap вимкнено, pane можна рухати вбік, не рухаючи
cursor: `Alt+Left` / `Alt+Right`, меню *View → Scroll Left/Right*, `Shift`+колесо, і
горизонтальне колесо (`ScrollLeft` / `ScrollRight`) там, де термінал його шле. Пункти
меню обов'язкові: `Alt` доходить не в кожному терміналі (ADR-008), і команда, доступна
лише через нього, — це команда, якої частина користувачів не має. Крок — 8 колонок; вікно
зупиняється на найширшому рядку, який зараз на екрані. При увімкненому wrap
горизонтальної осі немає і всі ці жести нічого не роблять.

**Go to Line** — `Ctrl+G`, меню *Search → Go to Line…*: input dialog з номером
рядка, на якому стоїть cursor, і кількістю рядків у prompt. Номер — one-based, як
у CLI-аргументі `+42` (§49); номер за межами файлу означає останній рядок.

---

## 65. CSV: перегляд файлу як таблиці

`.csv` і `.tsv` — це текстові файли, які майже завжди читають не як текст. Тому
таб із таким файлом відкривається одразу як таблиця:

```text
перший рядок файлу — заголовок колонок
кожен наступний   — один запис
ліворуч           — номер запису, як gutter із номерами рядків
```

Це друге *прочитання* того самого буфера, а не друга копія файлу: undo history,
`Ctrl+S`, encoding, line endings і watcher залишаються ті самі, що й у текстовому
режимі. Таблиця перебудовується з буфера щоразу, коли той змінюється.

**Table View** — перемикач (`F4`, меню *View → Table View*): таблиця ↔ текст.
Будь-який файл можна відкрити як таблицю, не лише `.csv`.

**Навігація** — ті самі клавіші, що й у тексті:

```text
Up / Down          — сусідній запис; вище першого запису — рядок заголовка
Left / Right       — сусідня колонка
PageUp / PageDown  — сторінка записів
Home / End         — перша і остання колонка запису
Ctrl+Home / Ctrl+End — початок і кінець таблиці
click              — вибрати комірку, заголовок теж
wheel              — прокрутити записи; Shift+wheel — колонки
```

**Виділення** — прямокутник комірок, як у будь-якій сітці. `Shift` із будь-якою
клавішею руху розтягує його від тієї комірки, де він почався; звичайний рух його
скидає, як і в тексті.

```text
Shift + рух         — розтягнути виділення на сусідні комірки
drag                — те саме мишею
click по номеру     — весь запис
click по заголовку  — вся колонка
Ctrl+A              — вся таблиця, разом із заголовком
меню Selection      — Select Row, Select Column
```

Вибравши колонку, курсор лишається на її назві: блок готовий до `Ctrl+C`, а `F2` тут же
перейменовує колонку.

Виділене показує status bar: `Sel 3×2` — три записи на дві колонки. `Ctrl+C` копіює
блок у діалекті самого файлу (значення, розділені роздільником, записи — рядками), тож
скопійоване з таблиці вставляється назад у таблицю, у текст або в іншу програму тим
самим, чим було. Одна комірка копіюється просто своїм значенням — тим, що видно на
екрані. `Ctrl+X` те саме, і очищає блок одним кроком undo; `Delete` очищає без копіювання.

**Редагування комірки.** Таблиця пише в той самий буфер, що й текст: правка комірки —
це звичайна правка документа. Вона стає одним кроком undo history, робить таб
*modified* і зберігається тим самим `Ctrl+S` (ADR-063).

```text
F2 / Enter          — відкрити комірку з тим значенням, що в ній є
друкований символ   — відкрити комірку і почати значення заново
Enter               — записати і стати на запис нижче
Tab                 — записати і перейти в наступну колонку
Esc                 — скасувати; у файлі не змінюється нічого
Delete / Backspace  — очистити комірку, коли її не відкрито
Ctrl+C / Ctrl+X     — копіювати значення комірки; вирізати — копіювати і очистити
Ctrl+V              — вставити значення в комірку
Insert              — додати порожній запис під поточним
Ctrl+D              — видалити поточний запис
```

Рядок заголовка — такий самий запис файлу, як решта: на нього стає курсор, і саме там
перейменовують колонку. Клік по іншій комірці записує ту, яку набирали, — так само, як
у будь-якій сітці.

Записуючи значення, редактор бере лапки лише там, де без них читання змінилося б:
значення містить роздільник, символ лапок або перенос рядка; лапка всередині
подвоюється (RFC 4180). Решта запису лишається байт у байт такою, як була — правка
однієї комірки дає один рядок diff'у, а не переписаний файл. Колонку, до якої запис не
дотягується, дописують тими роздільниками, яких бракує.

Три речі таблиця робити відмовляється — і каже, чому:

```text
значення з переносом рядка    — його редагують у тексті (F4)
роздільник у файлі без лапок  — у такому файлі його нікуди подіти
вставка тексту з переносами   — рядки вставленого є записами, а не однією коміркою
```

*Replace* і *Select All* теж лишаються текстовими: вони діють на діапазон тексту, якого
в сітці не видно.

Вікно рухається цілими колонками: заголовок має стояти рівно над своїми значеннями.
Комірка, ширша за колонку, обрізається з `…` — повне значення видно в текстовому
режимі. Перенос рядка всередині quoted-поля малюється як `⏎`: запис — це один рядок
сітки.

**Delimiter і quote** — у status bar, поруч із encoding і line endings, і клікаються
так само (§38, ADR-058):

```text
Row 3/128 · country   Delim ,   Quote "   UTF-8   LF   main   Editor
```

`Delim` відкриває вибір роздільника (кома, крапка з комою, tab, `|`, двокрапка,
пробіл), `Quote` — символу лапок (`"`, `'`, `` ` ``, або без лапок узагалі). Ті самі
два пункти є в меню *View*. Вибір діє одразу: на диску нічого не змінюється, змінюється
лише те, як ті самі байти діляться на колонки.

При відкритті роздільник вгадується: `.tsv` — це завжди tab, для решти береться той
кандидат (`,` `;` tab `|`), який ділить перші рядки файлу на однакову кількість колонок.
Пробіл і двокрапка ніколи не вгадуються — звичайний текст і час ділилися б ними рівно, —
але їх можна обрати вручну.

**Межі.** Таблиця тримає максимум 100 000 записів; довший файл читається в текстовому
режимі, який стрімиться з rope і межі не має. Колонка малюється завширшки від 3 до 32
клітинок.

---

## 66. Перевірка оновлень

FerroEdit ставиться одним бінарником через `install.sh` (ADR-060). Пакетного
менеджера, який сказав би про нову версію, тут немає — тому редактор питає сам.

При старті, якщо перевірка ввімкнена і від попередньої минуло більше доби,
редактор у фоновому потоці питає GitHub про останній реліз. Відповідь приходить
у той самий канал подій, що й ввід і git-worker: UI не блокується і нічого не
чекає.

```text
Help → Check for Updates   — спитати зараз
Help → Check on Start      — вимкнути/увімкнути автоматичну перевірку
```

**Що показується.** Перевірка, яку користувач запустив сам, відповідає завжди:

```text
є новіша версія   — діалог: версія в заголовку, адреса релізів у тілі
версія найновіша  — `FerroEdit 0.1.3 is the latest release` у status bar
не вдалося        — `Could not check for updates: …` (error, §39)
```

Автоматична перевірка каже лише те, що є новина: рядок у status bar із версією —
`FerroEdit 0.2.0 is out — re-run the install script`. Адреси там немає навмисне:
status bar першим віддає колонки вузькому терміналу (§39, ADR-039), а обрізаний
URL гірший за жоден. Модального вікна вона не відкриває — повідомлення без питання
не має права з'їдати натиснуту клавішу. Невдала фонова перевірка йде тільки в лог.

**Налаштування** — у тому самому `~/.config/ferroedit/config.json` (§43):

```json
{
  "check-for-updates": true,
  "last-update-check": 1757376000
}
```

`check-for-updates` типово `true`; `last-update-check` — коли остання перевірка
відповіла, у секундах від епохи, щоб автоматична була не частіше ніж раз на добу.
Перевірка з меню працює незалежно від обох.

**Мережа.** HTTP-клієнта в бінарнику немає: запит робить `curl` або `wget` — ті
самі, яких `install.sh` уже вимагає від машини (ADR-003, ADR-065). Запит іде на
`github.com/korkholeh/ferroedit/releases/latest` і читається з редіректу; нічого,
крім звичайних заголовків HTTP-клієнта, назовні не йде.

**Оновлення себе редактор не робить.** Він показує версію й адресу — далі це
знову `install.sh`.

---

## 67. Історія комітів

Перегляд історії — окрема вкладка, поруч із вкладками diff (ADR-068). Три
історії, одна панель: різниця тільки в аргументах `git log`.

```text
Git → Log            (`l` у git-панелі)  — історія репозиторію
Git → File History   (`h` у git-панелі)  — історія відкритого файлу
Git → Line History                       — історія виділених рядків
```

**Що в рядку.** Скорочений хеш, дата (`--date=short`), автор, subject — колонками,
ширина яких рахується від найширшого значення на екрані. Це список, а не пейджер:
стрілки рухають виділення, `Enter` або `d` відкриває обраний коміт як diff.

```bash
git log --no-color --no-ext-diff --date=short \
        --format=%H%x1f%h%x1f%an%x1f%ad%x1f%s -z --max-count=2000
# файл:  --follow -- <path>
# рядки: -L <first>,<last>:<path> --no-patch
```

Ліміт — 2000 комітів; заголовок каже `(cut)`, коли список обірвав саме він
(ADR-027, як `MAX_LINES` у diff і `MAX_ENTRIES` у статусі).

**Пошук.** `/` відкриває поле над списком. Воно має дві швидкості:

```text
набір тексту — звужує вже прочитані коміти (без підпроцесу, миттєво):
               subject, автор, дата, хеш
Enter        — віддає текст git: --fixed-strings --regexp-ignore-case --grep=…
               шукає по всьому повідомленню і по всій історії, за межами ліміту
```

Заголовок каже, що саме на екрані: `— 2 commits of 56` для звуженого списку,
`matching "fix"` для відповіді git. `Esc` закриває поле разом зі звуженням.

**Коміт як diff** (ADR-069). `git show -m --first-parent <oid>` — з заголовком і
повним повідомленням над патчем. Історія файлу звужує показ до цього файлу
(`-- <path>`). У коміта одна сторона: `s` каже про це, а не спорожняє панель, і
жодна git-операція його не перечитує — написаний об'єкт не змінюється.

Читання історії — на UI-потоці, як статус, список гілок і diff (ADR-030): це
локальне читання з лімітом, яке закінчується за мілісекунди.

---

## 68. Конфіг репозиторію

`Git → Config` (`g` у git-панелі) відкриває `.git/config` звичайною вкладкою
текстового редактора (ADR-070). Ніяких діалогів: файл читається, редагується й
зберігається тим самим кодом, що й будь-який інший файл.

Шлях береться з того, що відповів `git rev-parse --absolute-git-dir`, а не
збирається як `<root>/.git`: у worktree чи submodule останнє — файл-вказівник,
а конфіг, який читає git у цьому дереві, — той, який git назвав.

---

## 69. Легенда клавіш і меню

Меню не показує коротких клавіш, які є звичайними літерами без модифікатора —
`a`, `u`, `c`, `b`, `m`, `d`, `h`, `l`, `g`, `/` (ADR-071). Клавіші працюють, як
і працювали, але тільки поки фокус у своїй панелі, а меню читають із кареткою в
документі, де та сама літера просто друкується.

Натомість панель, якій ці клавіші належать, каже про них сама: перегляд списку
комітів малює легенду на нижній рамці, поруч із позицією.

```text
↑↓ move  ·  Enter diff  ·  /  search  ·  F5 refresh  ·  Esc close
↑↓ move  ·  Enter search git  ·  Esc cancel        (коли відкрите поле пошуку)
```

Легенда читає підписи з тієї ж таблиці клавіш, що й меню і `docs/SHORTCUTS.md`
(§25, ADR-028), і скорочується з кінця, коли не влазить. Повний перелік — у `F1`
і в `docs/SHORTCUTS.md`, таблицею на кожен фокус.

---

## 70. Стиснуті файли

Файл, який починається з `1F 8B` (gzip), відкривається розпакованим (ADR-074).
`dump.sql.gz` показує SQL, підсвічений як SQL — граматика береться з імені
всередині контейнера, а не з `.gz`.

Розпакування йде **перед** визначенням кодування: контейнер нічого не каже про
те, що всередині, тож charset визначається за розпакованими байтами (§17,
ADR-059), а перевірка на NUL — за розшифрованим текстом. Тому `.tar.gz`
розпакується й усе одно буде відхилений як бінарний.

Такий буфер — **лише для читання**. Редактор розпаковує й ніколи не пакує:
записати назад означало б вибрати за користувача рівень стиснення, ім'я та час
у заголовку, і один випадковий `Ctrl+S` переписав би весь бекап іншим файлом.

```text
відхиляється: набір тексту, Backspace, Delete, Delete Line, Cut, Paste,
              Replace, Save
працює:       рух, виділення, Copy, Find, перенос рядків, підсвітка,
              Reopen with Encoding, F5, Undo/Redo
```

Вихід — `Save As`: текст записується як звичайний файл, і вкладка перестає бути
переглядом стиснутого.

Розпакування обмежене 64 MB. Довший потік обрізається по останньому переносу
рядка; про це кажуть двічі — повідомленням при відкритті й постійно у статус-барі:

```text
gzip · read-only · cut
```

Обрізаний буфер не можна зберегти через `Save As`: на диску з'явився б файл, який
виглядає цілим і не є ним.

## 71. Перегляд зображень

Файл із розширенням `.png`, `.jpg` або `.jpeg` відкривається у **перегляді
зображень** — окремій вкладці, як diff і як історія комітів (ADR-078). Це не
документ: у ньому немає курсора, немає що зберігати, і `App::active` відповідає
`None`, тож жодна команда редагування до нього не дістанеться.

Формат визначається **першими байтами**, а не розширенням: `.png`, усередині
якого JPEG, відкриється як JPEG. Розширення лише вирішує, *який* переглядач
відкрити, бо відповісти треба до того, як файл прочитано.

Великий файл читається так само по шматках, як і великий текст (ADR-077): у
центрі панелі той самий бокс зі спінером і відсотками, а вкладка з'являється,
коли байти на місці.

### Як малюється

Кожна комірка терміналу — це символ `▀` (верхній півблок): колір тексту — один
рядок пікселів, колір фону — наступний. Отже одна комірка несе **два пікселі**, і
масштаб `100%` означає один піксель зображення на півкомірку — приблизно
квадрат, бо комірка терміналу вдвічі вища за ширину.

Тема редактора навмисно індексована (ADR-007), і зображення — єдине місце, де це
правило не діє: фотографія у шістнадцяти кольорах не є фотографією. Тож
`COLORTERM=truecolor` дає RGB, а без нього піксель квантується у палітру
xterm-256 — куб 6×6×6 плюс сіра шкала на 24 сходинки.

Зменшення **усереднює** пікселі, які покриває півкомірка, збільшення бере
**найближчий** — на 800% користувач дивиться саме на пікселі.

Прозорість композитується на шахівницю 8×8 ще на етапі декодування, тож
`has_alpha` у метаданих є поясненням того, що видно.

### Колонка метаданих

Ліворуч, 26 колонок, вмикається `m`. Зникає сама, коли на панелі лишилося б
менше 24 колонок під саме зображення: переглядач, який віддав мітках більше
місця, ніж фотографії, показує не те.

```text
File      photo.jpg      Colour    YCbCr 8-bit
Size      2.9 MB         Alpha     no
                         Bytes/px  0.24
Format    JPEG
Width     4032 px        Zoom      Fit 3.6%
Height    3024 px        Origin    0, 0
Pixels    12.2 MP        Showing   4032 × 3024 px
Aspect    4:3
```

### Вікно над зображенням

Масштаб — фіксовані сходинки від 1/16 до 16×, щоб `+` і `-` поверталися рівно
туди, звідки почали. `0` — вписати ціле (і тримати вписаним при зміні розміру
терміналу), `1` — 100%. Зум тримає нерухомим центр панелі.

Стрілки рухають вікно на восьму частину видимого — однаково далеко і на 6%, і на
800%. `PageUp`/`PageDown` — на цілу панель, `Home` — у центр. Вікно ніколи не
виходить за межі зображення; вісь, коротша за панель, центрується.

Мишею: колесо — панорамування, `Shift`+колесо — вбік, `Ctrl`+колесо — зум,
перетягування рухає зображення під курсором.

`F5` перечитує файл, **зберігаючи** масштаб і позицію: його натискають тоді, коли
інша програма переекспортовує файл, і дивляться на той самий куток.

### Межі

Декодуються PNG (усі глибини й палітри, 16 біт зрізається до 8) і JPEG
(YCbCr, сірий, Adobe CMYK). Понад 64 мегапікселі — відмова з поясненням, до
будь-яких виділень пам'яті. EXIF-орієнтація не читається: зображення показується
так, як лежать пікселі.

Зберегти зображення не можна нічим — ні `Ctrl+S`, ні `Save As`. Редактор його
лише показує.
