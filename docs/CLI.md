# stree 引擎契约与 CLI 参考手册 (V3.3)

本文档是 `stree` 引擎与外部业务层（胶水脚本）之间的严格契约。`stree` 采用 **混合保留模式**：内部通过 Rust 严格管理高频 UI 交互状态（焦点、滚动、展开、标记）以实现零延迟响应，外部通过 Unix 管道、IPC 与声明式配置驱动一切业务逻辑。

**核心哲学**：引擎持有 UI 交互状态，但不持有任何业务状态。业务逻辑 100% 由外部脚本处理。

---

## 1. 数据协议：四列 TSV 契约

### 1.1 实体数据流 (stdin / IPC)

引擎从标准输入或 IPC 读取制表符分隔值（TSV）流。每一非空行必须严格包含 4 个字段：

```text
ID \t Display \t Path \t Tags
```

#### 字段规范

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
- **去重**：`build_child_index` 中同一父节点的子 ID 不重复添加。
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

- **异步执行**：Tree 选中项改变时，引擎展开模板并在 **`std::thread::spawn` 后台线程异步执行**，通过 `mpsc::channel` 回传结果，避免预览大文件阻塞 UI。
- **防抖缓存**：内置 `cached_entity_id`。快速 `j/k` 滚动时，若选中 ID 与缓存一致且缓冲区非空，**零 I/O 零 fork**。
- **静态内容跳过**：若命令模板不包含 `{id}`/`{path}`/`{display}`/`{tags}`/`{ids}`/`{paths}` 任何占位符，且缓冲区已有内容，则不参与选中变化的自动重载。
- **加载挂起**：若 View 正在异步加载中（`is_loading == true`），新的选中变化被挂起到 `pending_view_reload`，待加载完成后重新触发。
- **竞态保护**：异步结果返回时，校验 `cached_entity_id == target_id`，过期结果丢弃。
- **尺寸占位符**：`{width}` 和 `{height}` 展开为 View 窗口的**内部内容区尺寸**（减去边框开销），常用于 `bat --terminal-width={width}`。

### 2.3 StatusBar 组件 (`--statusbar`)

```bash
--statusbar "Name:FormatTemplate"
```

**全息探测器**：引擎在渲染状态栏时，会通过 `collect_status_metrics` 将内部 30 多种状态收集为平坦的 `HashMap`。UI 层仅做无脑的字符串替换。支持以下占位符自动替换：

| 类别 | 占位符 | 展开值 |
| :--- | :--- | :--- |
| **VST 数据** | `{stree_id}` / `{stree_display}` / `{stree_path}` / `{stree_tags}` | 当前选中节点的 4 列数据 |
| **树拓扑** | `{stree_visible}` / `{stree_total}` | 可见节点数 / 总节点数 |
| | `{stree_marked}` / `{stree_expanded}` | 标记数 / 展开的节点数 |
| | `{stree_roots}` / `{stree_relations}` | 根节点数 / 图谱连线总数 |
| | `{stree_idx}` / `{stree_depth}` | 当前选中行号 (从1起) / 当前节点深度 |
| | `{stree_search}` | 当前激活的搜索词（无则空） |
| | `{stree_scroll_v}` / `{stree_scroll_h}` | 树视图垂直/水平滚动偏移 |
| **预览/IO** | `{stree_loading}` | 正在后台加载的 View 数量 |
| | `{stree_view_v}` / `{stree_view_h}` | 聚焦 View 的滚动位置 |
| | `{stree_buffer_kb}` | 聚焦 View 的缓冲区大小 (KB) |
| **布局渲染** | `{stree_ast}` | 当前主图层的 AST 拓扑简写字符串 |
| | `{stree_layers}` / `{stree_windows}` | 可见图层数 / 窗口总数 |
| | `{stree_dirty}` | 当前帧被标脏的组件数 |
| | `{stree_edges}` / `{stree_intersections}` | 可拖拽边缘数 / 交叉点数 |
| | `{stree_cols}` / `{stree_rows}` | 当前终端尺寸 |
| | `{stree_prev_cols}` / `{stree_prev_rows}` | 上一帧终端尺寸 |
| **交互系统** | `{stree_focus}` | 当前焦点窗口名 |
| | `{stree_drag}` | 拖拽状态 (显示 `DRAG` 或空) |
| | `{stree_marking}` | 鼠标右键批量标记状态 (显示 `MARK` 或空) |
| | `{stree_input}` | 输入模式状态 (显示 `INPUT` 或空) |
| | `{stree_history}` | 焦点历史栈深度 |
| **系统** | `{stree_ipc_sock}` | 当前 IPC Socket 路径 |
| | `{stree_pid}` | 引擎进程 PID |

