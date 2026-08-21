# stree 引擎契约与 CLI 参考手册

本文档定义了 `stree` 引擎与外部业务层的严格边界。引擎采用 **混合保留模式**：内部通过 Rust 严格管理高频 UI 交互状态（焦点、滚动、展开、标记），外部通过 Unix 管道、IPC 与声明式配置驱动一切业务逻辑。

**核心哲学**：引擎持有 UI 交互状态，绝不持有业务状态。业务逻辑 100% 由外部脚本处理。

---

## 1. 数据协议：四列 TSV 契约

引擎从标准输入或 IPC 读取 TSV 流。每一非空行必须严格包含 4 个字段：

```text
ID \t Display \t Path \t Tags
```

| 字段 | 约束条件 | 引擎行为 |
| :--- | :--- | :--- |
| **ID** | 非空，流内全局唯一。 | 树形关联、选中记忆、IPC 定位的主键。O(1) HashMap 去重，重复 ID 保留最后一次出现。 |
| **Display** | 任意 UTF-8 字符串。 | 逐字渲染。支持 ANSI SGR 颜色码透传。 |
| **Path** | 任意 UTF-8 字符串。 | 不透明业务负载。通过 `{path}` 原样传递。展开为 `{paths}` 时自动处理空格引号包裹。 |
| **Tags** | 逗号分隔的标签集。 | 按 `,` 拆分并 trim。仅供样式引擎匹配，**绝对不参与搜索**。 |

**防御性解析规则**：
1. **强制 Trim**：剥离字段首尾空白符和 `\r`。
2. **空 ID 拒绝**：静默跳过。
3. **版本头**：若首行首字段以 `VERSION:` 开头，解析版本号，从第二字段开始处理。
4. **字段不足**：少于 4 列的行静默跳过。

**关联表 (`--relations`)**：2 列 TSV 文件定义父子关系。未在任何关系中作为子节点出现的 ID 自动成为根节点（按字母序排列）。递归构建时维护祖先足迹集合，检测到循环引用时截断渲染。

---

## 2. 组件声明与前缀机制

### 2.1 Tree 组件 (`--tree`)
```bash
--tree "[click:][focus:][nomark:][nohover:][nofocus:][search-scope:scope]Name:SourceCmd"
```
- **`click:`**：单击节点即触发 `click` 信号（默认需双击或 Enter 触发 `confirm`）。
- **`focus:`**：焦点切换到该 Tree 时触发 `focus` 信号。
- **`nomark:`**：禁用标记功能。未声明此前缀的 Tree 默认允许 Space 键和鼠标右键拖拽标记。
- **`nohover:`**：免疫鼠标悬停夺焦。鼠标移入该组件不会改变键盘焦点，但滚轮仍可滚动该组件。
- **`nofocus:`**：完全不可聚焦。键盘 `Tab`、方向键切换、鼠标悬停与左键点击均无法使其获得焦点，但滚轮依然生效。适用于纯展示型侧边栏。
- **`search-scope:scope`**：声明该组件的搜索策略。`scope` 可选值为 `all`（默认，搜 ID/Display/Path）、`display`、`id`、`path` 或以逗号组合（如 `id,display`）。实现组件级的搜索数据隔离。
- **`SourceCmd`**：启动及 SIGUSR1 重载时执行的命令，stdout 必须为合法 TSV 流。


### 2.2 View 组件 (`--view`)
```bash
--view "[nohover:][nofocus:]Name:RenderCmdTemplate"
```
- **`nohover:`**：同 Tree，免疫鼠标悬停夺焦，实现“左手键盘控制左侧，右手滚轮预览右侧”的高级交互。
- **`nofocus:`**：同 Tree，完全不可聚焦。适用于纯粹的只读预览面板。
- **异步执行与防抖**：Tree 选中项改变时，引擎在后台线程异步执行命令。快速滚动时，若选中 ID 与缓存一致且缓冲区非空，直接跳过 I/O。
- **竞态保护**：异步结果返回时，校验 `cached_entity_id == target_id`，过期结果丢弃。
- **加载挂起**：若 View 正在加载，新的选中变化挂起到 `pending_view_reload`，待加载完成后重触。
- **尺寸占位符**：`{width}` 和 `{height}` 展开为 View 内部内容区尺寸（减去边框开销）。

