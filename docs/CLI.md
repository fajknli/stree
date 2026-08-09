# stree 引擎契约与 CLI 参考手册

本文档是 `stree` 引擎与外部业务层（胶水脚本）之间的严格契约。`stree` 采用 **混合保留模式**：内部通过 Rust 严格管理高频 UI 交互状态（焦点、滚动、展开、标记）以实现零延迟响应，外部通过 Unix 管道、IPC 与声明式配置驱动一切业务逻辑。

**核心哲学**：引擎持有 UI 交互状态，但不持有任何业务状态。业务逻辑 100% 由外部脚本处理。

---

## 1. 数据协议：四列 TSV 契约

### 1.1 实体数据流

引擎从标准输入或 IPC 读取制表符分隔值（TSV）流。每一非空行必须严格包含 4 个字段：

```text
ID \t Display \t Path \t Tags
```

| 字段 | 约束条件 | 引擎行为 |
| :--- | :--- | :--- |
| **ID** | 非空字符串。流内全局唯一。 | 树形关联、选中记忆、IPC 定位的主键。重复 ID 去重（保留最后一次出现）。O(1) HashMap 去重。 |
| **Display** | 任意 UTF-8 字符串。 | 在树形列表中逐字渲染。支持 ANSI SGR 颜色码透传（含 256 色与 RGB）。引擎不解析其业务语义。 |
| **Path** | 任意 UTF-8 字符串。 | 不透明业务负载。通过 `{path}` 原样传递。展开为 `{paths}` 时，内部双引号自动转义为 `\"`。 |
| **Tags** | 逗号分隔的标签集（如 `live,note,inbox`）。 | 按 `,` 拆分并 trim。**仅供样式引擎匹配，绝对不参与搜索。** |

#### 防御性解析规则
1. **强制 Trim**：剥离每个字段的首尾空白符和 `\r`。
2. **空 ID 拒绝**：ID 为空的行静默跳过（警告输出至 stderr）。
3. **版本头（可选）**：若首行首字段以 `VERSION:` 开头，解析版本号，从第二字段开始处理。
4. **字段不足**：少于 4 列的行静默跳过。

### 1.2 关联表 (`--relations <file>`)
2 列 TSV 文件，定义父子关系：`Parent_ID \t Child_ID`。
- **拓扑构建**：未在任何关系中作为子节点出现的 ID 自动成为根节点。根节点按字母序排列。
- **环检测**：递归构建时维护祖先足迹集合（`visited`），检测到循环引用时截断渲染并输出警告。
- **重载规则**：IPC 更新或 SIGUSR1 重载时，若 Tree 配置了 `--relations` 路径，引擎重新读取该磁盘文件；否则复用内存中的关系拓扑。

---

## 2. 组件声明与前缀机制

### 2.1 Tree 组件 (`--tree`)
```bash
--tree "[click:][focus:][nomark:]Name:SourceCmd"
```
- **前缀组合**：`click:`, `focus:`, `nomark:` 可无序、任意组合（循环 strip 解析）。
- **`click:`**：单击节点即触发 `click` 信号（默认需双击或 Enter 触发 `confirm`）。
- **`focus:`**：焦点切换到该 Tree 时触发 `focus` 信号。
- **`nomark:`**：禁用标记功能（Space 键和鼠标右键拖拽均无效）。**未声明此前缀的 Tree 默认可标记。**
- **`SourceCmd`**：启动及重载时执行的命令，stdout 必须为合法 TSV 流。

### 2.2 View 组件 (`--view`)
```bash
--view "Name:RenderCmdTemplate"
```
- **异步执行**：Tree 选中项改变时，引擎展开模板并在后台线程异步执行，通过 `mpsc::channel` 回传结果。
- **防抖缓存**：内置 `cached_entity_id`。快速 `j/k` 滚动时，若选中 ID 与缓存一致且缓冲区非空，零 I/O 零 fork。
- **加载挂起**：若 View 正在异步加载中（`is_loading == true`），新的选中变化被挂起到 `pending_view_reload`，待加载完成后重新触发。
- **竞态保护**：异步结果返回时，校验 `cached_entity_id == target_id`，过期结果丢弃。
- **尺寸占位符**：`{width}` 和 `{height}` 展开为 View 窗口的内部内容区尺寸（减去边框开销）。

