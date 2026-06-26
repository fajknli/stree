# stree 引擎契约与 CLI 参考手册

本文档是 `stree` 引擎与外部业务层（胶水脚本）之间的严格契约。任何偏离这些规范的行为都可能导致未定义的结果。

---

## 1. 数据协议

### 1.1 实体数据流 (stdin / IPC)

引擎从标准输入或 IPC 读取制表符分隔值（TSV）流。每一非空行必须严格包含 4 个字段：

```text
ID \t Display \t Path \t Tags
```

#### 字段规范

| 字段 | 约束条件 | 引擎行为 |
| :--- | :--- | :--- |
| **ID** | 非空字符串。在流内必须全局唯一。 | 用作树形关联、选中状态记忆和 IPC 定位的主键。重复的 ID 会被去重（保留最后一次出现的）。 |
| **Display** | 任意 UTF-8 字符串。 | 在树形列表中逐字渲染。引擎不解析也不解释其内容。 |
| **Path** | 任意 UTF-8 字符串。 | 视为不透明的业务负载。通过 `{path}` 占位符原样传递给外部命令。当展开为 `{paths}` 时，路径内部的双引号会被自动转义。 |
| **Tags** | 逗号分隔的标签列表（如 `live,note,inbox`）。 | 按 `,` 拆分，去除首尾空白，并过滤掉空标签。**仅供样式引擎匹配使用，绝对不参与搜索匹配**（见第 4 节）。 |

#### 防御性解析规则

1.  **强制 Trim**：每个字段的前导/尾随空白符和回车符（`\r`）会被剥离。
2.  **空 ID 拒绝**：`ID` 字段为空的行会被静默跳过（并向 stderr 输出警告）。
3.  **字段数检查**：少于 4 个字段的行会被静默跳过。
4.  **版本头（可选）**：如果第一行的第一个字段以 `VERSION:` 开头，引擎会解析版本号，并从第二个字段开始作为 ID 处理。

### 1.2 关联表 (`--relations <file>`)

一个 2 列的 TSV 文件，用于定义父子关系：`Parent_ID \t Child_ID`。

*   **树构建**：引擎在内存中构建离散的树形拓扑。任何未在任何关系中作为子节点出现的 ID，都会自动成为根节点。
*   **排序**：根节点按字母顺序排序。节点内部的子节点保留其在关联文件中出现的顺序。
*   **重载规则**：执行初始启动、IPC `stree update <TreeName>`、SIGUSR1 重载时，若启动参数携带 `--relations`，引擎会重新读取该磁盘文件；若未携带，则复用内存中的关系拓扑。

---

## 2. 组件声明与前缀

### 2.1 Tree 组件 (`--tree`)

```bash
--tree "[click:][focus:][nomark:]Name:SourceCmd"
```

*   **`click:`**：单击节点即触发 `click` 信号（默认需双击或 Enter 触发 `confirm`）。
*   **`focus:`**：当焦点切换到该 Tree 时，触发 `focus` 信号。
*   **`nomark:`**：禁用该 Tree 的右键拖拽标记功能。
*   **`SourceCmd`**：启动时及重载时执行的命令，其 stdout 必须为合法的 TSV 流。

### 2.2 View 组件 (`--view`)

```bash
--view "Name:RenderCmdTemplate"
```

*   当 Tree 选中状态改变时，引擎使用当前选中节点的上下文展开 `RenderCmdTemplate` 并执行，其 stdout 渲染到 View 窗口。
*   **特殊占位符**：在 View 的模板中，`{width}` 和 `{height}` 展开为 **View 窗口的内部内容区尺寸**（而非终端总尺寸），常用于传递给 `bat --terminal-width` 或 `ffmpeg` 等工具。

### 2.3 StatusBar 组件 (`--statusbar`)

```bash
--statusbar "Name:FormatTemplate"
```

除了常规占位符，StatusBar 专属以下引擎状态占位符：
*   `{stree_focus}`：当前焦点窗口名。
*   `{stree_visible}`：当前 Tree 可见节点数。
*   `{stree_total}`：当前 Tree 总节点数。
*   `{stree_marked}`：当前 Tree 被标记的节点数。
*   `{stree_id}`：当前选中节点的 ID。