### 2.3 StatusBar 组件 (`--statusbar`)
```bash
--statusbar "Name:FormatTemplate"
```
引擎渲染前将内部 30 余种状态收集为 HashMap，支持 `{stree_id}`, `{stree_visible}` 等占位符自动替换。当 Input 激活时，StatusBar 物理区域被劫持；全局错误以红底白字覆盖。

### 2.4 Input 组件 (`--input`)
```bash
--input "[instant:][search:]Name[Target]:Prefix:[@]OnSubmitTemplate"
```
- **`instant:`**：瞬时模式。按下任意字符键立即提交该字符并退出。
- **`search:`**：实时搜索模式。输入时引擎原生拦截并触发对 `Target` Tree 的纯客户端模糊搜索，不执行 `OnSubmitTemplate`。
- **`Name[Target]`**：声明组件名及劫持目标。如 `SearchInput[LeftTree]`。
- **`@` 静默前缀**：模板以 `@` 开头时，提交时静默执行（不挂起 TUI）。
- **退格退出**：输入框内容已空时，再次按下 `Backspace` 等同于按 `Esc` 退出。

---

## 3. 布局引擎：正交 Flexbox 与运行时重组

### 3.1 节点语法
```text
area(size)[border,drag]:Name
```
- **`size`**：`50%` (万分比精度)、`3` (绝对字符)、`auto:5` (自适应高度，fallback 5 行)、`40,15` (二维绝对，浮动层专用)、`area:Main` (均分剩余)。
- **`border`**：`box` (默认)、`line`、`none`。
- **`drag`**：允许该边框被鼠标拖拽调整大小。

### 3.2 多图层与 Z 轴
多次声明 `--layout` 创建多图层，声明顺序即 Z 轴渲染顺序。
- **全屏层**：无前缀，铺满终端。
- **浮动层**：`@(x,y)` 前缀，屏幕绝对坐标偏移（像素或百分比）。
- **初始隐藏**：`|` 前缀声明的图层初始不可见。

### 3.3 Auto 预处理降级与状态冻结
1. **预计算降级**：每帧渲染前，扫描 AST 中的 `Auto` 节点。Tree 取 `visible_ids.len()`，View 取 `content_buffer.lines().count()`，转换为临时 `Absolute` 存入 `auto_overrides`。
2. **字典优先级**：`拖拽物理锁` > `Auto预处理` > `AST声明`。
3. **加载期冻结**：View 异步加载时跳过重算，继承上一帧高度，并强制 `clamp(term_height)` 防溢出。
4. **拖拽固化**：拖拽 `Auto` 窗口边缘后，AST 重组直接固化为静态 `Absolute`。

### 3.4 鼠标拖拽与拓扑重组
- **Flexbox 边缘拖拽**：拖拽时注入物理覆盖冻结像素，松手时反算 AST 百分比。**需相邻双方均声明 `[drag]`**。
- **浮动窗口拉伸**：支持四边拉伸，递归修改 `Absolute2D` 尺寸与图层锚点。**仅需自身声明 `[drag]`**。

---

## 4. 渲染管线与交互契约

### 4.1 双缓冲 Diff 与 ANSI 透传
- 维护 `CURR_BUFFER` 与 `PREV_BUFFER`，逐 Cell Diff 后仅输出变化字符。终端尺寸变化或图层切换时触发全屏重绘。
- 浮动层渲染前强制用空格擦除背景，防止底层透出。
- 动态检测宽字符右半部分，跳过打印但同步 SGR 状态，防止状态机脱节残影。
- 零分配 ANSI SGR 解析器，全面支持 256 色、RGB 与粗体。