**渲染优先级**：当有 Input 组件激活时，StatusBar 的物理区域被 Input 劫持（渲染输入框）。全局错误提示（`last_error`）以红底白字覆盖 StatusBar 内容。

### 2.4 Input 组件 (`--input`)

```bash
--input "Name:Prefix:[@]OnSubmitTemplate"
```

- **`Prefix`**：激活时显示的前缀字符（如 `/` 或 `:`）。
- **`@` 静默前缀**：若模板以 `@` 开头，提交时静默执行（`execute_command_silent`，不挂起 TUI）。
- **`/` 特殊行为**：若 Prefix 为 `/`，引擎拦截输入执行**内部实时模糊搜索**（fzf 式逐键过滤），不执行 `OnSubmitTemplate`。
- **`{input}` 占位符**：提交时，用户输入的原始字符串通过 `{input}` 注入命令模板。
- **编辑快捷键**：`Ctrl-U` 清空、`Ctrl-A` 行首、`Ctrl-E` 行尾、`Home`/`End`、`Left`/`Right`。
- **退格退出**：若输入框内容已为空，再次按下 `Backspace` 等同于按 `Esc`，直接退出输入模式。

---

## 3. 布局引擎：正交 Flexbox 与 AST 动态手术

### 3.1 节点语法

```text
area(size)[border,drag]:Name
```

- **`size`** 支持的格式：

| 格式 | 示例 | 含义 |
| :--- | :--- | :--- |
| 百分比 | `area(50%)` | 主轴方向占比，万分比精度（0–10000），支持两位小数（`33.33%` → 3333） |
| 绝对尺寸 | `area(3)` | 主轴方向固定字符数 |
| 二维绝对 | `area(40,15)` | 宽 × 高固定字符数（`Absolute2D`），仅用于浮动层 |
| 二维百分比 | `area(95%,95%)` | 宽 × 高百分比（`Percent2D`），仅用于浮动层 |
| 自适应高度 | `area(auto)` 或 `area(auto:5)` | 根据内容行数自动计算高度，参数为 fallback 默认行数（默认 1）。**仅推荐在垂直容器中使用**。 |
| 留空 | `area:Main` | 均分剩余空间 |

- **`border`**：`box`（默认，四边框）、`line`（仅顶线）、`none`（无边框）。
- **`drag`**：允许该边框被鼠标拖拽调整大小。

### 3.2 容器语法

```text
horizontal(size, child1, child2, ...)
vertical(size, child1, child2, ...)
```

- 容器可声明自身的 `size`（百分比），作为在父容器中占据的空间。
- 嵌套逗号自动按括号深度切分子节点。
- **单子节点穿透**：若容器只有一个子节点且该子节点也是容器，直接穿透，无视方向差异。

### 3.3 多图层与 Z 轴

多次声明 `--layout` 创建多图层，**声明顺序即 Z 轴渲染顺序**（后声明的在上层）。

```bash
--layout "horizontal(area(30%):Tree, area(70%):Preview)" \
--layout "|@(5%,5%) area(95%,95%)[box]:HelpMenu"
```

- **全屏层**：无前缀，铺满终端。
- **浮动层**：`@(x,y)` 前缀，屏幕绝对坐标偏移。坐标支持像素（`10`）或百分比（`5%`）。
- **初始隐藏**：`|` 前缀声明的图层初始 `visible = false`，需通过 `__TOGGLE_LAYOUT__` 或 IPC 控制显隐。

### 3.4 Flexbox 空间分配算法

三阶段分配：