### 2.4 Input 组件 (`--input`)

```bash
--input "Name:Prefix:OnSubmitTemplate"
```

*   **`Prefix`**：激活时显示的前缀（如 `/` 或 `:`）。
*   **`OnSubmitTemplate`**：用户按下 Enter 后执行的命令模板。用户输入的文本通过 `{input}` 占位符展开。
*   **特殊行为**：如果 `Prefix` 为 `/`，引擎将拦截输入并执行**内部搜索过滤**（见第 4 节），而不会执行 `OnSubmitTemplate`。

---

## 3. 布局引擎与多图层

### 3.1 节点语法

```text
area(size)[border,drag]:Name
```

*   **`size`**：`50%`（百分比）、`3`（绝对尺寸）、*(留空)*（均分剩余空间）。
*   **`border`**：`box`（默认）、`line`（单顶线）、`none`。
*   **`drag`**：允许该边框被鼠标拖拽调整大小。

### 3.2 多图层与 Z 轴

多次声明 `--layout` 参数，声明顺序即为 Z 轴渲染顺序（后声明的在上层）。

```bash
--layout "horizontal(area(30%):Tree, area(70%):Preview)" \
--layout "@(10,5) area(40,15)[box,drag]:Popup"
```

*   **全屏层**：无前缀，自动铺满终端 (Z=0, 1, ...)。
*   **浮动层**：`@(x,y)` 前缀，相对于终端屏幕的绝对坐标偏移。

### 3.3 鼠标拖拽与状态锁定

当带有 `[drag]` 标记的边框被鼠标拖拽时：
1.  **拖拽中**：引擎写入 `Absolute` 像素值，保证物理像素守恒，相邻窗口此消彼长，**其他无关窗口绝对不动**。
2.  **松手后**：引擎**保持 `Absolute` 锁定**（拖拽即锁定）。终端 Resize 时，未拖拽的窗口（`Percent`）自适应缩放，拖拽过的窗口保持像素不变。
3.  **重置**：通过 IPC 发送 `@layout-reset` 可清空所有锁定，恢复初始的 `Percent` 声明。

---

## 4. 交互与搜索契约

### 4.1 搜索防幽灵契约

当通过 `/` 激活 Input 并输入搜索词时，引擎执行模糊匹配。
**严格契约**：引擎**仅搜索内容层**（`ID`, `Display`, `Path`），**绝对不搜索元数据层**（`Tags`）。
*   *设计意图*：防止搜索 "li" 时，因为隐藏的 "live" 标签导致所有节点全亮（幽灵匹配）。业务层若希望某些标签被搜索到，应将其拼接到 `Display` 字段中。

### 4.2 默认快捷键

| 按键 | 动作 | 按键 | 动作 |
| :--- | :--- | :--- | :--- |
| `j` / `Down` | 向下移动 | `k` / `Up` | 向上移动 |
| `h` / `Left` | 折叠/父节点 | `l` / `Right` | 展开/子节点 |
| `Enter` | 展开并触发 `confirm` | `Space` | 切换标记 (Mark) |
| `g` / `G` | 跳转顶部/底部 | `Tab` | 切换焦点窗口 |
| `/` | 激活搜索 (前缀 `/`) | `:` | 激活命令 (前缀 `:`) |
| `Esc` | 取消输入/退出 | `q` | 退出并输出选中 ID |

### 4.3 鼠标行为

*   **左键单击**：选中节点 / 切换焦点。
*   **左键双击**：展开/折叠节点并触发 `confirm`。
*   **右键拖拽**：框选标记/取消标记节点（若未禁用 `nomark:`）。
*   **滚轮**：上下滚动当前焦点窗口。
*   **边框拖拽**：按住带有 `[drag]` 标记的边框拖动，实时调整窗口大小。

---

## 5. 执行模型与占位符 (`--bind`)

### 5.1 占位符展开

占位符在命令被拆分为参数**之前**进行展开。

