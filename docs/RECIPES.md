# stree 高级实战食谱 (V3.0)

> 本文档展示 `stree` V3.0 引擎的极限能力。每一个食谱都来自真实的生产场景，证明 `stree` 绝不仅仅是一个“看目录的工具”，而是一个融合了响应式状态、异步管线与 AST 动态布局的通用 TUI 应用底座。

---

## 食谱 1：零闪烁多视图切换

**场景**：你的 TUI 需要支持多个视图（如 `atoms` 全局视图、`inbox` 待处理视图），切换时不能有黑屏闪烁。

**核心机制**：`@` 静默模式 + IPC 定向刷新 + 后台执行

### 步骤 1：编写视图切换脚本

创建 `bin/switch-view.sh`：

```bash
#!/bin/sh
CACHE_DIR="${HOME}/.cache/myapp"
mkdir -p "$CACHE_DIR"

TARGET="$1"
CURRENT=$(cat "$CACHE_DIR/current_view" 2>/dev/null || echo "atoms")

if [ -z "$TARGET" ]; then
    case "$CURRENT" in
        atoms)    TARGET="inbox" ;;
        inbox)    TARGET="timeline" ;;
        timeline) TARGET="atoms" ;;
    esac
fi

echo "$TARGET" > "$CACHE_DIR/current_view"

# 1. 写入状态文件
echo "$TARGET" > "$CACHE_DIR/current_view"

# 2. 关键：将 IPC 调用放入后台，让脚本瞬间退出，避免与 stree 主线程形成死锁
(generate-data-for "$TARGET" | stree update MainTree) &
```

### 步骤 2：在 stree 中绑定

```bash
stree \
  --tree "MainTree:my-data-source.sh" \
  --bind "ctrl-t=@bin/switch-view.sh" \
  ...
```

**效果**：按下 `Ctrl-T`，视图在 0 毫秒内无缝切换，无任何黑屏、无闪烁、无卡顿。

**原理**：
1. `@` 前缀让 `stree` 保留 TUI 状态，不退出 Alternate Screen，且将子进程的 stdout/stderr 重定向到 `/dev/null` 防止管道死锁。
2. 脚本瞬间退出，`stree` 主线程解除阻塞。
3. 后台的 `stree update` 在下一轮主循环中被非阻塞 `try_recv` 处理，实现局部定向刷新。

---

## 食谱 2：后台异步任务与自动推流

**场景**：你想在 TUI 中集成 AI 总结、全文检索、或任何耗时任务，但又不想让 UI 卡住等待。

**核心机制**：`@` 静默模式 + `nohup` 后台任务 + IPC 自动推送

### 步骤 1：编写异步任务脚本

创建 `bin/ai-summarize.sh`：

```bash
#!/bin/sh
TARGET_UID="$1"
[ -z "$TARGET_UID" ] && exit 1

# 立即返回，让 stree 主线程解除阻塞
# 真正的任务在后台执行
nohup sh -c "
    # 模拟耗时的 AI 调用
    RESULT=\$(ollama run llama3 \"Summarize: \$(get-content $TARGET_UID)\" 2>/dev/null)

    # 任务完成后，通过 IPC 将结果推送到 Preview 窗口
    echo \"\$RESULT\" | stree update Preview
" >/dev/null 2>&1 &
```

### 步骤 2：在 stree 中绑定

```bash
stree \
  --view "Preview:cat {path}" \
  --bind "ctrl-l=@bin/ai-summarize.sh {id}" \
  ...
```

**效果**：
1. 按下 `Ctrl-L`，TUI 零卡顿。
2. 3 秒后，右侧 `Preview` 窗口**自动、无声地**刷新出 AI 的总结结果。
3. 在这 3 秒内，你可以继续用 `j`/`k` 浏览其他节点，完全不受影响。

**原理**：`stree` 的 `@` 模式只调用 `.status()` 等待直接子进程退出。`nohup ... &` 让脚本瞬间返回，真正的耗时任务作为孙进程独立运行，完成后通过 `$STREE_SOCK` 推送数据，直接替换 View 的内容缓冲区。

