# stree

> A pure Unix pipeline-driven, declarative terminal TUI reactive engine.

`stree` is a generic terminal tree-view engine. It binds to no specific business and hardcodes no business logic. Its responsibility is purely focused: receiving standardized TSV data streams and relation tables, rendering them into an interactive multi-panel TUI, and accurately routing user interaction intents to external scripts.

It is a multi-dimensional tree-based upgrade to `fzf`, and serves as a generic foundation for building terminal knowledge bases, process tree viewers, log browsers, or any structured data explorer.

---

## Core Architectural Features

### 1. Protocol-Driven and Zero Business Intrusion
The engine does not understand whether your data represents files, processes, or JSON nodes. As long as you can output a standard 4-column TSV data stream using `awk`, `stree` can transform it into an interactive tree with previews. All business logic (data fetching, validation, write operations) is entirely handled by external glue scripts.

### 2. Orthogonal Declarative Layout
Completely abandoned the "border is state" black magic. The layout syntax completely decouples **geometric dimensions** from **visual borders**:
*   **Dimension `()`**: Supports percentages (e.g., `50%`) and absolute values (e.g., `3` rows/cols). Nodes with undeclared sizes will evenly distribute the remaining space.
*   **Border `[]`**: Supports `box` (default), `line` (single top line), and `none` (borderless).
The engine employs a Flexbox-like allocation algorithm: it deducts absolute sizes first, then distributes the remaining space by percentage, completely eliminating pixel gaps.

### 3. IPC-Targeted Partial Refresh
After an external script finishes execution, it can push new data precisely via Unix Socket by calling `stree update <window_name>`. The engine only redraws the specified component, never triggering a full redraw, achieving true zero-delay partial refresh.

### 4. Dual-Track Execution Model
*   **Default Mode**: The engine exits the Alternate Screen and yields TTY control. Perfectly compatible with heavy interactive tools that require full-screen takeover, such as `vim`, `fzf`, and `less`.
*   **`@` Silent Mode**: Prefixing a bound command with `@` (e.g., `@switch-view.sh`). The engine retains the TUI state, and the underlying implementation cuts off the child process's fd (file descriptor) pipe inheritance (preventing deadlocks). Combined with IPC, this achieves flicker-free, zero-latency background state switching.

### 5. Tag-Set Style Engine
The fourth column (status column) is no longer a single string, but a comma-separated **tag set** (e.g., `live,note,inbox`). The style engine adopts a "last-match-wins" mechanism, supporting the overlay and priority control of multi-dimensional styles, achieving extremely fine-grained visual feedback with minimal configuration.

### 6. Memory Buffering and Zero-I/O Preview
When moving the cursor, if the preview target has not changed, the engine directly reuses the memory cache (`cached_entity_id`) and never forks the process repeatedly. Scrolling through tens of thousands of nodes remains silky smooth.

---

## Protocol Specifications

### Entity Data Stream (stdin)
Standard input must be a 4-column Tab-Separated Values (TSV) text stream:
```text
ID \t Display \t Path \t Tags
```
*   **ID**: Globally unique identifier, used for tree association and state memory.
*   **Display**: Pure text displayed in the tree list.
*   **Path**: Business payload. The engine does not parse this field; it only passes it as-is to external commands when triggering shortcuts.
*   **Tags**: Comma-separated tag set (e.g., `live,archived,idea`). Used for style engine matching and business-layer state filtering.

### Relation Table (`--relations`)
Pass the parent-child relationship file via `--relations <file>`, formatted as a 2-column TSV:
```text
Parent_ID \t Child_ID
```
The engine builds a discrete tree topology in memory based on this table. Any ID that does not appear as a child in any relation will automatically become a root node.

---

## Layout and Component Syntax

### Layout String (`--layout`)
Syntax: `area(size)[border]:Name`
*   `size`: `50%` (percentage) or `3` (absolute rows/cols). Leave empty to participate in the even distribution of remaining space.
*   `border`: `box` (default), `line`, `none`. Leave empty for `box`.
*   `Name`: The unique identifier for component mounting.

**Example**:
```text
vertical(
  horizontal(area(50%):MainTree, area(50%):Preview),
  area(1)[none]:Status
)
```
*Parsing: Outer vertical layout. Top is a horizontal layout (left MainTree takes 50%, right Preview takes 50%); bottom is a Status area with an absolute height of 1 row and no border.*

### Component Mounting
*   `--tree "Name:reload_script.sh"`: Mounts the tree component and binds the data source script (used for initialization and SIGUSR1 hot reloading).
*   `--view "Name:command_template"`: Mounts the preview component. Supports placeholders and enjoys memory caching.
*   `--statusbar "Name:format_template"`: Mounts the status bar. Supports built-in engine variables (e.g., `{stree_visible}`).

