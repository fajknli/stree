# stree Advanced Recipes

> This document demonstrates the extreme capabilities of `stree`. Every recipe comes from real production scenarios, proving that `stree` is far more than just a "directory viewer"—it's a generic foundation for building complex terminal applications.

---

## Recipe 1: Zero-Flicker Multi-View Switching

**Scenario**: Your TUI needs to support multiple views (e.g., `atoms` global view, `inbox` pending view, `timeline` view), with no black-screen flickering during switching.

**Core Mechanism**: `@` silent mode + IPC targeted refresh + background execution

### Step 1: Write the View Switching Script

Create `bin/switch-view.sh`:

```bash
#!/bin/sh
CACHE_DIR="${HOME}/.cache/myapp"
mkdir -p "$CACHE_DIR"

TARGET="$1"
CURRENT=$(cat "$CACHE_DIR/current_view" 2>/dev/null || echo "atoms")

# If no argument is passed, cycle through views
if [ -z "$TARGET" ]; then
    case "$CURRENT" in
        atoms)    TARGET="inbox" ;;
        inbox)    TARGET="timeline" ;;
        timeline) TARGET="atoms" ;;
    esac
fi

# 1. Write state file
echo "$TARGET" > "$CACHE_DIR/current_view"

# 2. Critical: Put IPC call in background so the script exits instantly
# Avoids deadlock with stree's main thread
(generate-data-for "$TARGET" | stree update MainTree) &
```

### Step 2: Bind in stree

```bash
stree \
  --tree "MainTree:my-data-source.sh" \
  --bind "ctrl-t=@bin/switch-view.sh" \
  ...
```

**Result**: Press `Ctrl-T`, and the view switches seamlessly in 0 milliseconds—no black screen, no flicker, no stutter.

**How It Works**:
1. The `@` prefix tells `stree` to retain TUI state without exiting Alternate Screen.
2. The script exits instantly, unblocking `stree`'s main thread.
3. The background `stree update` is processed in the next main loop iteration, achieving partial refresh.

---

## Recipe 2: Background Async Tasks with Auto-Push

**Scenario**: You want to integrate AI summarization, full-text search, or any time-consuming task into the TUI without freezing the UI.

**Core Mechanism**: `@` silent mode + background task + IPC auto-push

### Step 1: Write the Async Task Script

Create `bin/ai-summarize.sh`:

```bash
#!/bin/sh
TARGET_UID="$1"
[ -z "$TARGET_UID" ] && exit 1

# Return immediately to unblock stree's main thread
# The actual task runs in the background
nohup sh -c "
    # Simulate time-consuming AI call (in practice, this could be ollama, openai-cli, etc.)
    RESULT=\$(ollama run llama3 \"Summarize: \$(get-content $TARGET_UID)\" 2>/dev/null)
    
    # After task completion, push result to Preview window via IPC
    echo \"\$RESULT\" | stree update Preview
" >/dev/null 2>&1 &
```

### Step 2: Bind in stree

```bash
stree \
  --view "Preview:cat {path}" \
  --bind "ctrl-l=@bin/ai-summarize.sh {id}" \
  ...
```

**Result**:
1. Press `Ctrl-L`, and the TUI remains completely responsive.
2. After 3 seconds, the right-side `Preview` window **automatically and silently** refreshes with the AI's summary.
3. During those 3 seconds, you can continue browsing other nodes with `j`/`k`—completely unaffected.

**How It Works**:
- `nohup ... &` makes the script return instantly. `stree` receives exit code 0 and the main thread resumes immediately.
- The background task runs independently and pushes data via `$STREE_SOCK` upon completion.
- `stree update Preview` directly replaces the View's content buffer, triggering a partial redraw.

---

## Recipe 3: "Hacker-Level" Dynamic Status Bar

**Scenario**: You want the status bar to display real-time information (Git branch, system time, TODO count) rather than just static node statistics.

