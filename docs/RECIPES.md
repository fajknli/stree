# stree 高级实战食谱 (RECIPES)

> 本文档展示 `stree` 的极限能力。每一个食谱都来自真实的生产场景，证明 `stree` 绝不仅仅是一个"看目录的工具"，而是一个可以构建复杂终端应用的通用底座。

---

## 食谱 1：零闪烁多视图切换

**场景**：你的 TUI 需要支持多个视图（如 `atoms` 全局视图、`inbox` 待处理视图、`timeline` 时间线视图），切换时不能有黑屏闪烁。

**核心机制**：`@` 静默模式 + IPC 定向刷新 + 后台执行

### 步骤 1：编写视图切换脚本

创建 `bin/switch-view.sh`：

```bash
#!/bin/sh
CACHE_DIR="${HOME}/.cache/myapp"
mkdir -p "$CACHE_DIR"

TARGET="$1"
CURRENT=$(cat "$CACHE_DIR/current_view" 2>/dev/null || echo "atoms")

# 如果未传参数，则在视图间循环切换
if [ -z "$TARGET" ]; then
    case "$CURRENT" in
        atoms)    TARGET="inbox" ;;
        inbox)    TARGET="timeline" ;;
        timeline) TARGET="atoms" ;;
    esac
fi

# 1. 写入状态文件
echo "$TARGET" > "$CACHE_DIR/current_view"

# 2. 关键：将 IPC 调用放入后台，让脚本瞬间退出
# 避免与 stree 主线程形成死锁
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
1. `@` 前缀让 `stree` 保留 TUI 状态，不退出 Alternate Screen。
2. 脚本瞬间退出，`stree` 主线程解除阻塞。
3. 后台的 `stree update` 在下一轮主循环中被处理，实现局部刷新。

---

## 食谱 2：后台异步任务与自动推流

**场景**：你想在 TUI 中集成 AI 总结、全文检索、或任何耗时任务，但又不想让 UI 卡住等待。

**核心机制**：`@` 静默模式 + 后台任务 + IPC 自动推送

### 步骤 1：编写异步任务脚本

创建 `bin/ai-summarize.sh`：

```bash
#!/bin/sh
TARGET_UID="$1"
[ -z "$TARGET_UID" ] && exit 1

# 立即返回，让 stree 主线程解除阻塞
# 真正的任务在后台执行
nohup sh -c "
    # 模拟耗时的 AI 调用（实际中可能是 ollama、openai-cli 等）
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

**原理**：
- `nohup ... &` 让脚本瞬间返回，`stree` 拿到退出码 0，主线程立即恢复。
- 后台任务独立运行，完成后通过 `$STREE_SOCK` 推送数据。
- `stree update Preview` 直接替换 View 的内容缓冲区，触发局部重绘。

---

## 食谱 3：状态栏的"黑客级"动态化

**场景**：你想让状态栏显示实时信息（Git 分支、系统时间、待办数量），而不仅仅是静态的节点统计。

**核心机制**：后台守护进程 + IPC 强行覆盖 StatusBar

### 步骤 1：编写状态守护脚本

创建 `bin/status-daemon.sh`：

```bash
#!/bin/sh
# 在启动 stree 时后台运行此脚本

while true; do
    # 收集各种外部状态
    GIT_BRANCH=$(git branch --show-current 2>/dev/null || echo "no-git")
    TODO_COUNT=$(grep -rl "TODO" ~/notes 2>/dev/null | wc -l)
    LOAD=$(uptime | awk -F'load average:' '{print $2}' | cut -d, -f1)
    
    # 拼装状态文本
    # 注意：这里可以直接使用 {stree_...} 占位符，引擎会再次解析
    STATUS_TEXT="[Git: $GIT_BRANCH] [TODO: $TODO_COUNT] [Load: $LOAD] | {stree_visible}/{stree_total} ({stree_marked} marked)"
    
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

**效果**：状态栏每隔 5 秒自动刷新 Git 状态、待办数量、系统负载，同时依然保留 `{stree_...}` 的引擎内置变量。

**原理**：
- `stree update Status` 直接替换 StatusBar 的渲染文本，绕过格式模板。
- 引擎在渲染时会再次解析 `{stree_...}` 占位符，实现"外部状态 + 内部状态"的混合渲染。

---

## 食谱 4：智能预览路由器

**场景**：你的 TUI 需要浏览多种文件类型（Markdown、代码、PDF、图片），但 Preview 窗口只能绑定一个命令。

**核心机制**：`{width}` 占位符 + 文件类型检测 + 自适应渲染

### 步骤 1：编写预览路由脚本

创建 `bin/preview-router.sh`：

```bash
#!/bin/sh
FILE="$1"
WIDTH="$2"
HEIGHT="$3"