---

## Context Placeholders

The following placeholders are supported in command templates for `--view`, `--bind`, and `--statusbar`:

| Placeholder | Description |
| :--- | :--- |
| `{id}` | ID of the currently selected node |
| `{path}` | Path of the currently selected node |
| `{ids}` | Space-separated list of IDs of all marked (multi-selected) nodes |
| `{paths}` | Space-separated list of Paths of all marked nodes (with quotes automatically escaped) |
| `{window}` | Name of the window that triggered the event |
| `{width}` | Internal physical width (in characters) of the current View window |
| `{height}` | Internal physical height (in rows) of the current View window |
| `{stree_visible}` | Number of visible nodes in the current focused tree |
| `{stree_total}` | Total number of nodes in the current focused tree |
| `{stree_marked}` | Number of marked nodes in the current focused tree |

---

## Dual-Track Execution & Keybindings (`--bind`)

Keybinding syntax: `--bind "key=command"`

### Default Mode (Full-screen Interactive)
```bash
--bind "enter=vi {path}"
```
When Enter is pressed, `stree` hides the TUI and yields terminal control to `vi`. After `vi` exits, `stree` restores the TUI and automatically triggers a data source reload.

### `@` Silent Mode (Zero-Latency Switching)
```bash
--bind "ctrl-t=@switch-view.sh"
```
When Ctrl-T is pressed, `stree` **does not** exit the TUI. `switch-view.sh` executes silently in the background (its stdout/stderr are discarded to cut off pipe deadlocks). After the script finishes, it can push new data via IPC, achieving a flicker-free view switch.

---

## IPC Targeted Updates

Upon startup, `stree` creates a Unix Socket and exposes its path in the environment variable `$STREE_SOCK`.
External scripts can push data to a specific window using the following command:

```bash
# Regenerate data and push to the MainTree window
./generate-data.sh | stree update MainTree

# Push plain text directly to the Preview window
echo "Hello World" | stree update Preview
```
After receiving the IPC data, the engine parses and updates only the target component's state, followed by triggering a partial redraw of that component.

---

## Style Engine (`--status-col`)

Configure the mapping rules from tags to styles via `--status-col`. The syntax is `pattern=style`, with multiple rules separated by commas.

**Example**:
```bash
--status-col "live=white,archived=gray,note=blue,inbox=cyan,^fail.*=red,bold"
```
*   **Exact Match**: `live=white` (When the Tags contain `live`, the text turns white).
*   **Regex Match**: `^fail.*=red` (When the Tags contain a tag starting with `fail`, the text turns red).
*   **Style Overlay**: `bold` can be appended after a color.
*   **Priority**: Rules are matched from left to right; **later matches override earlier matches**. Therefore, highest-priority styles (like `archived`) should be placed at the end.

---

## Interaction Paradigm

### Keyboard Navigation (Vim Paradigm)
| Key | Action |
| :--- | :--- |
| `j` / `k` | Move cursor up/down (scrolls content in View windows) |
| `g` / `G` | Jump to top / bottom |
| `h` / `l` | Collapse / expand current node |
| `Space` | Mark/unmark current node (enter multi-select mode) |
| `Tab` | Switch focus among multiple non-Status windows |
| `/` | Enter search mode (memory-level real-time filtering, preserves tree hierarchy) |
| `n` / `N` | Jump to next / previous match |
| `Esc` | Exit search mode / clear search query |
| `q` | Quit program (outputs the final selected ID to stdout upon exit) |

### Mouse Support
*   **Left Click**: Select node / switch window focus.
*   **Left Double-Click**: Expand/collapse node.
*   **Right Click**: Mark/unmark node.
*   **Scroll Wheel**: Scroll the content of the window currently hovered by the mouse (batch movement to avoid repeated forking).

---

## Architectural Red Lines (Kernel Design Principles)

The `stree` engine strictly adheres to the following architectural constitution. Any PR violating these principles will be rejected:
1.  **Never hardcode business paths or commands.**
2.  **Never embed business validation logic** (e.g., DAG cycle detection, data deduplication).
3.  **Never read business-specific cache files** (all data must be injected via standard streams or IPC).
4.  **Never persist any state** (the engine is a stateless pure functional mapping; restarting resets everything).
5.  **Provide mechanism, not policy** (the engine provides the tag-matching mechanism, but does not define the business semantics of the tags).

---

## Installation & Compilation

```bash
git clone https://github.com/fajknli/stree.git
cd stree
cargo build --release
# Binary located at target/release/stree
```

---

## License

MIT