---

## 食谱 3：状态栏的“黑客级”动态化

**场景**：你想让状态栏显示实时信息（Git 分支、系统时间、待办数量），而不仅仅是静态的节点统计。

**核心机制**：后台守护进程 + IPC 强行覆盖 StatusBar + 混合占位符渲染

### 步骤 1：编写状态守护脚本

创建 `bin/status-daemon.sh`：

```bash
#!/bin/sh
while true; do
    GIT_BRANCH=$(git branch --show-current 2>/dev/null || echo "no-git")
    TODO_COUNT=$(grep -rl "TODO" ~/notes 2>/dev/null | wc -l)

    # 拼装状态文本，这里可以直接混合使用 {stree_...} 引擎内置占位符
    STATUS_TEXT="[Git: $GIT_BRANCH] [TODO: $TODO_COUNT] | {stree_visible}/{stree_total} ({stree_marked} marked)"

    # 通过 IPC 强行覆盖 Status 组件的文本
    echo "$STATUS_TEXT" | stree update Status 2>/dev/null

    sleep 5
done
```

### 步骤 2：在启动脚本中挂载

```bash
#!/bin/sh
# 启动守护进程
bin/status-daemon.sh &

# 启动 stree
exec stree \
  --statusbar "Status:Initializing..." \
  --tree "Main:my-data.sh" \
  ...
```

**效果**：状态栏每隔 5 秒自动刷新 Git 状态、待办数量，同时依然保留 `{stree_visible}` 等引擎内部状态的实时更新。

**原理**：`stree update Status` 直接替换 StatusBar 的渲染文本。引擎在渲染该文本时，会再次解析内部的 `{stree_...}` 占位符，实现“外部状态 + 内部状态”的混合渲染。

---

## 食谱 4：智能预览路由器与零 I/O 滚动

**场景**：你的 TUI 需要浏览多种文件类型（Markdown、代码、PDF、图片），但 Preview 窗口只能绑定一个命令，且要求移动光标时零延迟。

**核心机制**：`{width}` 占位符 + 文件类型检测 + 引擎内置 `cached_entity_id` 防抖

### 步骤 1：编写预览路由脚本

创建 `bin/preview-router.sh`：

```bash
#!/bin/sh
FILE="$1"
WIDTH="$2"
HEIGHT="$3"

[ ! -f "$FILE" ] && echo "[File not found: $FILE]" && exit 0

MIME=$(file -b --mime-type "$FILE")

case "$MIME" in
    text/*)
        bat --terminal-width="$WIDTH" --color=always --style=plain "$FILE"
        ;;
    application/pdf)
        pdftotext -layout "$FILE" - | head -n "$HEIGHT"
        ;;
    image/*)
        if command -v chafa >/dev/null 2>&1; then
            chafa --size="${WIDTH}x${HEIGHT}" "$FILE"
        else
            echo "[Install 'chafa' for ASCII art preview]"
        fi
        ;;
    *)
        echo "[Unknown binary: $MIME]"
        ;;
esac
```

### 步骤 2：在 stree 中挂载

```bash
stree \
  --view "Preview:bin/preview-router.sh {path} {width} {height}" \
  ...
```

**效果**：
- 选中 `.md` → 语法高亮；选中 `.png` → 字符画预览。
- 在万级节点树中快速按 `j`/`k` 滚动时，如果连续经过多个同类文件，UI 丝滑无卡顿。

**原理**：
1. `{width}` 和 `{height}` 在 View 上下文中展开为**该 View 窗口的内部物理尺寸**，脚本自适应输出。
2. View 组件内置了 `cached_entity_id` 防抖机制。移动光标时，若新选中的 ID 与缓存一致，引擎跳过命令执行，直接复用内存缓冲区，实现零 I/O 滚动。未命中缓存时，命令在后台线程异步执行，不阻塞主循环。

---

## 食谱 5：多树焦点隔离与联动