### 4.2 搜索防幽灵契约与作用域控制
- **严格契约**：引擎默认绝对不搜索元数据层（`Tags`），仅搜索内容层（`ID`, `Display`, `Path`）。
- **组件级隔离**：通过 `search-scope:` 前缀，可为不同组件赋予不同的搜索策略。例如文件管理器的书签组件可声明 `search-scope:display:`，确保搜索时只匹配书签名，不会因长串的绝对路径导致误匹配。
- **安全剥离**：当作用域包含 `path` 时，使用 `Path::with_extension("")` 移除文件扩展名后匹配，完美处理多级目录和带点文件名。
- 匹配子串以 TrueColor 红色高亮，使用 `\x1b[39m` 恢复前景色，不破坏选中行背景色。


### 4.3 焦点系统与异步挂起
- **`set_focus` 统一入口**：自动维护 `focus_history` 栈，绝不允许聚焦到 StatusBar 或 `nofocus:` 组件。
- **异步防挂起**：焦点切换时严禁同步触发跨组件刷新（防借用冲突），状态变更推入 `pending_selection_changed` 或 `pending_blur`，由主循环渲染前统一 `flush`。
- **方向切换**：基于物理矩形计算欧几里得距离排序，选择最近候选。

### 4.4 信号模型与内部指令直连
- **信号**：`select` (防抖200ms)、`click`、`focus`、`blur`、`confirm`、`load`。
- **内部指令直连**：若信号绑定的命令解析为内部 UI 指令（如 `__SHOW_LAYER__`），引擎在内存中同步执行，零进程开销。
- **外部降级**：非内部指令则降级为外部静默 Shell 执行。

### 4.5 默认鼠标与快捷键
| 操作 | 行为 |
| :--- | :--- |
| 悬停 | 切换焦点（StatusBar、`nohover:` 和 `nofocus:` 组件免疫） |
| 左键单击 | 选中节点 / 切换焦点。`nofocus:` 组件免疫不夺焦。`click:` 前缀触发信号 |
| 左键双击 (< 300ms) | 展开/折叠 + 触发 `confirm` |
| 右键拖拽 | 框选标记/取消标记 |
| 滚轮 | **直接作用于鼠标当前悬停的组件**（即使它没有键盘焦点），批量移动 `scroll_step` 行 |

| 按键 | 内部命令 | 行为 |
| :--- | :--- | :--- |
| `j`/`Down`, `k`/`Up` | `__DOWN__`, `__UP__` | 垂直移动 |
| `h`/`Left`, `l`/`Right` | `__EXPAND__` | 展开/折叠切换 |
| `Enter` | `__ENTER__` | 展开/折叠 + 触发 `confirm` |
| `Space` | `__MARK__` | 切换标记（自动下移） |
| `g`, `G` | `__TOP__`, `__BOTTOM__` | 跳转顶/底 |
| `Tab` | `__TAB__` | 在当前可见图层的主要组件间循环切换焦点（过滤 `nofocus:`） |
| `Ctrl-T` | `__CYCLE_LAYER__` | Z 轴跨图层循环切换焦点 |
| `Ctrl-H/J/K/L` | `__FOCUS_*__` | 方向焦点切换（空间距离排序，过滤 `nofocus:`） |
| `H`, `L` | `__SCROLL_*__` | 水平左/右滚 5 字符 |
| `Esc` | `__ESC__` | 取消输入 / 退出搜索 / 取消拖拽 |
| `q` | `__EXIT__` | 退出引擎 |

### 4.6 按键绑定与内部指令直连机制 (核心)

通过 `--bind "Key=Cmd"` 绑定按键。引擎在接收到按键事件时，优先匹配内部指令直连，不匹配则降级为 Shell 执行。

#### 按键描述规则
- **修饰键**：支持 `ctrl-`, `alt-`, `shift-` 前缀（或 `+` 连接）。不区分大小写。如 `ctrl-h`, `Alt+Enter`。
- **普通字符**：直接书写。如 `a`, `1`, `/`。注意：`shift-a` 会被引擎规范化为 `A`。
- **特殊键**：`enter`, `esc`, `tab`, `backspace`, `delete`, `space`, `up`, `down`, `left`, `right`, `home`, `end`, `pageup`, `pagedown`, `f1`-`f12`。

