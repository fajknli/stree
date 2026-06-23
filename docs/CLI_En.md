# stree CLI & Protocol Reference

This document serves as the strict contract between the `stree` engine and the external business layer (glue scripts). Any deviation from these specifications may result in undefined behavior.

---

## 1. Data Protocols

### 1.1 Entity Data Stream (stdin)

The engine reads a Tab-Separated Values (TSV) stream from standard input. Each non-empty line must contain exactly 4 fields:

```text
ID \t Display \t Path \t Tags
```

#### Field Specifications

| Field | Constraints | Engine Behavior |
| :--- | :--- | :--- |
| **ID** | Non-empty string. Must be globally unique within the stream. | Used as the primary key for tree association, selection memory, and IPC targeting. Duplicate IDs are deduplicated (last occurrence wins). |
| **Display** | Any UTF-8 string. | Rendered verbatim in the tree list. The engine does not parse or interpret its content. |
| **Path** | Any UTF-8 string. | Treated as an opaque business payload. Passed as-is to external commands via the `{path}` placeholder. Double quotes inside the path are automatically escaped when expanded into `{paths}`. |
| **Tags** | Comma-separated list of labels (e.g., `live,note,inbox`). | Split by `,`, trimmed of surrounding whitespace, and empty labels are filtered out. Used by the Style Engine for matching and by the business layer for state filtering. |

#### Defensive Parsing Rules

To ensure robustness against dirty data from external scripts, the engine enforces the following:
1.  **Forced Trim**: Leading/trailing whitespace and carriage returns (`\r`) are stripped from every field.
2.  **Empty ID Rejection**: Lines with an empty `ID` field are silently skipped (a warning is emitted to stderr).
3.  **Field Count Check**: Lines with fewer than 4 fields are silently skipped.
4.  **Version Header (Optional)**: If the first field of the first line starts with `VERSION:`, the engine parses the version number and skips to the second field for the ID.

### 1.2 Relation Table (`--relations <file>`)

A 2-column TSV file defining parent-child relationships:

```text
Parent_ID \t Child_ID
```

*   **Tree Construction**: The engine builds a discrete tree topology in memory. Any ID that does not appear as a child in any relation becomes a root node.
*   **Ordering**: Root nodes are sorted alphabetically. Children within a node preserve the order of their appearance in the relation file.
*   **Cycle Tolerance**: The engine does not perform DAG cycle detection. Cyclic relations will result in undefined rendering behavior (the business layer is responsible for ensuring acyclicity).

---

## 2. IPC Binary Protocol

The engine exposes a Unix Domain Socket for targeted, partial updates. The socket path is exposed via the environment variable `$STREE_SOCK`.

### 2.1 Binary Frame Format

A client must send a binary frame with the following structure (all integers are **Big-Endian**):

| Offset | Size | Field | Description |
| :--- | :--- | :--- | :--- |
| 0 | 4 bytes | `target_len` | Length of the target window name (u32). Max: 128 bytes. |
| 4 | 8 bytes | `data_len` | Length of the payload data (u64). Max: 512 KB. |
| 12 | `target_len` | `target` | UTF-8 encoded window name (e.g., `MainTree`, `Preview`). |
| 12+N | `data_len` | `data` | UTF-8 encoded payload (TSV for Tree, plain text for View/StatusBar). |

### 2.2 Response

Upon successful processing, the engine replies with the ASCII string `OK`. If the frame is malformed (e.g., exceeds size limits), the engine replies with an `ERROR: ...` string and closes the connection.

### 2.3 CLI Client

The engine binary doubles as an IPC client:

```bash
# Push TSV data to a Tree window (triggers re-parse and partial redraw)
./generate-data.sh | stree update MainTree

# Push plain text to a View window (replaces content buffer)
echo "Loading..." | stree update Preview

# Push text to a StatusBar window (replaces format template output)
echo "Custom Status" | stree update Status
```

### 2.4 Semantics by Component Type

*   **Tree**: The payload must be a valid 4-column TSV stream. The engine re-parses it, rebuilds the tree, retains expanded/selected state where possible, and triggers a `broadcast_selection_changed` if the tree is currently focused.
*   **View**: The payload is treated as raw text. The `cached_entity_id` is invalidated to force a refresh on the next selection change.
*   **StatusBar**: The payload replaces the rendered text (bypassing the format template).

---

## 3. Layout Syntax & Allocation Rules

The `--layout` string defines the spatial topology. The engine uses a Flexbox-like allocation algorithm.

### 3.1 Node Syntax

```text
area(size)[border]:Name
```

*   **`size`**:
    *   `50%` : Percentage of the remaining space (after absolute sizes are deducted).
    *   `3` : Absolute size (rows for vertical containers, columns for horizontal).
    *   *(empty)* : Participates in the even distribution of remaining space.
*   **`border`**: `box` (default), `line` (single top line), `none` (borderless).
*   **`Name`**: Unique identifier for component mounting.

### 3.2 Allocation Algorithm (Strict Order)

For a container with total length `L` (width or height):

1.  **Deduct Absolute**: Sum all children with absolute sizes (`A`). Remaining space `R = L - A`.
2.  **Calculate Percentages**:
    *   If `undeclared_count > 0` AND `sum(percentages) <= 100`: Each percentage node gets `R * (pct / 100)`. The remaining space is evenly distributed among undeclared nodes.
    *   If `sum(percentages) > 100` OR `undeclared_count == 0`: Each percentage node gets `R * (pct / sum(percentages))`. This ensures the container is always filled, even if the user declares `200%` total.