### 2.3 StatusBar 组件 (`--statusbar`)
```bash
--statusbar "Name:FormatTemplate"
```
**全息探测器**：引擎在渲染状态栏时，通过 `collect_status_metrics` 将内部 30 多种状态收集为平坦的 `HashMap`。支持 `{stree_id}`, `{stree_visible}`, `{stree_loading}` 等占位符自动替换。
当有 Input 组件激活时，StatusBar 物理区域被 Input 劫持；全局错误提示以红底白字覆盖 StatusBar。

### 2.4 Input 组件 (`--input`)
```bash
--input "Name:Prefix:[@]OnSubmitTemplate"
```
- **`@` 静默前缀**：若模板以 `@` 开头，提交时静默执行（不挂起 TUI）。
- **`/` 特殊行为**：若 Prefix 为 `/`，引擎拦截输入执行内部实时模糊搜索，不执行 `OnSubmitTemplate`。
- **退格退出**：若输入框内容已为空，再次按下 `Backspace` 等同于按 `Esc` 退出输入模式。

---

## 3. 布局引擎：正交 Flexbox 与运行时拓扑重组

### 3.1 节点语法
```text
area(size)[border,drag]:Name
```
- **`size`** 支持的格式：
  - `area(50%)`：主轴方向占比，万分比精度，支持两位小数。
  - `area(3)`：主轴方向固定字符数。
  - `area(auto:5)`：根据内容行数自动计算高度，参数为 fallback 默认行数。仅推荐在垂直容器中使用。
  - `area(40,15)` / `area(95%,95%)`：二维绝对/百分比，仅用于浮动层。
  - `area:Main`：均分剩余空间。
- **`border`**：`box`（默认）、`line`、`none`。
- **`drag`**：**绝对防线标志**。允许该边框被鼠标拖拽调整大小。未声明此标志的边框严禁被拉伸。

### 3.2 多图层与 Z 轴
多次声明 `--layout` 创建多图层，声明顺序即 Z 轴渲染顺序。
- **全屏层**：无前缀，铺满终端。
- **浮动层**：`@(x,y)` 前缀，屏幕绝对坐标偏移。支持像素或百分比。
- **初始隐藏**：`|` 前缀声明的图层初始不可见。

### 3.3 Auto 预处理降级与状态冻结
对于 `area(auto)` 节点，引擎采用“预处理降级”策略，不修改底层 Flexbox 算法：
1. **预处理降级**：每帧渲染前，扫描 AST 中的 `Auto` 节点。对于 `Tree` 取 `visible_ids.len()`，对于 `View` 取 `content_buffer.lines().count()`，将其转换为临时的 `Absolute` 覆盖存入 `auto_overrides` 字典。
2. **字典优先级**：底层 `calc_window_rects` 合并字典时，优先级为 `拖拽物理锁` > `Auto预处理` > `AST声明`。
3. **加载期状态冻结**：如果 View 正在异步加载（`is_loading = true`），跳过重算，直接继承上一帧的高度。继承时必须强制进行 `clamp(term_height)` 限制，防止终端缩小时高度溢出导致布局崩溃。
4. **拖拽固化**：一旦用户拖拽了 `Auto` 窗口的边缘，AST 重组会直接将其固化为静态的 `Absolute`，彻底退出动态计算。需通过 `@layout-reset` 恢复。

### 3.4 鼠标拖拽与运行时拓扑重组
- **Flexbox 边缘拖拽**：拖拽时通过“物理覆盖注入”冻结像素，松手时用物理真相反算 AST 百分比，实现丝滑的拓扑突变。**仅双方均声明 `[drag]` 时生效。**
- **浮动窗口边缘拖拽**：支持四边拉伸，拖拽时递归修改 Window 节点的 `Absolute2D` 尺寸和图层锚点坐标。**仅自身声明 `[drag]` 时生效。**
- **布局重置**：`echo "" | stree update @layout-reset` 清空拖拽覆盖，并从蓝图快照完全重建 AST。