#### 作用域绑定
支持 `Scope:Key=Cmd` 语法。当焦点处于 `Scope` 组件时，该绑定生效，优先级高于全局绑定。
```bash
# 仅当焦点在 RenameInput 时，按 Enter 关闭它
--bind "RenameInput:enter=__CLOSE_OVERLAY__ RenameInput"
```

#### 内部指令全集 (零延迟同步执行)
绑定以下指令时，引擎在 Rust 内存中直接完成状态变更，不产生任何进程开销。

| 内部指令 | 参数 | 行为 |
| :--- | :--- | :--- |
| `__EXIT__` | 无 | 请求引擎退出主循环。 |
| `__ESC__` | 无 | 退出输入 / 取消搜索 / 隐藏浮动层 / 请求退出。 |
| `__TAB__` | 无 | 在当前可见图层的主要组件间循环切换焦点。 |
| `__CYCLE_LAYER__` | 无 | Z 轴跨图层循环切换焦点。 |
| `__UP__` / `__DOWN__` | 无 | 向上/向下移动选中项（Tree）或滚动（View）。 |
| `__TOP__` / `__BOTTOM__` | 无 | 跳转到顶/底部。 |
| `__EXPAND__` | 无 | 切换当前节点的展开/折叠状态。 |
| `__MARK__` | 无 | 切换当前节点标记状态（标记后自动下移）。 |
| `__ENTER__` | 无 | 切换展开 + 触发 `confirm` 信号。 |
| `__SCROLL_LEFT__` | 无 | 水平左滚 5 字符。 |
| `__SCROLL_RIGHT__` | 无 | 水平右滚 5 字符。 |
| `__FOCUS_LEFT__` | 无 | 焦点向左移（基于空间距离排序）。 |
| `__FOCUS_RIGHT__` | 无 | 焦点向右移。 |
| `__FOCUS_UP__` | 无 | 焦点向上移。 |
| `__FOCUS_DOWN__` | 无 | 焦点向下移。 |
| `__ACTIVATE_INPUT__` | Name | 激活指定 Input 组件，接管 StatusBar。 |
| `__TOGGLE_LAYOUT__` | Name | 切换指定图层的可见性。 |
| `__SHOW_LAYOUT__` | Name | 显示指定图层。 |
| `__HIDE_LAYOUT__` | Name | 隐藏指定图层。 |
| `__CLOSE_OVERLAY__` | Name | 关闭指定的 Input 覆盖层。 |
| `__CLOSE_TOP_OVERLAY__` | 无 | 关闭栈顶的 Input 覆盖层。 |

---

## 5. 执行模型与统一上下文

### 5.1 统一执行上下文
| 占位符 | 展开规则 |
| :--- | :--- |
| `{id}` / `{path}` / `{display}` / `{tags}` | 选中节点的对应字段。无选中则空。 |
| `{ids}` / `{paths}` | 所有被标记节点对应字段（空格分隔）。**无标记时回退到 `{id}` / `{path}`**。 |
| `{input}` | Input 提交时用户输入的原始字符串。 |
| `{window}` | 当前焦点窗口名称。 |
| `{width}` / `{height}` | `--view` 中：View 内部尺寸。`--bind` 中：终端总尺寸。 |
| `{event}` | 触发绑定的信号名称。 |

### 5.2 双轨制执行模式
- **默认模式**：挂起 TUI → 释放 TTY → 子进程继承终端 → 恢复 TUI → 触发 `refresh_engine_state`（清空 View 缓存并重载）。
- **静默模式 (`@`)**：保留 TUI。引擎将命令派发至后台线程（`std::thread::spawn`）异步执行，stdin/stdout/stderr 重定向至 `/dev/null`。此举彻底切断了主线程与外部脚本的生命周期绑定，防止脚本内部回调 `stree update` 时导致 IPC 管道死锁。成功退出后通过通道通知主线程触发全局刷新；失败不报错，交由脚本 IPC 推送。


---

## 6. IPC 协议与系统指令

Socket 路径通过 `$STREE_SOCK` 暴露。帧结构（大端序）：
`[4B target_len][8B data_len][target (UTF-8)][data (UTF-8)]` (硬限制：`target_len ≤ 128`，`data_len ≤ 512KB`)。