**Core Mechanism**: Background daemon + IPC forced override of StatusBar

### Step 1: Write the Status Daemon Script

Create `bin/status-daemon.sh`:

```bash
#!/bin/sh
# Run this script in the background when starting stree

while true; do
    # Collect various external states
    GIT_BRANCH=$(git branch --show-current 2>/dev/null || echo "no-git")
    TODO_COUNT=$(grep -rl "TODO" ~/notes 2>/dev/null | wc -l)
    LOAD=$(uptime | awk -F'load average:' '{print $2}' | cut -d, -f1)
    
    # Assemble status text
    # Note: You can use {stree_...} placeholders here; the engine will parse them again
    STATUS_TEXT="[Git: $GIT_BRANCH] [TODO: $TODO_COUNT] [Load: $LOAD] | {stree_visible}/{stree_total} ({stree_marked} marked)"
    
    # Force-override the Status component's text via IPC
    echo "$STATUS_TEXT" | stree update Status 2>/dev/null
    
    sleep 5
done
```

### Step 2: Mount in the Startup Script

```bash
#!/bin/sh
# Start the daemon
bin/status-daemon.sh &

# Start stree
exec stree \
  --statusbar "Status:Initializing..." \
  --tree "Main:my-data.sh" \
  ...
```

**Result**: The status bar automatically refreshes every 5 seconds with Git status, TODO count, and system load, while still retaining the `{stree_...}` engine built-in variables.

**How It Works**:
- `stree update Status` directly replaces the StatusBar's rendered text, bypassing the format template.
- The engine parses `{stree_...}` placeholders again during rendering, achieving "external state + internal state" hybrid rendering.

---

## Recipe 4: Smart Preview Router

**Scenario**: Your TUI needs to browse multiple file types (Markdown, code, PDF, images), but the Preview window can only bind to one command.

**Core Mechanism**: `{width}` placeholder + file type detection + adaptive rendering

### Step 1: Write the Preview Router Script

Create `bin/preview-router.sh`:

```bash
#!/bin/sh
FILE="$1"
WIDTH="$2"
HEIGHT="$3"

[ ! -f "$FILE" ] && echo "[File not found: $FILE]" && exit 0

# Select renderer based on MIME type
MIME=$(file -b --mime-type "$FILE")

case "$MIME" in
    text/*)
        # Text files: use bat for syntax highlighting, adapt to window width
        bat --terminal-width="$WIDTH" --color=always --style=plain "$FILE"
        ;;
    
    application/pdf)
        # PDF: extract text, limit lines to prevent screen flooding
        pdftotext -layout "$FILE" - | head -n "$HEIGHT"
        ;;
    
    image/*)
        # Images: use chafa for ASCII art (requires chafa installation)
        if command -v chafa >/dev/null 2>&1; then
            chafa --size="${WIDTH}x${HEIGHT}" "$FILE"
        else
            echo "[Image: $FILE]"
            echo "[Install 'chafa' for ASCII art preview]"
        fi
        ;;
    
    application/zip|application/x-tar|application/gzip)
        # Archives: list contents
        case "$MIME" in
            application/zip)     unzip -l "$FILE" ;;
            application/x-tar)   tar -tvf "$FILE" ;;
            application/gzip)    tar -tzvf "$FILE" ;;
        esac
        ;;
    
    audio/*)
        # Audio: display metadata
        echo "[Audio: $FILE]"
        ffprobe -v quiet -show_entries format=duration,bit_rate -of default=noprint_wrappers=1 "$FILE" 2>/dev/null
        ;;
    
    *)
        echo "[Unknown binary: $MIME]"
        echo "[Size: $(du -h "$FILE" | cut -f1)]"
        ;;
esac
```

### Step 2: Mount in stree

```bash
stree \
  --view "Preview:bin/preview-router.sh {path} {width} {height}" \
  ...
```