3.  **Integer Remainder**: Due to integer division, pixel gaps may occur. The engine adds the remainder to the **last valid flex node** to eliminate visual seams.

### 3.3 Edge Cases

*   **Percentage Sum > 100**: Nodes are proportionally compressed to fit the container.
*   **Percentage Sum < 100 (with undeclared)**: Undeclared nodes absorb the remaining space evenly.
*   **Percentage Sum < 100 (no undeclared)**: All percentage nodes are proportionally stretched to fill the container.
*   **Absolute Sum > Total Length**: Absolute nodes are clamped via `saturating_sub`, and flex nodes receive 0 size.

---

## 4. Execution Model (`--bind`)

### 4.1 Default Mode (Full-Screen)

```bash
--bind "enter=vi {path}"
```

1.  Engine disables raw mode, leaves Alternate Screen, releases mouse capture.
2.  TTY (`/dev/tty`) is cloned and attached to the child process's stdin/stdout/stderr.
3.  Engine blocks on `child.wait()`.
4.  Upon exit, engine drains pending input events (max 100), re-enters Alternate Screen, and triggers `trigger_reload` (re-executes all `--tree` source commands).

### 4.2 Silent Mode (`@` Prefix)

```bash
--bind "ctrl-t=@switch-view.sh"
```

1.  Engine **retains** the TUI state (no screen flicker).
2.  Child process is spawned with `stdin=null`, `stdout=null`, `stderr=null`.
3.  Engine calls `.status()` instead of `.output()`. This is a **critical defense**: it prevents deadlocks caused by background tasks (`&`) inheriting pipe file descriptors.
4.  Engine drains pending input events.
5.  **No automatic reload**. The business script is fully responsible for pushing updates via IPC (`stree update`).

### 4.3 Placeholder Expansion

Placeholders are expanded **before** the command is split into arguments.

| Placeholder | Expansion Rule |
| :--- | :--- |
| `{id}` | Selected node's ID. Empty string if none. |
| `{path}` | Selected node's Path. Empty string if none. |
| `{ids}` | Space-separated IDs of all marked nodes. Falls back to `{id}` if no marks. |
| `{paths}` | Space-separated Paths, each wrapped in double quotes with internal quotes escaped. |
| `{window}` | Name of the focused window. |
| `{width}` | In `--view`: internal width of the View window. In `--bind`: total terminal width. |
| `{height}` | In `--view`: internal height of the View window. In `--bind`: total terminal height. |

---

## 5. Style Engine (`--status-col`)

### 5.1 Syntax

```bash
--status-col "pattern1=style1,pattern2=style2,..."
```

*   **Separator**: Rules are separated by commas. However, since styles themselves can contain commas (e.g., `red,bold`), the parser uses a **right-to-left** heuristic: it finds the last comma before the next `=` to separate the style from the next pattern.
*   **Pattern**:
    *   Exact string: `live`
    *   Regex: `^fail.*` (detected by the presence of `^`, `*`, `.`, `+`, `?`, `[`)
*   **Style**: A comma-separated list of `color` and/or `bold`.
    *   Colors: `black`, `red`, `green`, `yellow`, `blue`, `magenta`, `cyan`, `white`, `gray`/`grey`/`darkgray`/`darkgrey`.

### 5.2 Matching Semantics (Tag-Set)

The engine splits the node's 4th column (Tags) by comma. For each rule, it checks if **any** tag in the set matches the pattern.

*   **Color Override**: Later matching rules override earlier ones.
*   **Bold Accumulation**: If any matching rule specifies `bold`, the final result is bold.

**Example**:
```bash
--status-col "live=white,archived=gray,note=blue,inbox=cyan,^fail.*=red,bold"
```
For a node with Tags `note,archived,inbox`:
1.  `note=blue` matches -> color becomes blue.
2.  `archived=gray` matches -> color becomes gray (overrides blue).
3.  `inbox=cyan` matches -> color becomes cyan (overrides gray).
**Final Result**: Cyan text.

---

## 6. Signals

| Signal | Behavior |
| :--- | :--- |
| `SIGUSR1` | Triggers `trigger_reload`: re-executes the source command of every `--tree` component and updates their data via IPC internally. |
| `SIGINT` (Ctrl-C) | Initiates a graceful shutdown. The engine cleans up the socket file, restores terminal state, and exits. |

---

## 7. Exit Behavior

Upon graceful exit (via `q` or `Esc`), the engine outputs the **final selected node's ID** to standard output, followed by a newline. This allows shell wrappers to capture the user's selection:

```bash
selected=$(my-data.sh | stree --tree "Main")
echo "User selected: $selected"
```

If the engine crashes or is killed by an unhandled signal, the panic hook ensures the terminal state (raw mode, alternate screen, mouse capture) is restored and the socket file is deleted.

---

## 8. Environment Variables

| Variable | Scope | Description |
| :--- | :--- | :--- |
| `$STREE_SOCK` | Child processes | Absolute path to the Unix Domain Socket. Set by the engine before spawning any child process. Valid only for the lifetime of the engine process. |
| `$FORCE_COLOR` | Child processes | Set to `1` by the engine when spawning `--view` commands, forcing color output from tools like `bat` or `ls`. |
| `$CLICOLOR_FORCE` | Child processes | Set to `1` for BSD-style color forcing. |
| `$TERM` | Child processes | Set to `xterm-256color` to ensure maximum color compatibility. |