**场景**：你的 TUI 有多棵树（如“文件树”和“标签树”），希望只有当前焦点树驱动 Preview 刷新，避免切换时产生不必要的 I/O。

**核心机制**：多 `--tree` 挂载 + 引擎焦点感知广播

### 配置多棵树

```bash
stree \
  --layout "horizontal(area(30%):Files, area(30%):Tags, area(40%):Preview)" \
  --tree "Files:ls-files.sh" \
  --tree "Tags:ls-tags.sh" \
  --view "Preview:cat {path}" \
  ...
```

**效果**：
- 在 `Files` 树中快速滚动 → Preview 显示文件内容。
- 按 `Tab` 切换到 `Tags` 树 → Preview 显示标签描述。
- 切换过程零延迟，因为引擎只在焦点树变化时才触发 View 异步刷新。

**原理**：`Engine::broadcast_selection_changed` 严格检查 `is_focused_tree`。非焦点树的选中变化只更新内部状态，不触发任何外部命令或 IPC。

---

## 食谱 6：批量操作与多选标记

**场景**：你想批量归档多个文件或批量执行某个命令。

**核心机制**：`Space` 多选 + `{paths}` 空格安全展开

### 步骤 1：编写批量操作脚本

创建 `bin/batch-archive.sh`：

```bash
#!/bin/sh
# 接收空格分隔的路径列表
for path in "$@"; do
    [ -z "$path" ] && continue
    mv "$path" "${path}.archived"
done

# 完成后通过 IPC 刷新 Tree
generate-file-list | stree update Files
```

### 步骤 2：在 stree 中绑定

```bash
stree \
  --tree "Files:generate-file-list.sh" \
  --bind "ctrl-d=@bin/batch-archive.sh {paths}" \
  ...
```

**效果**：
1. 用 `Space` 标记多个节点（标记后光标自动下移）。
2. 按 `Ctrl-D`，所有被标记的文件被批量归档。
3. 归档完成后，Tree 自动刷新。

**原理**：
1. `{paths}` 展开为所有被标记节点的 Path 列表，每个 Path 用双引号包裹，内部引号被转义。
2. **Split-Before-Replace 机制**：引擎先将命令模板拆分为参数数组，再通过统一的 `ExecutionContext` (HashMap) 替换占位符。无论 `{paths}` 包含多少空格或特殊字符，都会被安全地视为单个参数传递给脚本。

---

## 食谱 7：扁平化实时搜索与防幽灵过滤

**场景**：你想在万级节点的树中快速定位某个文件，要求输入即过滤，零延迟响应。

**核心机制**：`/` 搜索模式 + 内存级扁平过滤 + 焦点锁定

### 直接使用内置搜索

```bash
stree \
  --tree "Main:my-data.sh" \
  ...
```

### 理解引擎行为

`stree` 的搜索机制是一种“fzf 风格”的实时过滤：
1. 按 `/` 进入搜索模式，状态栏（或被劫持的输入框区域）显示 `/` 提示符。
2. 输入关键词（如 `rust`），引擎在内存中对所有节点的 `ID`、`Display`、`Path` 字段进行不区分大小写的子串匹配。
3. 匹配的节点被提取出来，**重组为一个扁平的列表**（忽略原有的层级关系）。
4. 每输入一个字符，过滤结果实时更新，光标自动停留在第一个匹配项上。
5. 此时按 `j` / `k` 可以在匹配项列表中上下移动，右侧的 `Preview` 会实时刷新。
6. 按 `Esc` 退出搜索模式，树会**立刻恢复完整的层级结构**和之前的展开状态。

**防幽灵契约**：引擎**仅搜索内容层**（`ID`, `Display`, `Path`），**绝对不搜索元数据层**（`Tags`）。防止搜索 "li" 时，因隐藏的 "live" 标签导致所有节点全亮。

---

## 食谱 8：自定义样式与多维度标签

**场景**：你想根据文件的多种属性（类型、状态、优先级、Git 状态）显示不同颜色。

**核心机制**：标签集 + 正则匹配 + 后规则覆盖