---

## 4. 渲染管线：双缓冲 Diff 与 ANSI 透传

- **双缓冲架构**：维护 `CURR_BUFFER` 与 `PREV_BUFFER`，逐 Cell Diff 后仅输出变化字符。全屏重绘触发条件：首帧、终端尺寸变化、图层显隐切换。
- **浮动层不透明**：Z-index > 0 的浮动层在渲染前会强制用空格擦除背景（`clear_bg = true`），防止底层内容透出。
- **宽字符防御**：动态检测宽字符右半部分，跳过打印但同步 SGR 状态，防止状态机脱节导致残影。
- **ANSI SGR 解析器**：零分配解析器，全面支持 256 色、RGB、粗体等 SGR 参数，截断逻辑确保不会劈裂 ANSI 序列。

---

## 5. 交互契约

### 5.1 搜索防幽灵契约
- **严格契约**：引擎仅搜索内容层（`ID`, `Display`, `Path`），绝对不搜索元数据层（`Tags`）。
- **安全剥离扩展名**：使用 Rust 标准库 `Path::with_extension("")` 移除文件扩展名，完美处理多级目录和带点的文件名。
- **高亮**：匹配子串以 TrueColor 红色高亮，使用 `\x1b[39m` 恢复前景色，不破坏选中行背景色。

### 5.2 信号模型与内部指令直连
事件驱动架构的核心。所有状态变更均可绑定信号。
- **支持的信号**：`select` (防抖200ms)、`click`、`focus`、`blur`、`confirm`、`load`。
- **内部指令直连**：若信号绑定的命令解析为内部 UI 指令（如 `__SHOW_LAYER__`、`__HIDE_LAYOUT__`、`__TOGGLE_LAYOUT__`），引擎在内存中同步执行状态变更，零延迟、零进程开销。
- **外部降级**：若非内部指令，降级为外部静默 Shell 执行。
- **应用场景**：实现零延迟的悬停弹窗与移开自动消失：
  ```bash
  --bind "MainTree:focus=__SHOW_LAYOUT__ HelpMenu"
  --bind "MainTree:blur=__HIDE_LAYOUT__ HelpMenu"
  ```

### 5.3 焦点系统与异步挂起
- **`set_focus` 统一入口**：自动维护 `focus_history` 栈，绝不允许聚焦到 StatusBar。
- **异步刷新防挂起**：`set_focus` 修改焦点后，严禁同步触发视图刷新（避免 `&mut self` 借用冲突），而是将刷新请求挂起到 `pending_selection_changed` 或 `pending_blur` 队列，由主循环在渲染前统一消费。
- **方向切换**：基于物理矩形计算空间距离，按欧几里得距离排序选择最近候选。

### 5.4 鼠标行为
| 操作 | 行为 |
| :--- | :--- |
| 悬停（`Moved`） | 切换焦点到悬停窗口（不进入点击逻辑），**StatusBar 免疫** |
| 左键单击 | 选中节点 / 切换焦点。若 Tree 开启 `click:` 前缀则触发 `click` 信号 |
| 左键双击（< 300ms） | 展开/折叠 + 触发 `confirm` 信号 |
| 右键按下 | 切换标记，进入右键拖拽模态 |
| 右键拖拽 | 框选标记/取消标记（`is_marking` 决定方向） |
| 滚轮 | 批量移动 `scroll_step` 行（默认 3） |
| 边框拖拽 | Flexbox 边缘调整 / 浮动窗口四边拉伸（需 `[drag]` 标志） |

