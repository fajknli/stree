# stree Architecture & Kernel Design

> This document records the core design decisions, architectural constraints, and the engineering reasoning behind every critical technical choice in the `stree` engine. Intended for source code readers, contributors, and anyone interested in the underlying mechanics of TUI engines.

---

## 1. Paradigm Declaration: Why Not Another TUI Framework

### 1.1 Rejecting Retained Mode

Mainstream TUI frameworks (Ratatui, Bubble Tea, tview) adopt **Retained Mode**: developers build a component tree in code, the framework holds state, and every frame is diffed and redrawn. This pattern is suitable for building "applications," but not for building a "foundation."

`stree`'s choice is **Immediate Mode + External State**:
*   The engine holds no business state.
*   Every frame, the engine projects the screen from the in-memory Dataset via pure functions.
*   State evolution is driven by external scripts through stdin/IPC.

This means `stree` is essentially a **renderer**, not an **application framework**. Its responsibility boundary is extremely clear: receive data, draw the interface, forward intents.

### 1.2 Why Pure Rust Hand-Written, No TUI Library Dependencies

`stree`'s rendering layer is built directly on `crossterm` primitives (`MoveTo`, `Print`, `SetForegroundColor`), without using any high-level TUI component library. There are three reasons:

1.  **Pixel-Level Control**: TUI layout precision requirements are extremely high. A single character overflow can cause terminal line wrapping, which collapses the layout. Using a high-level framework means giving up absolute control over every column and every row.
2.  **Zero Abstraction Cost**: `stree`'s render loop is single-threaded and synchronous, with no async runtime overhead. The rendering path for every frame is deterministic and predictable.
3.  **ANSI Pass-Through**: The engine needs to perfectly pass through ANSI escape sequences from external command output while accurately calculating visible character width. This requires the renderer to understand the internal structure of ANSI, rather than treating it as a black-box string.

---

## 2. Protocol-Driven: Extreme Unix Pipeline Philosophy

### 2.1 Design Motivation for the 4-Column TSV Protocol

Why 4 columns? Why TSV?

*   **TSV (Tab-Separated Values)**: Tab characters almost never appear in natural text, so no escaping mechanism is needed. Compared to CSV, TSV parsing cost is an order of magnitude lower, and it perfectly integrates with Unix standard tools like `awk`, `cut`, and `sort`.
*   **Minimal Completeness of 4 Columns**:
    *   Column 1 (ID): The primary key required by graph theory.
    *   Column 2 (Display): Human-readable display text.
    *   Column 3 (Path): Machine-operable business payload.
    *   Column 4 (Tags): Unified carrier for multi-dimensional classification and style mapping.

Fewer than 4 columns cannot express complete tree interaction semantics; more than 4 columns means the engine begins to understand business semantics, violating the "zero business intrusion" principle.

### 2.2 Orthogonal Separation of Display and Path

This is the most critical orthogonal design decision in the protocol.

*   `Display` is for humans. It can contain formatting like `[tag]`, `(type)`, or even ANSI color codes. The engine only renders, never parses.
*   `Path` is for machines. It's the argument for external commands, the `{path}` in `vim {path}`. The engine never opens, reads, or interprets it.

This separation ensures the business layer can freely compose display text (e.g., `title " (" type ")"`) without affecting command execution accuracy.

### 2.3 Evolution of the Fourth Column: From Status to Tags

In early designs, the fourth column was named `status` and was a single value (e.g., `live` or `archived`).

The problem quickly emerged: the business layer needed to express simultaneously "this is a note-type file" and "this file has been archived." With only one field, the business layer either had to concatenate into `archived_note` (causing style rule explosion) or be forced to add a fifth column (breaking protocol stability).

**Final Solution: Tag-Set**.
The fourth column became a comma-separated tag set (e.g., `archived,note,inbox`), and the style engine upgraded from "exact match" to "containment match." This change elevated expressive power from one dimension to N dimensions without altering the protocol's physical structure (still 4-column TSV).

---

## 3. Execution Model: Dual-Track and Deadlock Defense

### 3.1 Why Dual-Track is Needed

The TUI engine faces a fundamental contradiction: **there is only one terminal, but both the engine and external commands want to own it.**

*   When the user presses Enter to open `vim`, `vim` must own terminal control (Raw Mode, Alternate Screen, TTY stdin/stdout). The engine must completely yield.
*   When the user presses Ctrl-T to switch views, the business script only needs to modify a state file and trigger a refresh. It doesn't need terminal control; the engine shouldn't yield.