### 步骤 1：在数据源中输出多维度标签

```bash
#!/bin/sh
for file in *.md; do
    id=$(basename "$file" .md)
    type="note"
    status="live"
    priority="normal"
    git_status="clean"

    if git ls-files --modified | grep -q "$file"; then
        git_status="modified"
    fi
    if grep -q "priority: high" "$file"; then
        priority="high"
    fi

    # 输出 4 列 TSV，第 4 列为逗号分隔的标签集
    echo -e "$id\t$id\t$file\t$status,$type,$priority,$git_status"
done
```

### 步骤 2：配置样式规则

```bash
stree \
  --status-col "live=white,archived=gray,note=blue,task=yellow,high=red,bold,modified=green" \
  ...
```

**效果**：
- 一个 `live` 的 `note`，优先级 `normal`，Git 状态 `clean` → 蓝色（`note=blue`）
- 一个 `archived` 的 `task`，优先级 `high`，Git 状态 `modified` → 绿色（`modified=green` 最后匹配，覆盖前面的颜色）+ 加粗（`high=red,bold` 中的 `bold` 累加）

**原理**：样式引擎按逗号拆分第 4 列，得到标签集 `["archived", "task", "high", "modified"]`。遍历样式规则，颜色后匹配覆盖先匹配，加粗任意命中即累加生效。

---

## 食谱 9：与外部工具深度集成（TTY 交接）

**场景**：你想在 TUI 中集成 `fzf`、`vim`、`less` 等外部全屏工具，实现无缝切换。

**核心机制**：默认模式（全屏交互）+ TTY 交接 + 事件排空

### 绑定全屏交互工具

```bash
stree \
  --bind "enter=vi {path}" \
  --bind "ctrl-f=fzf < {path}" \
  --bind "ctrl-p=less {path}" \
  ...
```

**原理**：
1. 默认模式下，`stree` 禁用 raw mode，离开 Alternate Screen，释放鼠标捕获。
2. TTY (`/dev/tty`) 被克隆并附加到子进程的 stdin/stdout/stderr。
3. 子进程退出后，`stree` 重新进入 Alternate Screen，启用 raw mode，恢复鼠标捕获。
4. **事件排空**：引擎会排空挂起的输入事件（最多 100 个），防止用户在外部工具中按的键在 `stree` 恢复后误触发快捷键。

---

## 食谱 10：浮动弹窗与 Z 轴图层控制

**场景**：你想按快捷键弹出一个帮助菜单、或者一个命令面板，它浮在所有窗口之上；按 `Esc` 后消失。

**核心机制**：多图层 `--layout` + `|` 初始隐藏 + IPC `@show`/`@hide` 控制

### 步骤 1：声明隐藏的浮动图层

```bash
stree \
  --layout "vertical(area:Main, area:Preview)" \
  --layout "|@(20,5) area(60,15)[box]:HelpMenu" \ # | 前缀表示初始隐藏，@(20,5) 定义浮动坐标
  --view "HelpMenu:cat HELP.md" \
  ...
```

### 步骤 2：绑定 IPC 控制显隐

```bash
# 显示弹窗
--bind "f1=@sh -c 'echo show | stree update @layer HelpMenu'" \
# 隐藏弹窗 (在 stree 内部按 Esc 或 q 触发)
--bind "esc=@sh -c 'echo hide | stree update @layer HelpMenu'" \
```
*(注：实际使用时可编写专用的小脚本替代内联 `sh -c` 以提高可读性)*

**效果**：按 `F1` 瞬间浮出一个带边框的帮助菜单，遮盖在主界面之上；按 `Esc` 瞬间消失。底层的主界面状态完全保留，无需重绘。

**原理**：`stree` 支持 Z 轴多图层渲染。`|` 前缀将图层 `visible` 置为 `false`。通过特殊的 IPC target `@layer <Name>`，可以直接修改图层的 `visible` 状态。引擎会自动处理图层的遮挡和渲染顺序。

---