[ ! -f "$FILE" ] && echo "[File not found: $FILE]" && exit 0

# 根据 MIME 类型选择渲染器
MIME=$(file -b --mime-type "$FILE")

case "$MIME" in
    text/*)
        # 文本文件：使用 bat 高亮，适配窗口宽度
        bat --terminal-width="$WIDTH" --color=always --style=plain "$FILE"
        ;;
    
    application/pdf)
        # PDF：提取文本，限制行数防止刷屏
        pdftotext -layout "$FILE" - | head -n "$HEIGHT"
        ;;
    
    image/*)
        # 图片：使用 chafa 转字符画（需安装 chafa）
        if command -v chafa >/dev/null 2>&1; then
            chafa --size="${WIDTH}x${HEIGHT}" "$FILE"
        else
            echo "[Image: $FILE]"
            echo "[Install 'chafa' for ASCII art preview]"
        fi
        ;;
    
    application/zip|application/x-tar|application/gzip)
        # 压缩包：列出内容
        case "$MIME" in
            application/zip)     unzip -l "$FILE" ;;
            application/x-tar)   tar -tvf "$FILE" ;;
            application/gzip)    tar -tzvf "$FILE" ;;
        esac
        ;;
    
    audio/*)
        # 音频：显示元信息
        echo "[Audio: $FILE]"
        ffprobe -v quiet -show_entries format=duration,bit_rate -of default=noprint_wrappers=1 "$FILE" 2>/dev/null
        ;;
    
    *)
        echo "[Unknown binary: $MIME]"
        echo "[Size: $(du -h "$FILE" | cut -f1)]"
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
- 选中 `.md` 文件 → 语法高亮渲染
- 选中 `.pdf` 文件 → 文本提取预览
- 选中 `.png` 文件 → 字符画预览（如果安装了 `chafa`）
- 选中 `.zip` 文件 → 列出压缩包内容

**原理**：
- `{width}` 和 `{height}` 在 View 上下文中展开为**该 View 窗口的内部尺寸**。
- 预览脚本根据窗口尺寸自适应输出，避免内容溢出或留白。

---

## 食谱 5：多树焦点隔离与联动

**场景**：你的 TUI 有多棵树（如"文件树"和"标签树"），希望只有当前焦点树驱动 Preview 刷新，避免切换时产生不必要的 I/O。

**核心机制**：多 `--tree` 挂载 + 焦点感知广播

### 步骤 1：配置多棵树

```bash
stree \
  --layout "horizontal(area(30%):Files, area(30%):Tags, area(40%):Preview)" \
  --tree "Files:ls-files.sh" \
  --tree "Tags:ls-tags.sh" \
  --view "Preview:cat {path}" \
  ...
```

### 步骤 2：理解引擎行为

`stree` 的 `broadcast_selection_changed` 机制：
- 当用户在 `Files` 树中移动光标时，只有 `Files` 树的选中状态变化会触发 `Preview` 刷新。
- 当用户在 `Tags` 树中移动光标时，只有 `Tags` 树的选中状态变化会触发 `Preview` 刷新。
- 两棵树完全独立，互不干扰。

**效果**：
- 在 `Files` 树中快速滚动 → Preview 显示文件内容
- 按 `Tab` 切换到 `Tags` 树 → Preview 显示标签描述（如果 Tags 树的 Path 指向标签说明文件）
- 切换过程零延迟，因为引擎只在焦点树变化时才触发 View 刷新。

**原理**：
- `Engine::broadcast_selection_changed` 检查 `is_focused_tree`，只有焦点树才驱动 View 刷新。
- 非焦点树的选中变化只更新内部状态，不触发任何外部命令。

---

## 食谱 6：批量操作与多选标记

**场景**：你想批量归档多个文件、批量删除多个节点、或批量执行某个命令。

**核心机制**：`Space` 多选 + `{ids}` / `{paths}` 占位符

### 步骤 1：编写批量操作脚本

创建 `bin/batch-archive.sh`：

```bash
#!/bin/sh
# 接收空格分隔的路径列表
for path in "$@"; do
    [ -z "$path" ] && continue
    [ ! -f "$path" ] && continue
    
    # 执行归档逻辑
    mv "$path" "${path}.archived"
    echo "Archived: $path"
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
3. 归档完成后，Tree 自动刷新，被标记的节点从列表中消失。

**原理**：
- `{paths}` 展开为所有被标记节点的 Path 列表，每个 Path 用双引号包裹，内部引号被转义。
- 脚本接收多个参数，循环处理。
- 处理完成后通过 IPC 推送新数据，触发局部刷新。

### 关键契约：`@` 模式不会自动刷新

在默认模式下，脚本退出后引擎会自动触发 `trigger_reload()`，重新执行所有 Tree 的数据源命令。
但在 `@` 静默模式下，**引擎不会自动刷新**。脚本必须显式调用 `stree update <窗口名>` 推送新数据。

这是"引擎不越界"设计哲学的直接体现：静默模式下，刷新的主动权完全由业务脚本掌控。
如果你发现 `@` 模式下 UI 没有更新，请先检查脚本末尾是否有 IPC 推送。

---

## 食谱 7：搜索过滤与高亮联动

**场景**：你想在树中实时搜索，匹配项高亮显示，非匹配项折叠，且保持树形层级结构。

**核心机制**：`/` 搜索模式 + 内存级过滤 + 祖先节点自动展开

### 步骤 1：直接使用内置搜索

```bash
stree \
  --tree "Main:my-data.sh" \
  ...
```

### 步骤 2：理解引擎行为

`stree` 的搜索机制：
1. 按 `/` 进入搜索模式，状态栏显示 `/` 提示符。
2. 输入关键词（如 `rust`），引擎在内存中遍历所有节点的 ID、Display、Path、Tags 字段。
3. 匹配的节点被加入 `matched_ids` 集合。
4. 引擎自动展开所有匹配节点的祖先节点，确保匹配项可见。
5. 非匹配节点被过滤掉，树形结构保持完整。
6. 按 `n` / `N` 跳转到下一个 / 上一个匹配项。
7. 按 `Esc` 退出搜索模式，恢复完整树形。

**效果**：
- 万级节点树中，搜索响应时间 < 10ms（纯内存遍历，无 I/O）。
- 匹配项高亮显示（黄色加粗）。
- 树形层级结构保持完整，不会变成扁平列表。

**原理**：
- `match_entities` 遍历 `Vec<Entity>`，对 4 个字段进行不区分大小写的子串匹配。
- `collect_ancestors_inner` 递归计算匹配节点的所有祖先，加入 `ancestors_of_matched`。
- `rebuild_visible_ids` 在过滤模式下，只收集匹配节点及其祖先。

---

## 食谱 8：自定义样式与多维度标签

**场景**：你想根据文件的多种属性（类型、状态、优先级、Git 状态）显示不同颜色。

**核心机制**：标签集 + 正则匹配 + 后规则覆盖

### 步骤 1：在数据源中输出多维度标签

```bash
#!/bin/sh
# 生成数据时，第 4 列输出逗号分隔的标签集
for file in *.md; do
    id=$(basename "$file" .md)
    type="note"
    status="live"
    priority="normal"
    git_status="clean"
    
    # 检测 Git 状态
    if git ls-files --modified | grep -q "$file"; then
        git_status="modified"
    fi
    
    # 检测优先级（假设 frontmatter 中有 priority 字段）
    if grep -q "priority: high" "$file"; then
        priority="high"
    fi
    
    # 输出 4 列 TSV，第 4 列为标签集
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

**原理**：
- 样式引擎按逗号拆分第 4 列，得到标签集 `["archived", "task", "high", "modified"]`。
- 遍历样式规则，检查每个标签是否匹配。
- 颜色：后匹配的规则覆盖先匹配的（`modified=green` 覆盖 `high=red`）。
- 加粗：任意规则命中 `bold` 即生效（累加逻辑）。

---

## 食谱 9：与外部工具深度集成

**场景**：你想在 TUI 中集成 `fzf`、`vim`、`less` 等外部工具，实现无缝切换。

**核心机制**：默认模式（全屏交互）+ TTY 交接

### 步骤 1：绑定全屏交互工具

```bash
stree \
  --bind "enter=vi {path}" \
  --bind "ctrl-f=fzf < {path}" \
  --bind "ctrl-p=less {path}" \
  ...
```

**效果**：
- 按 `Enter` → `stree` 隐藏，`vi` 全屏打开文件。退出 `vi` 后，`stree` 恢复。
- 按 `Ctrl-F` → `stree` 隐藏，`fzf` 在文件内容中搜索。退出后，`stree` 恢复。
- 按 `Ctrl-P` → `stree` 隐藏，`less` 分页查看文件。退出后，`stree` 恢复。

**原理**：
- 默认模式下，`stree` 禁用 raw mode，离开 Alternate Screen，释放鼠标捕获。
- TTY (`/dev/tty`) 被克隆并附加到子进程的 stdin/stdout/stderr。
- 子进程退出后，`stree` 重新进入 Alternate Screen，启用 raw mode，恢复鼠标捕获。
- 排空挂起的输入事件（最多 100 个），防止用户在外部工具中按的键在 `stree` 恢复后误触发。

---

## 食谱 10：构建完整的终端知识库（brain-tui 实战）

**场景**：你想用 `stree` 构建一个完整的个人知识库系统，支持笔记管理、标签分类、关系图谱、全文搜索。

**核心机制**：综合运用以上所有食谱

### 架构概览

```
brain-tui/
├── bin/
│   ├── brain-tui          # 主启动脚本
│   ├── brain-list         # 数据源：生成 4 列 TSV
│   ├── brain-context      # 预览脚本：显示入链出链
│   ├── brain-switch-view  # 视图切换（食谱 1）
│   ├── brain-ai-summarize # AI 总结（食谱 2）
│   ├── status-daemon      # 状态栏守护（食谱 3）
│   └── preview-router     # 智能预览（食谱 4）
├── atoms/                 # 笔记文件
├── inbox/                 # 待处理碎片
└── cache/                 # 索引缓存
```

### 主启动脚本

```bash
#!/bin/sh
# bin/brain-tui

# 启动状态守护进程
bin/status-daemon.sh &

# 启动 stree
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

**效果**：
- 左侧：笔记树形列表，支持搜索、多选、展开折叠
- 右上：当前笔记的入链出链图谱
- 右下：智能预览（Markdown 高亮、PDF 文本、图片字符画）
- 底部：实时状态栏（Git 分支、待办数量、系统负载）
- `Ctrl-T`：零闪烁切换 atoms/inbox/timeline 视图
- `Ctrl-L`：后台 AI 总结，自动推送到 Preview
- `Ctrl-D`：批量归档选中的笔记
- `Ctrl-O`：批量建立链接关系

**这就是 `stree` 的极限**：它不是一个工具，它是一个**构建工具的元工具**。

---

## 结语

以上 10 个食谱展示了 `stree` 的核心能力：
- **零延迟交互**（食谱 1、2）
- **动态状态管理**（食谱 3）
- **智能内容适配**（食谱 4）
- **多组件联动**（食谱 5）
- **批量操作**（食谱 6）
- **实时搜索**（食谱 7）
- **多维度样式**（食谱 8）
- **外部工具集成**（食谱 9）
- **完整应用构建**（食谱 10）

`stree` 的设计哲学是：**提供机制，不提供策略**。它给你最强大的原语，让你自由组合出任何你想要的终端应用。

现在，轮到你了。去创造属于你自己的终端宇宙吧。