1. **Phase 1 — 基础整数分配**：`Absolute` 节点直接占用固定尺寸；`Percent` 节点按 `flex_len × p / pct_base` 取整。
2. **Phase 2 — 未声明节点公平份额**：未声明尺寸的节点均分剩余空间（含余数分配）。
3. **Phase 3 — 最大余数法**：全局余数按小数部分降序分配，确保容器被**精确填满**，无黑色缝隙。

### 3.5 Auto 语义与防闪烁冻结机制

对于 `area(auto)` 节点，引擎采用“降维打击”策略，不修改底层 Flexbox 算法，而是通过预计算动态覆盖：

1. **预计算降维**：每帧渲染前，`precalculate_auto_sizes` 扫描 AST 中的 `Auto` 节点。对于 `Tree` 取 `visible_ids.len()`，对于 `View` 取 `content_buffer.lines().count()`，将其转换为临时的 `Absolute` 覆盖存入 `auto_overrides` 字典。
2. **字典优先级**：底层 `calc_window_rects` 合并字典时，优先级为 `拖拽物理锁 (window_rect_overrides)` > `Auto预计算 (auto_overrides)` > `AST声明`。
3. **防闪烁冻结**：如果 View 正在异步加载（`is_loading = true`），跳过重算，直接继承上一帧的 `auto_overrides` 高度。但在继承时，必须强制进行 `clamp(term_height)` 限制，防止终端缩小时高度溢出导致布局崩溃。
4. **拖拽固化**：一旦用户拖拽了 `Auto` 窗口的边缘，AST 重组会直接将其固化为静态的 `Absolute`，彻底退出动态计算。需通过 `@layout-reset` 从蓝图快照恢复。

### 3.6 鼠标拖拽与 AST 动态手术

#### Flexbox 边缘拖拽（`ResizeEdge`）

当鼠标按下 `[drag]` 标记的边框时：

1. **首次移动触发 AST 重组**：`restructure_tree_after_drag` 将嵌套叶子拉平为兄弟节点，改变拓扑结构。
2. **像素反算冻结**：`force_recalculate_percentages` 用旧物理坐标反算新 AST 百分比，杜绝拓扑突变的视觉跳跃。
3. **Absolute 覆盖注入**：拖拽过程中注入 `Absolute` 尺寸覆盖，保证物理像素守恒，相邻窗口此消彼长，**无关窗口绝对不动**。
4. **松手固化**：`force_recalculate_percentages` 用最终物理真相反算万分比百分比，清空覆盖，新 AST 接管。

#### 浮动窗口边缘拖拽（`ResizeFloating`）

浮动层（`ScreenAbsolute` 锚点）的窗口支持四边拉伸：

- 位掩码：`1=Left, 2=Right, 4=Top, 8=Bottom`。
- 拖拽时递归修改 Window 节点的 `Absolute2D` 尺寸和图层锚点坐标，保证对侧不动。
- 最小尺寸约束：宽 ≥ 2，高 ≥ 2。

#### 布局重置

```bash
echo "" | stree update @layout-reset           # 清空所有覆盖
echo "" | stree update "@layout-reset Main"     # 仅重置指定窗口
```

### 3.7 图层显隐控制

- `__TOGGLE_LAYOUT__ Name`：切换包含该窗口的图层显隐。打开时自动聚焦图层内第一个窗口；关闭时从 `focus_history` 栈回退焦点。
- `__SHOW_LAYOUT__ Name` / `__HIDE_LAYOUT__ Name`：强制显示/隐藏。
- 支持按图层索引（数字字符串）定位。

---

## 4. 渲染管线：双缓冲 Diff 与 ANSI 透传

### 4.1 双缓冲架构

引擎维护两个全屏 `Buffer`（`CURR_BUFFER` / `PREV_BUFFER`），通过 `thread_local!` + `RefCell` 管理：

1. 每帧在 `CURR_BUFFER` 上绘制所有组件。
2. 与 `PREV_BUFFER` 逐 Cell Diff（`diff_and_flush`），仅输出变化的字符。
3. 交换缓冲区：当前帧移入 `PREV_BUFFER`，旧帧移回 `CURR_BUFFER` 复用内存。

**全屏重绘触发条件**：`prev_rects` 为空（首帧）、终端尺寸变化、图层显隐切换。此时先发送 `Clear(All)`，再重建 `PREV_BUFFER`。

### 4.2 背景擦除与 Z 轴覆盖