### 6.1 组件数据更新
```bash
./generate-data.sh | stree update MainTree   # 重建 Tree 内存结构 + 广播选中 + 触发 load
echo "Loading..." | stree update Preview    # 直接替换 View 缓冲区 (纯文本)
echo "[ERR] ..." | stree update Status       # StatusBar 临时消息推送（3秒后过期）
```

#### View 图形透传协议 (STREE_GRAPHIC)
若需向 `View` 组件（如预览面板）推送二进制图形数据（如 Sixel、Kitty 图形协议字节流），数据流必须以特定头部声明：
```bash
printf "STREE_GRAPHIC\n" | cat - <(chafa -f sixel image.jpg) | stree update Preview
```
- **识别机制**：引擎读取 `View` stdout 时，若前 15 字节匹配 `STREE_GRAPHIC\n`，则判定后续数据为二进制图形流。
- **内存处理**：引擎将剥离头部，剩余字节作为 `Vec<u8>` 原始保留在内存中，**绝不进行 UTF-8 校验或 `lines()` 扫描**，彻底消灭大图导致的 CPU 阻塞。
- **渲染机制**：渲染时引擎将绕过双缓冲字符 Diff，直接通过 PTY 将字节流 `write_all` 给终端。并在绘制前强制用空格擦除物理背景，防止旧图残影。
- **进程组强杀**：若 View 绑定的外部预览脚本执行过慢（如视频缩略图生成），引擎在收到新的选中项变更时，会通过 `libc::kill(-pgid, SIGKILL)` 强杀上一个还在运行的预览进程树，防止孤儿进程堆积导致系统卡顿。

```markdown
#### 图形数据体积防御 (防 PTY 阻塞)
终端 PTY 管道缓冲区极小（通常 16KB~64KB）。若一次性写入过大的图形数据（如全屏 4MB Sixel），将瞬间撑爆管道触发“背压”，导致引擎主线程被操作系统强制挂起，引发严重卡顿和按键事件丢失。

**强制契约**：外部脚本在生成 Sixel 等图形字节流时，**必须**限制最大输出分辨率（建议宽度不超过 120 字符，高度不超过 40 字符，数据量控制在 200KB 以内）。
若需预览超高清原图，建议在外部脚本中先进行 `downscale` 降采样，或通过环境变量检测终端是否支持 Kitty Graphic Protocol (KGP) 等异步图层协议。引擎底层对 Sixel 模式不提供异步分块写入，严格遵守体积约束是保证 TUI 丝滑流畅的唯一途径。
```



### 6.2 系统控制指令
以 `@` 开头的 target 被视为系统指令，处理完毕后直接返回，不走数据更新逻辑。

| 指令 | 格式 | 行为 |
| :--- | :--- | :--- |
| `@exit` | `stree update @exit` | 请求引擎优雅退出。 |
| `@layout-reset` | `stree update @layout-reset` | 清空拖拽物理锁，从蓝图快照完全重建 AST。 |
| `@clear-marks` | `stree update @clear-marks` | 清理所有 Tree 组件的标记状态。 |
| `@layout-show` | `stree update "@layout-show HelpMenu"` | 显示指定图层。 |
| `@layout-hide` | `stree update "@layout-hide HelpMenu"` | 隐藏指定图层。 |
| **`@select`** | `stree update "@select LeftTree docs"` | **强制指定 Tree 的选中 ID（触发焦点跳转与视图滚动，恢复空间记忆）。** |
| **`@title`** | `stree update "@title LeftTree $PWD"` | **动态修改 Tree 的边框标题（上下文感知）。** |

---

## 7. 样式与主题引擎

支持 TrueColor 十六进制（`#a9b5d5`）和 3 位简写（`#fff`）。

### 7.1 数据状态颜色 (`--status-col`)
```bash
--status-col "live=white,archived=gray,^fail.*=red,bold,__marked__=#c93b3b"
```
- **匹配语义**：按逗号拆分 Tags，任意标签命中规则即生效。
- **优先级**：颜色覆盖（后匹配覆盖先匹配），粗体累加（任意规则命中 `bold` 即生效）。
- **模式**：含 `^`, `*`, `.`, `+`, `?`, `[` 的 pattern 自动编译为正则，失败降级精确匹配。
- **内置规则**：`__marked__` -> `red,bold`。