| 占位符 | 展开规则 |
| :--- | :--- |
| `{id}` / `{path}` | 选中节点的 ID/Path。若无则空字符串。 |
| `{ids}` | 所有被标记节点的 ID 列表（空格分隔）。若无标记则回退到 `{id}`。 |
| `{paths}` | 所有被标记节点的 Path 列表，每个 Path 用双引号包裹，内部引号被转义。 |
| `{input}` | 仅在 Input 组件提交时有效，展开为用户输入的原始字符串。 |
| `{window}` | 当前焦点窗口的名称。 |
| `{width}` / `{height}`| 在 `--view` 中：View 内部尺寸。在 `--bind` 中：终端总尺寸。 |

### 5.2 执行模式

*   **默认模式（全屏）**：`--bind "enter=vi {path}"`。引擎暂停 TUI，释放 TTY 给子进程，子进程退出后引擎重绘并触发 `trigger_reload`。
*   **静默模式（`@` 前缀）**：`--bind "ctrl-t=@switch-view.sh"`。引擎保留 TUI，子进程以 `null` 标准流后台执行。**不会自动重载**，业务脚本需通过 IPC 推送更新。

---

## 6. IPC 协议与特殊指令

### 6.1 二进制帧格式

Socket 路径通过 `$STREE_SOCK` 暴露。帧结构（大端序）：
`[4B target_len][8B data_len][target (UTF-8)][data (UTF-8)]`

### 6.2 常规更新

```bash
./generate-data.sh | stree update MainTree   # 更新 Tree
echo "Loading..." | stree update Preview     # 更新 View
```

### 6.3 布局控制特殊指令

引擎拦截以 `@` 开头的特定 Target，不将其视为组件名：

*   **`stree update @layout-reset`**
    清空所有因鼠标拖拽产生的 `Absolute` 尺寸锁定，使所有窗口恢复为 `--layout` 声明的初始 `Percent` 比例。
*   **`stree update @layout-reset <WindowName>`**
    仅清空指定窗口的尺寸锁定，使其恢复为声明式百分比。

---

## 7. 样式引擎 (`--status-col`)

```bash
--status-col "pattern1=style1,pattern2=style2,..."
```

*   **匹配语义**：引擎按逗号拆分节点的第 4 列（Tags）。对于每条规则，检查标签集中是否**有任何一个**标签匹配该模式。
*   **优先级**：颜色覆盖（后匹配的规则覆盖先匹配的），加粗累加（任意规则命中 `bold` 即生效）。
*   **模式**：支持精确字符串（`live`）和正则表达式（`^fail.*`）。

---

## 8. 信号、退出与环境变量

### 8.1 信号处理

| 信号 | 行为 |
| :--- | :--- |
| `SIGUSR1` | 触发 `trigger_reload`：重新执行每个 `--tree` 的数据源命令，并重新读取 `--relations` 文件。 |
| `SIGINT` | 优雅关闭。清理 socket，恢复终端状态。 |

### 8.2 退出行为

优雅退出时，引擎将**最终选中节点的 ID** 输出到 stdout。
```bash
selected=$(my-data.sh | stree --tree "Main")
echo "User selected: $selected"
```

### 8.3 环境变量

| 变量 | 作用域 | 描述 |
| :--- | :--- | :--- |
| `$STREE_SOCK` | 子进程 | Unix Domain Socket 路径。 |
| `$FORCE_COLOR` | 子进程 | 设为 `1`，强制子进程输出颜色。 |
| `$TERM` | 子进程 | 设为 `xterm-256color`。 |
```

### 核心完善点说明：
1. **明确了“搜索防幽灵”契约**：将之前讨论中确定的“Tags 绝对不参与搜索”写入文档，防止未来业务层产生误解。
2. **完善了布局与拖拽契约**：详细解释了 `[drag]` 的行为，以及“拖拽即锁定（Absolute）”和 `@layout-reset` 的联动机制。
3. **补充了多图层与浮动布局**：明确了 `@(x,y)` 和多次 `--layout` 的 Z 轴语义。
4. **梳理了组件前缀与专属占位符**：将 `click:/focus:/nomark:` 以及 View/StatusBar 的特殊占位符独立成节，便于查阅。
5. **规范了 IPC 特殊指令**：将 `@layout-reset` 作为引擎级拦截指令明确列出。