**Result**:
- Select a `.md` file → syntax-highlighted rendering
- Select a `.pdf` file → text extraction preview
- Select a `.png` file → ASCII art preview (if `chafa` is installed)
- Select a `.zip` file → archive contents listing

**How It Works**:
- `{width}` and `{height}` expand to the **internal dimensions of that View window** in the View context.
- The preview script adapts its output based on window dimensions, avoiding content overflow or whitespace.

---

## Recipe 5: Multi-Tree Focus Isolation and Coordination

**Scenario**: Your TUI has multiple trees (e.g., "File Tree" and "Tag Tree"), and you want only the currently focused tree to drive Preview refreshes, avoiding unnecessary I/O during switching.

**Core Mechanism**: Multiple `--tree` mounts + focus-aware broadcasting

### Step 1: Configure Multiple Trees

```bash
stree \
  --layout "horizontal(area(30%):Files, area(30%):Tags, area(40%):Preview)" \
  --tree "Files:ls-files.sh" \
  --tree "Tags:ls-tags.sh" \
  --view "Preview:cat {path}" \
  ...
```

### Step 2: Understand Engine Behavior

`stree`'s `broadcast_selection_changed` mechanism:
- When the user moves the cursor in the `Files` tree, only the `Files` tree's selection change triggers `Preview` refresh.
- When the user moves the cursor in the `Tags` tree, only the `Tags` tree's selection change triggers `Preview` refresh.
- The two trees are completely independent and don't interfere with each other.