## 食谱 11：AST 动态手术与拖拽布局重组

**场景**：用户用鼠标拖拽窗口分割线调整大小，甚至将嵌套的子窗口“拖拽出来”变成平级窗口。

**核心机制**：运行时 AST 树旋转 + 像素反算 + Absolute 覆盖注入

### 启用拖拽支持

在布局语法中为边框添加 `[drag]` 标记：
```bash
stree \
  --layout "horizontal(area(30%)[drag]:Tree, area(70%)[drag]:Preview)" \
  ...
```

### 理解引擎魔法

当用户按住鼠标拖拽分割线时：
1. **AST 树旋转**：如果拓扑结构允许，引擎会在运行时重组布局抽象语法树（AST），将嵌套的叶子节点拉平为兄弟节点。
2. **像素反算**：用拖拽时的旧物理坐标，反算出新 AST 节点的百分比，杜绝拓扑突变带来的视觉跳跃。
3. **覆盖注入**：拖拽过程中注入 `Absolute` 像素尺寸覆盖，保证物理像素守恒，相邻窗口此消彼长，**其他无关窗口绝对不动**。
4. **重置**：通过 IPC 发送 `stree update @layout-reset` 可清空所有锁定与 AST 重组，恢复初始声明。

这是 `stree` 在 TUI 领域极其罕见的动态布局能力，达到了类似 GUI 窗口管理器的拖拽体验。

---

## 食谱 12：构建完整的终端知识库 (brain-tui 实战)

**场景**：综合运用以上所有特性，构建一个支持笔记管理、关系图谱、实时搜索的个人知识库系统。

### 主启动脚本

```bash
#!/bin/sh
# bin/brain-tui

# 启动状态栏守护进程 (食谱 3)
bin/status-daemon.sh &

# 启动 stree
exec stree \
  --relations "$HOME/brain/cache/links.tsv" \
  --layout "vertical(horizontal(area(50%):MainTree, vertical(area(30%):Links, area(70%):Preview)), area(1)[none]:Status)" \
  --layout "|@(20,5) area(60,15)[box]:HelpMenu" \
  --tree "MainTree:bin/brain-list-router" \
  --view "Links:bin/brain-context {id}" \
  --view "Preview:bin/preview-router.sh {path} {width} {height}" \
  --view "HelpMenu:cat HELP.md" \
  --input "SearchInput:/" \
  --input "RenameInput:rename:@bin/brain-rename {path} {input}" \
  --statusbar "Status:Initializing..." \
  --bind "enter=vi {path}" \
  --bind "r=activate_input RenameInput" \
  --bind "space=__MARK__" \
  --bind "ctrl-l=__TOGGLE_LAYER__ HelpMenu" \
  --bind "ctrl-d=@bin/brain-archive {paths}" \
  --bind "ctrl-t=@bin/brain-switch-view" \
  --status-col "live=white,archived=gray,note=blue,task=yellow,inbox=cyan,idea=magenta,^fail.*=red,bold,__marked__=#c93b3b"
```

**架构解析**：
- **左侧 (MainTree)**：笔记树形列表，支持 `Space` 批量标记 (食谱 6)、`/` 实时扁平搜索 (食谱 7)。
- **右上 (Links)**：根据当前选中 `{id}` 异步显示入链出链图谱 (食谱 5 焦点联动)。
- **右下 (Preview)**：智能预览路由，支持 Markdown/PDF/图片自适应 (食谱 4 零 I/O 滚动)。
- **底部 (Status)**：实时刷新 Git 与 TODO 状态的混合状态栏 (食谱 3)。
- **浮动层 (HelpMenu)**：按 `Ctrl-L` 浮出的帮助菜单 (食谱 10 Z 轴图层)。
- **`r` 键 (RenameInput)**：利用统一执行上下文，安全地将带有空格的新标题通过 `{input}` 传给重命名脚本。
- `Ctrl-T`：零闪烁视图切换 (食谱 1)。
- `Ctrl-D`：静默批量归档 (食谱 2 后台异步)。