### 5.5 默认快捷键
| 按键 | 内部命令 | 行为 |
| :--- | :--- | :--- |
| `j` / `Down` | `__DOWN__` | 向下移动 |
| `k` / `Up` | `__UP__` | 向上移动 |
| `h` / `Left` | `__EXPAND__` | 展开/折叠切换 |
| `l` / `Right` | `__EXPAND__` | 展开/折叠切换 |
| `Enter` | `__ENTER__` | 展开/折叠 + 触发 `confirm` 信号 |
| `Space` | `__MARK__` | 切换标记（标记后自动下移一行） |
| `g` | `__TOP__` | 跳转顶部 |
| `G` | `__BOTTOM__` | 跳转底部 |
| `Tab` | `__CYCLE_LAYER__` | Z 轴图层焦点切换（跨图层跳转） |
| `Ctrl-H` | `__FOCUS_LEFT__` | 焦点向左移动（空间距离排序） |
| `Ctrl-L` | `__FOCUS_RIGHT__` | 焦点向右移动 |
| `Ctrl-K` | `__FOCUS_UP__` | 焦点向上移动 |
| `Ctrl-J` | `__FOCUS_DOWN__` | 焦点向下移动 |
| `H` | `__SCROLL_LEFT__` | 水平左滚 5 字符 |
| `L` | `__SCROLL_RIGHT__` | 水平右滚 5 字符 |
| `/` | `__ACTIVATE_SEARCH__` | 激活搜索输入框 |
| `:` | `__ACTIVATE_CMD__` | 激活命令输入框 |
| `Esc` | `__ESC__` | 取消输入 / 退出搜索 / 取消拖拽 |
| `q` | `__EXIT__` | 退出引擎 |

### 5.6 终端尺寸变化
终端尺寸改变时：1. 清空 `window_rect_overrides`（解除物理锁）；2. 清空 `prev_rects`（强制全屏重绘）；3. 标脏所有组件。

---

## 6. 执行模型：统一上下文与双轨制

### 6.1 统一执行上下文
| 占位符 | 展开规则 |
| :--- | :--- |
| `{id}` / `{path}` / `{display}` / `{tags}` | 选中节点的对应字段。无选中则空字符串。 |
| `{ids}` | 所有被标记节点 ID（空格分隔）。无标记则回退到 `{id}`。 |
| `{paths}` | 所有被标记节点 Path，双引号包裹，内部引号转义。 |
| `{input}` | Input 提交时用户输入的原始字符串。 |
| `{window}` | 当前焦点窗口名称。 |
| `{width}` / `{height}` | `--view` 中：View 内部内容区尺寸。`--bind` 中：终端总尺寸。 |
| `{event}` | 触发绑定的信号名称。 |

### 6.2 双轨制执行模式
| 模式 | 触发 | 行为 | 失败处理 |
| :--- | :--- | :--- | :--- |
| **默认模式** | `--bind "enter=vi {path}"` | 挂起 TUI → 释放 TTY → 子进程继承终端 → 恢复 TUI → 触发 `refresh_engine_state` | `last_error = "Command exited..."` |
| **静默模式（`@`）** | `--bind "ctrl-t=@script.sh"` | 保留 TUI。stdin/stdout/stderr 重定向到 `/dev/null`。 | **成功**：触发刷新。**失败**：引擎不报错，错误 100% 交给脚本通过 IPC 推送。 |

**`refresh_engine_state` 单点回退策略**：执行后清空所有 View 缓存，优先使用当前聚焦的 Tree 提供上下文刷新；若焦点不在 Tree，则回退到主树 (`main_tree_name`)。避免多树并发广播覆盖同一 View 产生的竞态条件。

---

## 7. 样式与主题引擎

支持 TrueColor 十六进制（`#a9b5d5`）和 3 位简写（`#fff` → `#ffffff`）。