- **浮动窗口强制不透明**：为防止底层 Flexbox 窗口内容透出，Z-index > 0 的浮动层在渲染前会强制用空格擦除背景（`clear_bg = true`）。普通底层窗口在无背景色时跳过擦除以提升性能。
- **行级清空**：Tree 和 View 组件在绘制每一行新内容前，会先清空该行残留的旧属性，彻底杜绝颜色重叠残影。

### 4.3 宽字符处理

- 宽度为 2 的字符（如中文）在 Buffer 中占据两列：第一列存储字符本身，第二列存储**空格 `' '` 占位符**（绝不使用 `\0`）。
- Diff 时动态检测左侧字符宽度（`UnicodeWidthChar::width`），若为 2 则当前列是宽字符右半部分：**不打印字符**（防止破坏宽字符），但**必须同步样式状态**（防止 ANSI 状态机脱节导致残影）。

### 4.4 ANSI SGR 解析器

`WindowRenderer::parse_ansi_sgr` 零分配解析器，支持：

| SGR 参数 | 行为 |
| :--- | :--- |
| `0` | 重置所有样式 |
| `1` / `22` | 开启/关闭粗体 |
| `30–37` / `90–97` | 前景色（标准/高亮） |
| `40–47` / `100–107` | 背景色（标准/高亮） |
| `38;5;N` | 256 色前景 |
| `48;5;N` | 256 色背景 |
| `38;2;R;G;B` | RGB 前景 |
| `48;2;R;G;B` | RGB 背景 |
| `39` / `49` | 重置前景/背景色 |

截断逻辑：按可见字符宽度计数，确保不会劈裂 ANSI 序列。超出宽度时追加 `~` 符号（预留 1 列）。

### 4.5 脏标记与延迟广播

- **`dirty_components`**：记录需要重绘的组件集合。Tree 变化时自动标脏所有 StatusBar。
- **`pending_selection_changed`**：`j/k/Space` 等操作不立即广播选中变化，而是在渲染前统一 `flush_pending_updates`，合并多次变更为一次。
- **`prev_rects`**：上一帧的窗口矩形快照，用于检测布局变化并触发 `force_full`。

---

## 5. 交互契约

### 5.1 搜索防幽灵契约与高亮

通过 `/` 激活搜索时，引擎执行实时模糊匹配。

**严格契约**：引擎**仅搜索内容层**（`ID`, `Display`, `Path`），**绝对不搜索元数据层**（`Tags`）。
**安全剥离扩展名**：搜索 Path 时，使用 Rust 标准库 `Path::with_extension("")` 移除文件扩展名，完美处理多级目录和带点的文件名（如 `/tmp/my.note.md` -> `/tmp/my.note`），防止搜索扩展名匹配到所有文件。

*设计意图*：防止搜索 `"li"` 时，因隐藏的 `"live"` 标签导致所有节点全亮。

**搜索高亮**：匹配的子串会在树视图中以 TrueColor 红色 (`\x1b[38;2;201;59;59m`) 高亮显示，使用 `\x1b[39m` 恢复前景色，确保不破坏选中行的背景色。

### 5.2 默认快捷键

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

**Shift 兼容**：`Shift + 字母` 自动规范化为 `NONE + 大写字母`（如 `shift-g` ≡ `G`）。

### 5.3 焦点系统

- **`set_focus` 统一入口**：自动维护 `focus_history` 栈（最多 10 层），**绝不允许聚焦到 StatusBar**。
- **异步刷新防挂起**：`set_focus` 修改焦点后，不会同步触发视图刷新（避免 `&mut self` 借用冲突），而是将刷新请求挂起到 `pending_selection_changed` 队列，由主循环在渲染前统一消费。这解决了 `Tab` 切换时 View 不刷新的时序 Bug。
- **方向切换（`focus_direction`）**：基于物理矩形计算空间距离（`tolerance = 2`），按欧几里得距离排序选择最近候选。仅考虑**可见图层**内的组件。
- **Z 轴切换（`cycle_layer`）**：在可见图层间循环跳转，优先从 `focus_history` 中恢复该图层的历史焦点。
- **焦点回退（`recover_focus`）**：当前焦点丢失时，从历史栈恢复；历史栈为空时选择第一个非 StatusBar 组件。