**Result**:
- Rapidly scroll in the `Files` tree → Preview displays file content
- Press `Tab` to switch to the `Tags` tree → Preview displays tag descriptions (if the Tags tree's Path points to tag description files)
- The switching process has zero delay because the engine only triggers View refresh when the focused tree changes.

**How It Works**:
- `Engine::broadcast_selection_changed` checks `is_focused_tree`; only the focused tree drives View refresh.
- Selection changes in non-focused trees only update internal state without triggering any external commands.

---

## Recipe 6: Batch Operations and Multi-Select Marking

**Scenario**: You want to batch archive multiple files, batch delete multiple nodes, or batch execute a command.

**Core Mechanism**: `Space` multi-select + `{ids}` / `{paths}` placeholders

### Step 1: Write the Batch Operation Script

Create `bin/batch-archive.sh`:

```bash
#!/bin/sh
# Receives space-separated path list
for path in "$@"; do
    [ -z "$path" ] && continue
    [ ! -f "$path" ] && continue
    
    # Execute archive logic
    mv "$path" "${path}.archived"
    echo "Archived: $path"
done

# After completion, refresh Tree via IPC
generate-file-list | stree update Files
```

### Step 2: Bind in stree

```bash
stree \
  --tree "Files:generate-file-list.sh" \
  --bind "ctrl-d=@bin/batch-archive.sh {paths}" \
  ...
```

**Result**:
1. Use `Space` to mark multiple nodes (cursor automatically moves down after marking).
2. Press `Ctrl-D`, and all marked files are batch archived.
3. After archiving completes, the Tree automatically refreshes, and the marked nodes disappear from the list.

**How It Works**:
- `{paths}` expands to the Path list of all marked nodes, with each Path wrapped in double quotes and internal quotes escaped.
- The script receives multiple arguments and processes them in a loop.
- After processing, new data is pushed via IPC, triggering a partial refresh.

### Critical Contract: `@` Mode Does Not Auto-Refresh

In default mode, after a script exits, the engine automatically triggers `trigger_reload()`, re-executing all Tree data source commands.

But in `@` silent mode, **the engine does not auto-refresh**. The script must explicitly call `stree update <window_name>` to push new data.

This is a direct manifestation of the "engine does not overstep its boundary" design philosophy: in silent mode, the initiative for refreshing is **100% handed over to the business script**.

If you find that the UI doesn't update in `@` mode, first check whether your script has an IPC push at the end.

---

## Recipe 7: Search Filtering and Highlight Coordination

**Scenario**: You want to search in real-time within the tree, with matches highlighted, non-matches collapsed, while maintaining the tree hierarchy structure.

**Core Mechanism**: `/` search mode + memory-level filtering + automatic ancestor expansion

### Step 1: Use Built-in Search Directly

```bash
stree \
  --tree "Main:my-data.sh" \
  ...
```

### Step 2: Understand Engine Behavior

`stree`'s search mechanism:
1. Press `/` to enter search mode; the status bar displays a `/` prompt.
2. Enter a keyword (e.g., `rust`), and the engine traverses all nodes' ID, Display, Path, and Tags fields in memory.
3. Matching nodes are added to the `matched_ids` set.
4. The engine automatically expands all ancestor nodes of matching nodes, ensuring matches are visible.
5. Non-matching nodes are filtered out, and the tree structure remains intact.
6. Press `n` / `N` to jump to the next / previous match.
7. Press `Esc` to exit search mode and restore the full tree.

**Result**:
- In a tree with tens of thousands of nodes, search response time is < 10ms (pure memory traversal, no I/O).
- Matches are highlighted (yellow bold).
- The tree hierarchy remains intact; it doesn't become a flat list.

**How It Works**:
- `match_entities` traverses `Vec<Entity>`, performing case-insensitive substring matching on 4 fields.
- `collect_ancestors_inner` recursively calculates all ancestors of matching nodes, adding them to `ancestors_of_matched`.
- `rebuild_visible_ids` in filter mode only collects matching nodes and their ancestors.

---

## Recipe 8: Custom Styling and Multi-Dimensional Tags

**Scenario**: You want to display different colors based on multiple file attributes (type, status, priority, Git status).

**Core Mechanism**: Tag set + regex matching + last-rule-wins override

### Step 1: Output Multi-Dimensional Tags in the Data Source

```bash
#!/bin/sh
# When generating data, output comma-separated tag set in the 4th column
for file in *.md; do
    id=$(basename "$file" .md)
    type="note"
    status="live"
    priority="normal"
    git_status="clean"
    
    # Detect Git status
    if git ls-files --modified | grep -q "$file"; then
        git_status="modified"
    fi
    
    # Detect priority (assuming frontmatter has a priority field)
    if grep -q "priority: high" "$file"; then
        priority="high"
    fi
    
    # Output 4-column TSV, 4th column is tag set
    echo -e "$id\t$id\t$file\t$status,$type,$priority,$git_status"
done
```

### Step 2: Configure Style Rules

```bash
stree \
  --status-col "live=white,archived=gray,note=blue,task=yellow,high=red,bold,modified=green" \
  ...
```

**Result**:
- A `live` `note` with `normal` priority and `clean` Git status → blue (`note=blue`)
- An `archived` `task` with `high` priority and `modified` Git status → green (`modified=green` matches last, overriding previous colors) + bold (from `high=red,bold`'s `bold` accumulation)

**How It Works**:
- The style engine splits the 4th column by comma, obtaining the tag set `["archived", "task", "high", "modified"]`.
- It traverses style rules, checking if each tag matches.
- Color: later matching rules override earlier ones (`modified=green` overrides `high=red`).
- Bold: if any rule matches `bold`, it takes effect (accumulation logic).

---

## Recipe 9: Deep Integration with External Tools

**Scenario**: You want to integrate external tools like `fzf`, `vim`, `less` into the TUI for seamless switching.

**Core Mechanism**: Default mode (full-screen interaction) + TTY handoff

### Step 1: Bind Full-Screen Interactive Tools

```bash
stree \
  --bind "enter=vi {path}" \
  --bind "ctrl-f=fzf < {path}" \
  --bind "ctrl-p=less {path}" \
  ...
```

**Result**:
- Press `Enter` → `stree` hides, `vi` opens the file in full screen. After exiting `vi`, `stree` restores.
- Press `Ctrl-F` → `stree` hides, `fzf` searches within the file content. After exiting, `stree` restores.
- Press `Ctrl-P` → `stree` hides, `less` paginates through the file. After exiting, `stree` restores.

**How It Works**:
- In default mode, `stree` disables raw mode, leaves Alternate Screen, and releases mouse capture.
- TTY (`/dev/tty`) is cloned and attached to the child process's stdin/stdout/stderr.
- After the child process exits, `stree` re-enters Alternate Screen, enables raw mode, and restores mouse capture.
- Pending input events (up to 100) are drained to prevent keys pressed in the external tool from accidentally triggering in `stree` after restoration.

---

## Recipe 10: Building a Complete Terminal Knowledge Base (brain-tui in Practice)

**Scenario**: You want to use `stree` to build a complete personal knowledge base system, supporting note management, tag classification, relationship graphs, and full-text search.

**Core Mechanism**: Comprehensive use of all the above recipes

### Architecture Overview

```
brain-tui/
├── bin/
│   ├── brain-tui          # Main startup script
│   ├── brain-list         # Data source: generates 4-column TSV
│   ├── brain-context      # Preview script: displays inlinks/outlinks
│   ├── brain-switch-view  # View switching (Recipe 1)
│   ├── brain-ai-summarize # AI summarization (Recipe 2)
│   ├── status-daemon      # Status bar daemon (Recipe 3)
│   └── preview-router     # Smart preview (Recipe 4)
├── atoms/                 # Note files
├── inbox/                 # Pending fragments
└── cache/                 # Index cache
```

### Main Startup Script

```bash
#!/bin/sh
# bin/brain-tui

# Start status daemon
bin/status-daemon.sh &

# Start stree
exec stree \
  --relations "$HOME/brain/cache/links.tsv" \
  --layout "vertical(horizontal(area(50%):MainTree, vertical(area(50%):Context, area(50%):Preview)), area(1)[none]:Status)" \
  --tree "MainTree:bin/brain-list" \
  --view "Context:bin/brain-context {id}" \
  --view "Preview:bin/preview-router.sh {path} {width} {height}" \
  --statusbar "Status:Initializing..." \
  --bind "enter=vi {path}" \
  --bind "ctrl-d=@bin/brain-archive {paths}" \
  --bind "ctrl-o=@bin/brain-link-to {ids} {path}" \
  --bind "ctrl-t=@bin/brain-switch-view" \
  --bind "ctrl-l=@bin/brain-ai-summarize {id}" \
  --status-col "live=white,archived=gray,note=blue,task=yellow,inbox=cyan,idea=magenta,^fail.*=red,bold"
```

**Result**:
- Left: Note tree list, supporting search, multi-select, expand/collapse
- Top-right: Current note's inlink/outlink graph
- Bottom-right: Smart preview (Markdown highlighting, PDF text, image ASCII art)
- Bottom: Real-time status bar (Git branch, TODO count, system load)
- `Ctrl-T`: Zero-flicker switching between atoms/inbox/timeline views
- `Ctrl-L`: Background AI summarization, auto-pushes to Preview
- `Ctrl-D`: Batch archive selected notes
- `Ctrl-O`: Batch establish link relationships

**This is `stree`'s极限**: It's not a tool; it's a **meta-tool for building tools**.

---

## Conclusion

The above 10 recipes demonstrate `stree`'s core capabilities:
- **Zero-delay interaction** (Recipes 1, 2)
- **Dynamic state management** (Recipe 3)
- **Intelligent content adaptation** (Recipe 4)
- **Multi-component coordination** (Recipe 5)
- **Batch operations** (Recipe 6)
- **Real-time search** (Recipe 7)
- **Multi-dimensional styling** (Recipe 8)
- **External tool integration** (Recipe 9)
- **Complete application building** (Recipe 10)

`stree`'s design philosophy is: **Provide mechanism, not policy**. It gives you the most powerful primitives, allowing you to freely compose any terminal application you desire.

Now, it's your turn. Go create your own terminal universe.