### 7.1 数据状态颜色 (`--status-col`)
```bash
--status-col "live=white,archived=gray,^fail.*=red,bold,__marked__=#c93b3b"
```
- **匹配语义**：按逗号拆分第 4 列标签集，每条规则检查是否**有任何一个标签**匹配。
- **优先级**：颜色覆盖（后匹配覆盖先匹配），粗体累加（任意规则命中 `bold` 即生效）。
- **模式**：精确字符串（`live`）或正则（`^fail.*`）。含 `^`, `*`, `.`, `+`, `?`, `[` 的 pattern 自动编译为正则，失败则降级精确匹配。
- **内置默认规则**：`__marked__` → `red,bold`（在用户规则之前注入）。
- **保留标签**：`__selected__`（选中行）、`__marked__`（标记行）。

### 7.2 UI 框架颜色 (`--ui-colors`)
控制非数据元素的视觉呈现，格式 `key=value` 逗号分隔。

| 键 | 默认值 | 用途 |
| :--- | :--- | :--- |
| `border_focused` | `#a9b5d5` | 焦点窗口边框色 |
| `border_unfocused` | `#565d7e` | 非焦点窗口边框色 |
| `view_focused` | `#a9b5d5` | 焦点窗口默认前景色 |
| `view_unfocused` | `#565d7e` | 非焦点窗口默认前景色 |
| `statusbar_fg` | `#d4dcf2` | 状态栏前景色 |
| `input_prefix` | `#c93b3b` | 输入框前缀色 |
| `input_buffer` | `#a9b5d5` | 输入框内容色 |
| `selected_bg` | `#242838` | 选中行背景色 |
| `error_fg` | `#ffffff` | 错误提示前景色 |
| `error_bg` | `#c93b3b` | 错误提示背景色 |
| `empty_data_fg` | `#565d7e` | 空数据提示色 |

---

## 8. IPC 协议与局部刷新

Socket 路径通过 `$STREE_SOCK` 暴露。帧结构（大端序）：
`[4B target_len (u32)][8B data_len (u64)][target (UTF-8)][data (UTF-8)]` (硬限制：`target_len ≤ 128`，`data_len ≤ 512KB`)。

```bash
./generate-data.sh | stree update MainTree   # 重建 Tree 内存结构 + 广播选中 + 触发 load 信号
echo "Loading..." | stree update Preview     # 直接替换 View 缓冲区
echo "[ERR] ..." | stree update Status       # StatusBar 临时消息推送
echo "" | stree update @layout-reset         # 从蓝图重置 AST
```
- **Tree 更新**：解析 TSV → 重建内存树 → 保留 `expanded_ids` → 恢复选中 → 广播。
- **View 更新**：替换 `content_buffer`，清空 `cached_entity_id`，滚动归零。
- **StatusBar 更新**：推送的消息作为临时消息显示，3 秒后自动过期恢复原模板。

---

## 9. CLI 参数完整参考

```bash
stree [SUBCOMMAND] [OPTIONS]
```

### 子命令
| 子命令 | 用途 |
| :--- | :--- |
| `update <target>` | IPC 客户端模式，从 stdin 读取数据推送到指定组件 |

### 全局选项
| 参数 | 类型 | 默认值 | 说明 |
| :--- | :--- | :--- | :--- |
| `--relations <file>` | String | 无 | 关联表文件路径（2 列 TSV） |
| `--layout <expr>` | 可重复 | `area:Main` | 布局声明（顺序即 Z 轴） |
| `--tree <cfg>` | 可重复 | 无 | Tree 组件声明 |
| `--view <cfg>` | 可重复 | 无 | View 组件声明 |
| `--statusbar <cfg>` | 可重复 | 无 | StatusBar 组件声明 |
| `--input <cfg>` | 可重复 | 无 | Input 组件声明 |
| `--bind <key=cmd>` | 可重复 | 内置默认 | 按键/信号绑定 |
| `--border-chars <name:chars>` | 可重复 | 无 | 自定义边框字符（6 字符：┌┐└┘│─） |
| `--status-col <rules>` | String | `""` | 数据状态颜色规则 |
| `--ui-colors <kv>` | String | `""` | UI 框架颜色主题 |
| `--select <id>` | String | 无 | 启动时预选指定 ID |
| `--no-mouse` | Flag | false | 禁用鼠标 |
| `--scroll-step <n>` | u8 | `3` | 滚轮每次移动行数 |
| `--max-lines <n>` | usize | `500` | View 输出最大行数（超出截断） |