### 5.4 鼠标行为

| 操作 | 行为 |
| :--- | :--- |
| 悬停（`Moved`） | 切换焦点到悬停窗口（不进入点击逻辑），**StatusBar 免疫** |
| 左键单击 | 选中节点 / 切换焦点。若 Tree 开启 `click:` 前缀则触发 `click` 信号 |
| 左键双击（< 300ms） | 展开/折叠 + 触发 `confirm` 信号 |
| 右键按下 | 切换标记，进入右键拖拽模态 |
| 右键拖拽 | 框选标记/取消标记（`is_marking` 决定方向） |
| 滚轮 | 批量移动 `scroll_step` 行（默认 3） |
| 边框拖拽 | Flexbox 边缘调整 / 浮动窗口四边拉伸 |

**事件批处理**：每帧批量读取所有事件到 `key_events` / `mouse_events` 队列。连续 `Drag`/`Moved` 事件去重（仅保留最新一条）。拖拽中拿到最新坐标后立即退出去渲染。

### 5.5 终端尺寸变化

终端尺寸改变时：
1. 清空 `window_rect_overrides`（解除物理锁，让 AST 百分比接管）。
2. 清空 `prev_rects`（强制全屏重绘）。
3. 标脏所有组件。

---

## 6. 执行模型：统一上下文与双轨制

### 6.1 统一执行上下文

引擎将所有变量收集到 `HashMap<String, String>` 中，在拆分命令参数**之前**进行展开。

- **空格安全**：`split_args` 发生在替换之前，业务变量包含空格时仍为单个参数。
- **确定性替换**：Context Keys 排序后替换，确保嵌套占位符替换顺序确定。

| 占位符 | 展开规则 |
| :--- | :--- |
| `{id}` / `{path}` / `{display}` / `{tags}` | 选中节点的对应字段。无选中则空字符串。 |
| `{ids}` | 所有被标记节点 ID（空格分隔）。无标记则回退到 `{id}`。 |
| `{paths}` | 所有被标记节点 Path，双引号包裹，内部引号转义。 |
| `{input}` | Input 提交时用户输入的原始字符串。 |
| `{window}` | 当前焦点窗口名称。 |
| `{width}` / `{height}` | `--view` 中：View 内部内容区尺寸。`--bind` 中：终端总尺寸。 |
| `{event}` | 触发绑定的信号名称（`select`, `click`, `focus`, `confirm`, `load`）。 |

### 6.2 双轨制执行模式

| 模式 | 触发 | 行为 | 失败处理 |
| :--- | :--- | :--- | :--- |
| **默认模式** | `--bind "enter=vi {path}"` | 挂起 TUI → 释放 TTY → 子进程继承终端 → 恢复 TUI → 触发 `refresh_engine_state` | `last_error = "Command exited with code X"` |
| **静默模式（`@`）** | `--bind "ctrl-t=@script.sh"` | 保留 TUI。stdin/stdout/stderr 全部重定向到 `/dev/null`。仅调用 `.status()` 获取退出码。 | **成功**：触发 `refresh_engine_state`。**失败**：引擎**不设置 `last_error`**，错误提示 100% 交给脚本通过 IPC 推送。仅当 `exec` 系统调用本身失败（如文件不存在）时才设置 `last_error`。 |

**`refresh_engine_state` 三步曲**：
1. `trigger_reload`：重新执行所有 Tree 的数据源命令。
2. 清空所有 View 的 `cached_entity_id`，强制重新加载。
3. `broadcast_selection_changed`：广播当前选中状态，触发 View 异步加载。若焦点不在 Tree 上，则**回退到主树 (`main_tree_name`)** 作为上下文刷新 View，避免视图永久挂起。

### 6.3 静默模式的死锁防御

**fd 继承死锁**：若使用 `.output()`，后台孙进程继承 pipe 写端导致 EOF 永远不到来。防御：`Stdio::null()` + `.status()`。

**IPC 同步死锁**：`@` 模式的脚本内部调用 `stree update` 时，引擎主线程阻塞在 `wait()` 无法处理 IPC。**契约**：脚本必须将 IPC 调用放入后台（`(...) &`），让脚本瞬间退出。