### 7.2 UI 框架颜色 (`--ui-colors`)
格式 `key=value` 逗号分隔。支持键：`border_focused`, `border_unfocused`, `view_focused`, `view_unfocused`, `statusbar_fg`, `input_prefix`, `input_buffer`, `selected_bg`, `error_fg`, `error_bg`, `empty_data_fg`。

---

## 8. CLI 参数与环境变量

### 8.1 全局选项
| 参数 | 类型 | 默认值 | 说明 |
| :--- | :--- | :--- | :--- |
| `--relations` | String | 无 | 关联表文件路径 |
| `--layout` | 可重复 | `area:Main` | 布局声明（顺序即 Z 轴） |
| `--tree` | 可重复 | 无 | Tree 组件声明 |
| `--view` | 可重复 | 无 | View 组件声明 |
| `--statusbar` | 可重复 | 无 | StatusBar 组件声明 |
| `--input` | 可重复 | 无 | Input 组件声明 |
| `--bind` | 可重复 | 内置默认 | 按键/信号绑定 |
| `--border-chars` | 可重复 | 无 | 自定义边框字符（6字符：┌┐└┘│─） |
| `--status-col` | String | `""` | 数据状态颜色规则 |
| `--ui-colors` | String | `""` | UI 框架颜色主题 |
| `--select` | String | 无 | 启动时预选指定 ID |
| `--no-mouse` | Flag | false | 禁用鼠标 |
| `--scroll-step` | u8 | `1` | 滚轮每次移动行数 |
| `--max-lines` | usize | `1000` | View 输出最大行数（超出截断并 kill） |

### 8.2 环境变量与信号
| 变量 / 信号 | 作用域 | 描述 |
| :--- | :--- | :--- |
| `$STREE_SOCK` | 引擎 → 子进程 | Unix Domain Socket 路径 |
| `$FORCE_COLOR` | 子进程 | 设为 `1`，强制子进程输出颜色 |
| `$CLICOLOR_FORCE`| 子进程 | 设为 `1`，兼容更多工具 |
| `$TERM` | 子进程 | 设为 `xterm-256color` |
| `SIGUSR1` | 引擎 | 触发全局重载（重新执行 Tree 数据源命令与关联表读取） |
| `SIGINT` | 引擎 | 优雅关闭。清理 socket，恢复终端状态 |

### 8.3 Panic 安全与退出
引擎注册自定义 panic hook：禁用 Raw Mode → 禁用鼠标 → 离开 Alternate Screen → 显示光标 → 清理 `$STREE_SOCK`。
优雅退出时，引擎将最终选中节点的 ID 输出到 stdout。

---

## 9. 架构红线

1. **业务隔离**：引擎绝不硬编码业务语义，只理解 ID、Display、Path、Tags。
2. **状态解耦**：引擎不持久化业务状态，重启后重置。业务状态 100% 由外部脚本管理。
3. **机制优于策略**：提供标签匹配机制但不定义标签含义；提供 IPC 通道但不定义推送格式。
4. **主循环纯洁性**：主循环只消耗在纯计算上。物理 I/O 剥离到子进程或后台线程。
5. **动态语义降级**：底层算法纯粹。高级动态语义（如 `Auto`）必须在渲染前降级为静态类型注入字典，底层 Flexbox 数学算法绝不为特定特性妥协。
6. **异步挂起队列优先**：严禁同步触发跨组件级联广播。状态变更推入挂起队列，由主循环在安全时间点统一 `flush`。
7. **极限防御**：所有物理坐标计算使用饱和运算防 Panic；历史帧状态继承必须经过 `clamp` 边界重校验；遭遇不可恢复错误时通过 panic hook 安全退出。
8. **焦点与交互解耦**：引擎将键盘焦点与鼠标悬停分离。通过 `nohover:` 和 `nofocus:` 前缀，允许组件仅接受滚轮滚动而不夺焦或不可聚焦，保障复杂 TUI 布局的空间多路复用体验。