If both scenarios were handled the same way, either `vim` couldn't run properly (because the engine didn't yield TTY), or view switching would cause black-screen flickering (because the engine unnecessarily yielded TTY).

The **Dual-Track Execution Model** was thus born:
*   **Default Mode**: Yield everything. Suitable for full-screen interactive tools.
*   **`@` Silent Mode**: Retain everything. Suitable for pure background state changes.

### 3.2 fd Inheritance Deadlock in `@` Mode

This is the most dangerous system-level trap discovered during `stree` development.

**Trap Trigger Path**:
1.  User binds `--bind "ctrl-l=@sh -c 'long_task &'"`
2.  `stree` forks the `sh` process. If using `.output()`, Rust creates pipes connected to the child's stdout/stderr.
3.  `sh` executes `long_task &`. `long_task`, as a grandchild process, **inherits `sh`'s stderr pipe write-end by default**.
4.  `sh` exits immediately. But `long_task` still holds the pipe write-end.
5.  Rust's `.output()` blocks on `read(pipe)` waiting for EOF. Since `long_task` hasn't closed the write-end, EOF never arrives.
6.  `stree` main thread deadlocks. TUI becomes completely unresponsive.

**Defense Solution**:
In silent mode, redirect stdin, stdout, stderr **all to `/dev/null`**, and use `.status()` instead of `.output()`. `.status()` only calls `waitpid()` and reads no pipes. Even if background tasks inherit fd, they won't affect the main thread.

**Cost**: Cannot capture stderr error messages in silent mode. But this is a completely acceptable tradeoff—absolute UI fluidity takes priority over error text display. Exit codes remain available; the business layer can push detailed error information via IPC.

### 3.3 The Other Side of IPC Deadlock

When a business script in `@` mode internally calls `stree update` (IPC push), if the IPC call is synchronous, another deadlock forms:
*   `stree` main thread blocks waiting for the script to exit.
*   The script blocks waiting for `stree` to process the IPC request.

**Solution**: The business layer must put the IPC call in the background (`(...) &`), making the script exit instantly. The engine processes the IPC request in the next main loop poll. This isn't an engine deficiency—it's the correct collaboration contract under a synchronous architecture.

---

## 4. Partial Refresh: From Full Redraw to Targeted IPC

### 4.1 The Cost of Full Redraw

In early designs, after any external command execution, the engine would re-execute all `--tree` data source commands, rebuild the entire Dataset, then clear the screen and redraw.

For a knowledge base system like `brain-tui`, this meant:
*   Every view switch required re-reading frontmatter from hundreds of Markdown files.
*   Every file archive required rebuilding the entire tree.
*   The screen would briefly flicker (even with Alternate Screen, clear-and-redraw still causes visual jitter).

### 4.2 IPC Targeted Refresh Mechanism

The introduction of `stree update <window_name>` reduced refresh granularity from "global" to "component-level."

*   **Tree Component**: Receives new TSV stream, incrementally updates Dataset, retains expanded/selected state, only redraws that tree window.
*   **View Component**: Directly replaces content buffer, only redraws that preview window.
*   **StatusBar Component**: Directly replaces text, only redraws the status bar.

Other components remain completely unaffected. This "point-and-shoot" refresh mechanism is the core pillar enabling `stree`'s zero-delay interaction.

### 4.3 Caching and Debouncing

The View component has a built-in `cached_entity_id` mechanism. When the user moves the cursor in the tree, if the newly selected node ID matches the cache, the engine **skips command execution** and directly reuses the in-memory `content_buffer`.

This ensures that when rapidly pressing `j`/`k` to scroll through a tree with tens of thousands of nodes, no `fork()` is triggered, achieving true zero-I/O scrolling.

---

## 5. Layout Engine: From Special-Case Black Magic to Pure Orthogonal Flexbox

### 5.1 Early Black Magic

In V1.0, status bar handling worked like this: if a window's `border == None`, the engine would guess it's a status bar, forcibly truncate its height to 1 row, exclude it from percentage allocation in vertical layouts, and finally hard-insert it at the container bottom.

This logic was scattered across multiple locations in `compute_rects`, coupling "border style" and "space allocation"—two dimensions that should be orthogonal.

### 5.2 Introduction of Orthogonal Syntax

V2.0 introduced the `WindowSize` enum (`Percent(u16)` | `Absolute(u16)`), completely separating size declaration from border declaration.

*   `area(1)[none]:Status`: Explicitly declares "absolute height 1 row" and "no border." The engine doesn't need to guess.
*   `area(50%)[box]:Main`: Explicitly declares "occupies 50% space" and "has border."