---

## 10. 信号、退出与环境变量

### 10.1 信号处理
| 信号 | 行为 |
| :--- | :--- |
| `SIGUSR1` | 触发全局重载：重新执行每个 `--tree` 的数据源命令，重新读取 `--relations` |
| `SIGINT` | 优雅关闭。清理 socket，恢复终端状态 |

### 10.2 退出行为
优雅退出时，引擎将**最终选中节点的 ID** 输出到 stdout：
```bash
selected=$(my-data.sh | stree --tree "Main:my-data.sh")
echo "User selected: $selected"
```

### 10.3 Panic 安全
引擎注册自定义 panic hook：
1. 禁用 Raw Mode。 2. 禁用鼠标捕获。 3. 离开 Alternate Screen。 4. 显示光标。 5. 清理 `$STREE_SOCK`。 6. 调用原始 hook。

### 10.4 环境变量
| 变量 | 作用域 | 描述 |
| :--- | :--- | :--- |
| `$STREE_SOCK` | 引擎 → 子进程 | Unix Domain Socket 路径 |
| `$FORCE_COLOR` | 子进程 | 设为 `1`，强制子进程输出颜色 |
| `$CLICOLOR_FORCE`| 子进程 | 设为 `1`，兼容更多工具 |
| `$TERM` | 子进程 | 设为 `xterm-256color` |

### 10.5 输出截断保护
`execute_command_args`（View 预览）内置三重保护：
| 限制 | 值 | 行为 |
| :--- | :--- | :--- |
| 最大行数 | `--max-lines`（默认 500） | 超出后 `kill` 子进程，追加截断提示 |
| 单行最大字符 | 500 | 超长行截断并追加 `...` |
| 总字节上限 | 1 MB | 超出后 `kill` 子进程 |

---

## 11. 架构红线

以下原则是 `stree` 内核的宪法。任何违反均视为架构退化。

1. **绝不硬编码业务语义**：引擎不理解"文件"、"笔记"、"归档"。只理解 ID、Display、Path、Tags。
2. **绝不持久化状态**：重启后展开/选中/滚动全部重置。持久化是业务层的责任。
3. **提供机制，不提供策略**：引擎提供标签匹配机制但不定义标签含义；提供 IPC 通道但不定义推送格式；提供占位符展开但不定义命令逻辑。
4. **同步主循环纯洁性**：主循环只消耗在纯计算上。可能阻塞的物理操作（文件 I/O、进程等待）剥离到子进程或后台线程。
5. **UI 交互状态与业务状态分离**：引擎持有焦点、滚动、展开、标记等 UI 交互状态（延迟敏感，不能走 IPC）。业务状态 100% 由外部脚本管理。
6. **动态语义降级，底层算法纯粹**：引擎底层仅处理有限的基础尺寸类型（如 `Absolute`, `Percent`）。任何高级动态语义（如自适应高度 `Auto`），必须在渲染前通过预计算机制降级为静态类型（注入 `auto_overrides`）。底层 Flexbox 数学算法绝不为特定业务特性妥协或打补丁。
7. **状态变更解耦，异步挂起队列优先**：在事件处理或焦点切换中，严禁同步触发跨组件的级联广播（极易引发 Rust 借用检查器冲突或死锁）。所有状态变更必须推入挂起队列（如 `pending_selection_changed`），由主循环在安全的时间点统一执行 `flush`。
8. **极限防御与安全降级**：终端环境存在高度不确定性（尺寸突变、宽字符越界、异步加载延迟）。所有物理坐标计算必须使用饱和运算（`saturating_sub/add`）防止下溢 Panic；所有继承自历史帧的状态（如防闪烁的高度冻结）必须经过 `clamp` 边界重校验；遭遇不可恢复错误时，通过 panic hook 安全退出并恢复终端状态，严禁将破坏状态遗留至父 Shell。