### 6.4 信号绑定

```bash
--bind "select=@echo {id}"           # 全局信号
--bind "TreeA:select=bat {path}"     # 局部作用域
--bind "ctrl-t=@switch-view.sh"      # 物理按键
```

- 局部作用域优先于全局。
- 信号绑定**强制静默执行**（`execute_command_silent`），绝不交出 TTY。

| 信号 | 触发时机 | 防抖 |
| :--- | :--- | :--- |
| `select` | 选中项变化 | 200ms |
| `click` | 单击节点（需 `click:` 前缀） | 无 |
| `focus` | 窗口获得焦点（需 `focus:` 前缀） | 无 |
| `confirm` | 双击 / Enter | 无 |
| `load` | Tree 数据重载完毕 | 无 |

### 6.5 `activate_input` 绑定

```bash
--bind "r=activate_input RenameInput"
```

激活指定 Input 组件。Input 的 `Prefix` 被清空（`activate_input(&name, "")`），用户直接输入内容。

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

### 8.1 二进制帧格式

Socket 路径通过 `$STREE_SOCK` 暴露。帧结构（大端序）：

```text
[4B target_len (u32)][8B data_len (u64)][target (UTF-8)][data (UTF-8)]
```

- **硬限制**：`target_len ≤ 128`，`data_len ≤ 512KB`。
- **超时保护**：已接受连接设置 2 秒读写超时。
- **客户端 stdin 读取**：同样受 512KB 上限约束（`take(MAX_DATA_SIZE)`）。

### 8.2 定向局部刷新

```bash
./generate-data.sh | stree update MainTree   # 重建 Tree 内存结构 + 广播选中 + 触发 load 信号
echo "Loading..." | stree update Preview     # 直接替换 View 缓冲区
echo "[ERR] ..." | stree update Status       # StatusBar 临时消息推送
```

- **Tree 更新**：解析 TSV → 重建内存树 → 保留 `expanded_ids`（retain 有效 ID）→ 恢复选中 → 广播 → 触发 `select` 和 `load` 信号。
- **View 更新**：替换 `content_buffer`，清空 `cached_entity_id`，滚动归零。
- **StatusBar 更新**：推送的消息作为**临时消息**显示，3 秒后自动过期恢复原模板。

### 8.3 布局控制指令

```bash
echo "" | stree update @layout-reset           # 清空所有拖拽覆盖
echo "" | stree update "@layout-reset Main"     # 仅重置指定窗口
```

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
1. 禁用 Raw Mode。
2. 禁用鼠标捕获。
3. 离开 Alternate Screen。
4. 显示光标。
5. 清理 `$STREE_SOCK`。
6. 调用原始 hook。

### 10.4 环境变量

| 变量 | 作用域 | 描述 |
| :--- | :--- | :--- |
| `$STREE_SOCK` | 引擎 → 子进程 | Unix Domain Socket 路径 |
| `$FORCE_COLOR` | 子进程 | 设为 `1`，强制子进程输出颜色 |
| `$CLICOLOR_FORCE` | 子进程 | 设为 `1`，兼容更多工具 |
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
6. 动态语义降维，底层算法纯粹：
引擎底层只认识有限的基础类型（如 Absolute, Percent）。任何高级动态语义（如根据内容自适应的 Auto），必须在渲染前通过“预计算”降维为静态类型（注入 auto_overrides）。底层 Flexbox 数学算法绝不为业务特性妥协或打补丁。
7. 状态变更解耦，异步挂起队列优先：
在事件处理或焦点切换中，绝不同步触发跨组件的级联广播（极易引发 Rust Borrow Checker 冲突或死锁）。所有状态变更必须丢入挂起队列（如 pending_selection_changed），由主循环在安全的时间点统一 flush。
8. 极限防御与安全降级：
终端环境是极其混乱的（尺寸突变、宽字符越界、异步加载延迟）。所有物理坐标计算必须使用 saturating_sub/add 防止下溢 Panic；所有继承自历史帧的状态（如防闪烁的高度冻结）必须经过 clamp 边界重校验；遇到不可恢复的错误时，通过 panic hook 安全退出并恢复终端，绝不把烂摊子留给 Shell。