### 5.3 Boundary Handling in Allocation Algorithm

Flexbox allocation faces a TUI-specific challenge: **integer division remainder**.

If container width is 100 characters and 3 nodes each take 33%, then `100 * 33 / 100 = 33`. Three nodes occupy 99 characters total, leaving 1 character as a black gap.

The engine's strategy: add the remainder to the **last valid flex node**. This ensures the container is always precisely filled, eliminating the most unsightly visual flaw in TUI layouts.

---

## 6. Rendering Pipeline: ANSI Pass-Through and Pixel-Level Truncation

### 6.1 Why Simple Truncation Doesn't Work

When a line of text exceeds window width, simple `chars().take(width)` causes disaster:
*   If the truncation point falls in the middle of an ANSI escape sequence (like `\x1b[31m`), the terminal's color state collapses, and colors in all subsequent lines become corrupted.
*   If the text contains wide characters (e.g., Chinese characters occupy 2 columns), truncating by character count causes actual display width to mismatch expectations.

### 6.2 Segmented Parsing

The engine's `draw_text` function implements ANSI-aware text rendering:
1.  Split text into alternating sequences of `Segment::Text` (plain text) and `Segment::Ansi` (escape sequences).
2.  Only calculate `UnicodeWidthChar` width for `Text` segments.
3.  When truncating, count by visible character width, ensuring ANSI sequences are never split.
4.  When exceeding width, append a `~` symbol at the end, precisely reserving 1 character width for the `~`.

---

## 7. Architectural Red Lines

The following principles are the constitution of the `stree` kernel. Any code change violating these principles will be considered architectural regression.

### 7.1 Never Hardcode Business Semantics
The engine doesn't understand what a "file," "note," or "archive" is. It only understands ID, Display, Path, and Tags. If code contains business judgments like `if status == "archived"`, that's a boundary violation.

### 7.2 Never Persist State
The engine is a stateless pure functional mapping. After restart, expanded state, selected state, and scroll offset all reset. Persistence is the business layer's responsibility.

### 7.3 Provide Mechanism, Not Policy
The engine provides tag-matching mechanisms but doesn't define tags' business meanings. The engine provides IPC channels but doesn't define pushed content formats. The engine provides placeholder expansion but doesn't define command business logic.

### 7.4 Purity of the Synchronous Main Loop
The main loop (observation loop) execution time must and can only be consumed by pure computation (rendering, event handling, IPC reception). Any physical operations that might block (file I/O, network requests, process waiting) must be stripped to child processes or background threads.

---

## 8. Performance Boundaries and Known Limitations

| Scenario | Performance | Reason |
| :--- | :--- | :--- |
| Scrolling through 10k+ node tree | Silky smooth (zero I/O) | `cached_entity_id` debouncing, memory cache hits |
| Continuous mouse wheel scrolling | Single fork | `move_up_n` batch movement, N displacements trigger only 1 broadcast |
| `@` silent mode instant switching | < 5ms | No TTY handoff, no Alternate Screen switching |
| Long-running background tasks in `@` mode | No UI blocking | `.status()` + `Stdio::null()` cuts pipe inheritance |
| Search filtering | Memory-level real-time | `match_entities` traverses `Vec<Entity>`, no I/O |
| IPC partial refresh | Single redraw | Only updates target component, other components completely unaffected |

### Known Limitations
*   **Single-Threaded Synchronous**: Executing View commands in `broadcast_selection_changed` is synchronously blocking. If a preview command takes a long time (e.g., rendering a large PDF), the UI will briefly stutter. Mitigation: business layer uses lightweight preview commands, or implements asynchronous caching within scripts.
*   **Full Rebuild**: `trigger_reload` (SIGUSR1) re-executes all Tree data source commands. For ultra-large datasets, recommend using IPC targeted updates instead of global reloads.
*   **No Incremental Diff**: When IPC pushes Tree data, the engine rebuilds the entire tree's memory structure. For frequent small-scale updates, this may introduce unnecessary CPU overhead.

---

## 9. Version Evolution Path

| Version | Core Changes |
| :--- | :--- |
| V1.0 | Foundation: protocol parsing, tree construction, layout rendering, keybinding execution |
| V1.1 | Kernel enhancement: full-field search, regex status styling, SIGUSR1 hot reload |
| V2.0 | Architectural refactor: IPC partial refresh, `@` silent execution, orthogonal layout syntax, tag-set style engine |
| Future | Possible directions: async View command execution, incremental Tree diff, WebSocket remote IPC |
