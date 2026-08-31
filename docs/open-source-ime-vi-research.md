# macOS 开源输入法与 Zed Vim 联动

## 结论

截至本次调研，前端已确定为 Squirrel，目标也不是“换到某个 ASCII 输入源”，而是让 Zed 的最终 Vim 模式与**确切的 Squirrel Rime session**形成可确认、可恢复的状态契约。结论：**Zed 和 Squirrel 两侧都必须修改**。最终方案是在 Squirrel 内新增 session-aware lease server，在 Zed 内新增唯一 owner 的 bridge client；Normal/Visual/Replace 获取 ASCII lease，Insert 释放 lease 并恢复进入命令模式前的真实 `ascii_mode`。现有 CLI、task、keymap、按键猜测、无关联通知和通用 TIS source-ID 切换均不作为最终实现。

## Fcitx5

- 官方仓库：<https://github.com/fcitx/fcitx5-macos>
- 官方文档：<https://fcitx-contrib.github.io/docs/advanced/macosfrontend.html>
- 官方 CLI 文档：<https://fcitx-contrib.github.io/docs/topic/cli.html>
- 官方 README 说明支持 macOS >= 13.3，使用插件管理器安装其他输入引擎。
- 官方插件列表包含 `fcitx5-rime`：<https://github.com/fcitx-contrib/fcitx5-plugins>
- macOS 前端内置 Vim mode，可在 Esc/Ctrl-[/Ctrl-C 时切英文；文档明确说明这是按键检测，可能误判或漏判。
- `/Library/Input Methods/Fcitx5.app/Contents/bin/fcitx5-remote` 是 macOS CLI 子集，官方文档说明配合 `fcitx.vim` 可在 InsertLeave 切英文、InsertEnter 恢复中文。
- 源码：<https://raw.githubusercontent.com/fcitx/fcitx5-macos/master/src/remote.cpp>。`c` 调用 deactivate，`o` 调用 activate，`t` toggle，`n` 查询当前 IM，`s` 选择 IM。

因此 Fcitx5 的前端状态模型比 Squirrel 的有限 `vim_mode` 更适合保存/恢复输入状态；但是官方没有 Zed 集成，也没有 Zed 模式事件来源。

## Squirrel/Rime

- Squirrel 是 macOS Rime/librime 前端，不是另一个中文转换引擎：<https://github.com/rime/squirrel>
- `app_options` 和 `vim_mode`：<https://raw.githubusercontent.com/rime/squirrel/master/data/squirrel.yaml>
- `vim_mode` 实际只在未被 Rime 处理的 Esc/Ctrl-C/Ctrl-[ 上设置 `ascii_mode=true`：<https://raw.githubusercontent.com/rime/squirrel/master/sources/SquirrelInputController.swift>
- Squirrel 主程序有 `--ascii`、`--nascii`、`--getascii`：<https://raw.githubusercontent.com/rime/squirrel/master/sources/Main.swift>
- 没有证据表明 Squirrel 会监听 Zed Vim 状态或恢复进入 Insert 前的状态。

## 其他工具

- `macism`：<https://github.com/laishulu/macism>，切换 macOS input source ID 的 CLI，主要用于 Vim/Emacs workaround；它是切换器，不是输入法，也不会感知 Zed Vim 模式。
- `im-select.nvim`：<https://github.com/keaising/im-select.nvim>，可在 Neovim 的 InsertEnter/InsertLeave 恢复输入源；仅适用于 Neovim，不能感知 Zed。
- `keyboard-switcher`：<https://github.com/tolnaiz/keyboard-switcher>，按应用焦点切换输入源；不理解编辑器 Normal/Insert，也不能恢复每次模式切换状态。

## Zed 当前能力与缺口

- Vim 状态和 keymap context：<https://zed.dev/docs/vim>、<https://zed.dev/docs/key-bindings>。
- Zed 尚未提供系统输入源控制或 Vim mode-change IPC；普通 WASM 扩展也没有编辑器模式、焦点或 `NSTextInputContext` hook。
- 自动输入法切换需求仍为 open：<https://github.com/zed-industries/zed/issues/7997>。
- `jj`/`jk` 抢在 CJK IME 前的问题见 <https://github.com/zed-industries/zed/issues/28174>、<https://github.com/zed-industries/zed/issues/31819>。前者现已标记 completed；当前源码在 `crates/gpui_macos/src/window.rs::handle_key_event` 中对组合型非 ASCII IME 和接受文本的 handler 优先调用 `NSTextInputContext`，不能再把这些历史 issue 当作当前源码仍缺少该处理的证据。

## 推荐

1. 最终方案选择直接的 `SquirrelVimLeaseBridge`：Squirrel 保持当前 input source，不通过 TIS 切走；bridge 只修改 active Rime session 的 `ascii_mode`，不触碰 schema 或 preedit。
2. 必须同时修改 Squirrel 和 Zed，并用版本化、带确认的长期 IPC 协议一起发布。若改动不能进入 Squirrel 上游，就需要维护自定义 Squirrel 构建；只改 Zed 无法满足精确 session 契约。
3. Zed 在发布目标 Vim mode 前必须等到 Squirrel 对同一 session/generation 读回确认；composition 未结束时明确拒绝转换，不允许第一键与异步状态修改竞态。
4. 通用 source-ID、Squirrel CLI 和既有 `vim_mode` 只保留为能力对照或降级路径，不与正式 bridge 同时充当状态写入者。

## macOS 接口与 Zed 集成可行性（追加证据）

### 1. macOS 能做什么，不能把什么误当成 API

**已证实：** AppKit 的 `NSTextInputContext` 表示 Cocoa 文字输入系统；`currentInputContext` 返回当前已激活的输入上下文，`selectedKeyboardInputSource` 是可读写的输入源 ID，`keyboardInputSources` 列出该上下文可用的输入源，系统还提供输入源改变通知。因此，普通 AppKit 应用可以在自己的已激活文字输入上下文上读取并设置输入源，而不是必须实现一个输入法。来源：[Apple `NSTextInputContext`](https://developer.apple.com/documentation/appkit/nstextinputcontext?language=objc)、[`selectedKeyboardInputSource`](https://developer.apple.com/documentation/appkit/nstextinputcontext/selectedkeyboardinputsource?language=objc)、[`keyboardInputSources`](https://developer.apple.com/documentation/appkit/nstextinputcontext/keyboardinputsources?language=objc)。

**已证实：** 较低层的 Text Input Source Services/Carbon 接口以 `kTISPropertyInputSourceID` 标识输入源；Apple 的归档参考包含 `TISCreateInputSourceList`、`TISCopyCurrentKeyboardInputSource` 与 `TISSelectInputSource` 这一套枚举、查询、选择接口。[Apple 归档参考](https://developer.apple.com/library/archive/documentation/TextFonts/Reference/TextInputSourcesReference/)。当前 Zed 的 macOS GPUI 也已经调用 `TISCopyCurrentKeyboardInputSource`、`TISGetInputSourceProperty` 读取输入源类型及 `kTISPropertyInputSourceIsASCIICapable`，见 [`crates/gpui_macos/src/window.rs` 的 `is_ime_input_source_active`](https://github.com/zed-industries/zed/blob/main/crates/gpui_macos/src/window.rs#L2366-L2399)。这证明 Zed 已有可复用的 macOS Carbon 链路，但该函数目前只判定“组合型、非 ASCII-capable 的输入法”，没有选择输入源。

**已证实：** InputMethodKit 的职责是开发输入法、候选窗口以及输入法模式；Apple 的概览明确把 `IMKServer` 描述为输入法服务端，并为每个客户端输入会话创建对应的 `IMKInputController`。`IMKStateSetting` 的 `setValue:forTag:client:` 设置的是输入法自身的状态。它不是普通编辑器用来操控任意系统输入源的通用控制面。[Apple `InputMethodKit`](https://developer.apple.com/documentation/inputmethodkit?language=objc)、[Apple `IMKStateSetting`](https://developer.apple.com/documentation/inputmethodkit/imkstatesetting?language=objc)。所以，把 InputMethodKit 直接加入 Zed 并不能替代 AppKit/Carbon 的输入源选择；Zed 若要使用它，实质上是在实现或承载一个输入法客户端/服务端，范围远超本需求。

**已证实：** `NSTextInputClient` 的协议方法涵盖 marked text、`setMarkedText`、`unmarkText`、`insertText` 和选区；Apple 特别注明这些方法用于文字输入，一般不适合其他目的。[Apple `NSTextInputClient`](https://developer.apple.com/documentation/appkit/nstextinputclient?language=objc)。因此，输入源切换与 marked-text 处理必须是两个独立责任：前者由 `NSTextInputContext`/TIS 完成，后者由 Zed 现有的文字输入客户端完成。

对普通应用的实际限制是：AppKit/TIS 选择的是当前激活上下文的“输入源”，而不是抽象的“中文/英文意图”或任意输入法的内部转换状态；Apple 所列 `NSTextInputContext` 属性提供 source identifier，未定义跨输入法统一的 ASCII/preedit 状态字段（[Apple `NSTextInputContext`](https://developer.apple.com/documentation/appkit/nstextinputcontext?language=objc)、[Apple `selectedKeyboardInputSource`](https://developer.apple.com/documentation/appkit/nstextinputcontext/selectedkeyboardinputsource?language=objc)）。因此，使用 TIS/AppKit 只能可靠地保存/恢复可观察的 source ID；把“source ID 恢复”扩大解释成“恢复 Squirrel/Fcitx5 当前中文转换状态”属于推断，必须由各输入法的专用接口验证。

### 2. Zed 当前的真实模式、输入与焦点边界

**已证实：** Zed 的模式不是一个公开 IPC 事件。`crates/vim/src/state.rs` 的 `Mode` 包含 `Normal`、`Insert`、`Replace`、三种 Visual 以及 Helix 模式（[`Mode`](https://github.com/zed-industries/zed/blob/main/crates/vim/src/state.rs#L43-L54)）。`crates/vim/src/vim.rs` 的 `Vim::switch_mode` 保存 `last_mode`、更新 `mode`、清理操作栈、同步编辑器设置，并处理 Insert/Visual 的选区和事务边界（[`Vim::switch_mode`](https://github.com/zed-industries/zed/blob/main/crates/vim/src/vim.rs#L1208-L1367)）。其中 Helix 模式在同步设置之后还会把 `Normal` 转为 `HelixNormal`、`Visual` 转为 `HelixSelect`，所以任何未来的 mode-change 通知都应在最终模式归一化后发出，而不能只在入口参数变化时发出。

**已证实：** `Vim::switch_mode` 当前没有专门的 mode-change 事件；`VimEvent` 只有 `Focused`，状态栏通过观察 Vim entity 的变更刷新（[`VimEvent` 与 `Vim::new`](https://github.com/zed-industries/zed/blob/main/crates/vim/src/vim.rs#L561-L617)、[`ModeIndicator`](https://github.com/zed-industries/zed/blob/main/crates/vim/src/mode_indicator.rs#L30-L45)）。虽然 `extend_key_context` 暴露了 `vim_mode`（如 `normal`、`insert`、`visual`）和 `VimControl`，这是 keymap 匹配上下文，不是可供扩展订阅的状态事件（[`Vim::extend_key_context`](https://github.com/zed-industries/zed/blob/main/crates/vim/src/vim.rs#L1484-L1543)）。

**已证实：** Zed 已有可用于原生实现的模式动作：`SwitchToNormalMode`、`SwitchToInsertMode` 等动作在 `Vim::activate` 中调用 `Vim::switch_mode`（[`actions!` 定义](https://github.com/zed-industries/zed/blob/main/crates/vim/src/vim.rs#L166-L184)、[`Vim::activate`](https://github.com/zed-industries/zed/blob/main/crates/vim/src/vim.rs#L668-L699)）。但 `Esc`、插入动作、Visual/Replace/临时 Normal、宏重放等路径都会间接调用 `switch_mode`；仅包装一个按键不是完整覆盖。Insert 中的 `NormalBefore` 也会直接调用 `switch_mode(Mode::Normal, ...)`（[`crates/vim/src/insert.rs` 的 `normal_before`](https://github.com/zed-industries/zed/blob/main/crates/vim/src/insert.rs#L35-L101)）。

**已证实：** 编辑器已经发出 `EditorEvent::Focused`、`FocusedIn`、`Blurred`，且 Vim 订阅这些事件；`Vim::focused` 设置当前 focused Vim，`Vim::blurred` 停止录制、保存 Visual marks、清理操作符（[`Editor::handle_focus`/`handle_blur`](https://github.com/zed-industries/zed/blob/main/crates/editor/src/editor.rs#L10511-L10601)、[`EditorEvent`](https://github.com/zed-industries/zed/blob/main/crates/editor/src/editor.rs#L11899-L11965)、[`Vim::handle_editor_event`](https://github.com/zed-industries/zed/blob/main/crates/vim/src/vim.rs#L1142-L1185)、[`Vim::focused`/`blurred`](https://github.com/zed-industries/zed/blob/main/crates/vim/src/vim.rs#L1545-L1631)）。这提供了窗口/编辑器边界，但没有提供“失焦时输入源应该如何保存或恢复”的现成语义。

**已证实：** GPUI macOS 为原生 view 注册了完整的 `NSTextInputClient` 方法，并把回调转发到 `PlatformInputHandler`（[`crates/gpui_macos/src/window.rs` 的注册代码](https://github.com/zed-industries/zed/blob/main/crates/gpui_macos/src/window.rs#L239-L283)、`with_input_handler` [实现](https://github.com/zed-industries/zed/blob/main/crates/gpui_macos/src/window.rs#L3393-L3407)）。编辑器的 `EntityInputHandler` 以 `HighlightKey::InputComposition` 返回 marked range，`unmark_text` 清除 composition highlight 和 IME transaction；替换操作会在 marked ranges 上按多光标展开，而不是把 macOS 的单一 replacement range 生搬到所有光标（[`Editor` 的 marked-text 方法](https://github.com/zed-industries/zed/blob/main/crates/editor/src/input.rs#L2742-L2800)、[`replace_text_in_range`](https://github.com/zed-industries/zed/blob/main/crates/editor/src/input.rs#L2802-L2890)）。

**已证实：** macOS key dispatch 在 composition 存在时先把 key 交给 `NSTextInputContext`；在非 composition 情况下，若输入源是日文/韩文/中文等非 ASCII 组合型 IME 且输入 handler 接受文字，也会先让 IME 处理 printable key，再回退到 Zed key binding。这是为了避免 `jj` 被误识别成 Vim 绑定而破坏日文组合，见 [`handle_key_event`](https://github.com/zed-industries/zed/blob/main/crates/gpui_macos/src/window.rs#L2438-L2499) 与 [`is_ime_input_source_active`](https://github.com/zed-industries/zed/blob/main/crates/gpui_macos/src/window.rs#L2366-L2399)。`Vim::observe_keystrokes` 也会在 pending keys 或 `keystroke.is_ime_in_progress()` 时返回（[`observe_keystrokes`](https://github.com/zed-industries/zed/blob/main/crates/vim/src/vim.rs#L1080-L1106)）。这些路径说明不能通过“在收到 Esc 的第一刻强制切换源”来绕过 composition。

### 3. 三个实现接缝的比较

| 接缝 | 能力与证据 | 结论 |
| --- | --- | --- |
| Zed 原生 macOS 集成 | 在 `Vim::switch_mode` 统一观察最终模式；在 macOS 平台层以 `NSTextInputContext.selectedKeyboardInputSource`（或 TIS）选择 ASCII 源；Zed 已有 `PlatformInputHandler` 和 IME marked-range 管线。相关接口见 [Apple `selectedKeyboardInputSource`](https://developer.apple.com/documentation/appkit/nstextinputcontext/selectedkeyboardinputsource?language=objc)、[`crates/gpui/src/platform.rs` 的 `PlatformWindow`](https://github.com/zed-industries/zed/blob/main/crates/gpui/src/platform.rs#L800-L850)、[`crates/gpui_macos/src/window.rs`](https://github.com/zed-industries/zed/blob/main/crates/gpui_macos/src/window.rs#L1914-L1928)。 | 唯一能系统性覆盖所有模式路径、每个 Zed window 和 composition 边界的方案；需要新增公开于 Zed 内部的抽象和 macOS 后端，不能只改 keymap。 |
| Vim 模块钩子 + 外部 CLI | Fcitx5 官方 CLI 文档明确支持“InsertLeave 切英文、InsertEnter 恢复中文”，并给出 `fcitx.vim` 示例；Squirrel 官方 `Main.swift` 提供 `--ascii`、`--nascii`、`--getascii`，通过 distributed notification 与输入法进程通信。[Fcitx5 CLI](https://fcitx-contrib.github.io/docs/topic/cli.html)、[Squirrel `Main.swift`](https://raw.githubusercontent.com/rime/squirrel/master/sources/Main.swift#L53-L93)。 | 对已有输入法的内部状态（尤其 Squirrel 的 ascii mode）比通用源 ID 更精确，但 Zed 当前没有可订阅的 Vim mode-change IPC；必须先加 Zed 钩子，且异步子进程存在竞态、权限、超时和焦点变化风险。 |
| 扩展、task、keymap workaround | Zed tasks 能运行 shell 命令，变量主要是当前文件/选区等上下文；文档列出的自动 hooks 只有 `create_worktree`，没有 Vim mode 或 focus hook（[Tasks](https://zed.dev/docs/tasks)）。扩展 WASM 的 WIT world 暴露 `process`，`run-command` 等能力，但没有编辑器 focus、Vim mode 或 NSTextInputContext API（[`extension.wit`](https://github.com/zed-industries/zed/blob/main/crates/extension_api/wit/since_v0.8.0/extension.wit#L1-L20)、[`process.wit`](https://github.com/zed-industries/zed/blob/main/crates/extension_api/wit/since_v0.8.0/process.wit#L1-L29)）；执行还受 `process:exec` capability 约束（[Extension Capabilities](https://zed.dev/docs/extensions/capabilities)）。 | 只能做手动 PoC（例如显式 task 或绑定某个 `vim::NormalBefore`），不能可靠地在每次模式转换时保存/恢复，也不能知道 marked text 是否仍在 composition。 |

表中第二行的“必须先加 Zed 钩子”是对当前源码的推断：官方 Fcitx5/Squirrel 接口确实存在，但它们不会凭空得知 Zed 的 `Mode`；当前 `VimEvent` 也只有 `Focused`。第三行关于扩展能力是对当前 WIT 导出面的源码事实，不能据此推断未来扩展 API 永远不会增加此能力。

### 4. 通用 source-ID 架构（未采用）

当输入法前端尚未确定、需求只要求恢复 source ID 时，可以实现下面的通用模块。现在前端已确定为 Squirrel，且目标要求精确 session、`ascii_mode`、composition 和首键顺序，因此本节只保留为 fallback 对照；最终方案以第 6 节为准。

1. 在 `crates/gpui/src/platform.rs` 的 `PlatformWindow` interface 增加最小的 macOS 输入源操作：读取当前 source ID、取得用户最近使用的 ASCII-capable source、按 ID 选择 source。macOS adapter 放在 `crates/gpui_macos/src/window.rs` 或相邻 keyboard 实现中，复用该 crate 已链接的 Carbon/TIS；不要假定 `com.apple.keylayout.ABC` 一定存在。Apple 归档 TIS 参考定义了 `TISCopyCurrentASCIICapableKeyboardInputSource` 和 `TISSelectInputSource`；`NSTextInputContext.selectedKeyboardInputSource` 也提供当前 activated context 的可读写 ID（[Apple TIS 归档参考](https://developer.apple.com/library/archive/documentation/TextFonts/Reference/TextInputSourcesReference/)、[Apple `selectedKeyboardInputSource`](https://developer.apple.com/documentation/appkit/nstextinputcontext/selectedkeyboardinputsource?language=objc)）。
2. 在 `crates/vim/src/vim.rs` 增加私有的模式同步 helper，由 `Vim::switch_mode` 在 Helix 模式归一化后调用，并由 `Vim::focused`、`Vim::blurred` 和 `Vim::deactivate` 处理初始焦点、编辑器切换和关闭。无需先公开 `ModeChanged` 事件：当前只有 Vim 自己消费该行为，新增事件会扩大 interface 而没有第二个调用方。
3. 只把 `Mode::Insert` 视为应恢复用户输入源的文字输入模式。当前 `Vim::editor_input_enabled` 仅对 `Insert` 返回 `true`，`Replace` 与 Normal/Visual/Helix 模式都返回 `false`（[`Vim::editor_input_enabled`](https://github.com/zed-industries/zed/blob/main/crates/vim/src/vim.rs#L1448-L1464)）；因此首版应让 Replace 保持 ASCII，而不是把它与 Insert 合并。
4. 把唯一协调状态放在已有的 `VimGlobals` 中，而不是散落到每个 editor：`Inactive`、`Deferred { owner, generation }`、`ForcedAscii { owner, previous_id, forced_id, generation }`。`VimGlobals` 已保存 `focused_vim`（[`VimGlobals`](https://github.com/zed-industries/zed/blob/main/crates/vim/src/state.rs#L249-L310)、[`Vim::focused`](https://github.com/zed-industries/zed/blob/main/crates/vim/src/vim.rs#L1595-L1620)），适合协调 macOS 当前 activated context 这一共享状态。
5. 第一次从 Insert 离开或聚焦到非 Insert editor 时，保存当前 ID 并切到 ASCII；Normal/Visual/Replace 内的重复切换不得覆盖 `previous_id`。回到同一 owner 的 Insert，或该 owner blur/deactivate 时，仅在当前 ID 仍等于 `forced_id` 的情况下恢复 `previous_id`；如果用户在 Normal 中手动切过输入源，则释放 owner 但保留用户的新选择。聚焦另一个 editor 前，先释放旧 owner，再依据新 editor 的模式重新同步。
6. composition 仍由现有 GPUI 顺序负责：marked text 存在时先交给 `NSTextInputContext`，只有 IME 未处理时才进入 Zed action。若模式确实在 marked text 尚存时被程序化改变，记录 `Deferred`，在下一次输入/composition 状态变化后重查，再切源；不要调用 `discardMarkedText` 或直接清除 highlight。若现有 `EditorEvent::InputHandled` 无法稳定表示 composition 结束，应新增一个窄的 `InputCompositionChanged` 事件，而不是轮询或猜按键。
7. 所有查询/选择失败都应保持原输入源、记录可诊断错误并继续编辑；不允许因为 TIS/AppKit 失败阻断 Vim 模式切换。输入法专用 provider 只在 source-ID PoC 证明无法满足某个已命名输入法后再设计；届时 Fcitx5 与 Squirrel 才构成第二、第三个真实 adapter。

该架构的可行性结论是：**Normal/Visual/Replace 自动 ASCII 可行；Insert 恢复 source ID 可行；安全处理 CJK marked text 可行；跨输入法恢复内部 ascii/preedit 状态不可作通用保证。** 实现范围主要集中在 GPUI macOS `PlatformWindow` adapter、Vim 的集中模式转换与焦点生命周期，以及一个可观察 composition 结束的窄事件。

### 5. 通用 source-ID PoC（未采用）

1. **平台探针：** 在聚焦的 Zed native view 上读取当前 ID，选择 `TISCopyCurrentASCIICapableKeyboardInputSource` 返回的目标，再按保存 ID 恢复。验收：目标不存在或选择失败时不改变状态；切到其他应用后不向其 activated context 写入。
2. **模式/owner 探针：** 覆盖 `i`、Esc、Visual、Replace、临时 Normal、Helix、启停 Vim、两个 editor 和两个 window。验收：每次只存在一个 owner；Insert 恢复；非 Insert 为 ASCII；blur/deactivate 不把 Zed 强制的 ASCII 泄漏到其他输入场景。
3. **composition 探针：** 用中文拼音/Wubi、日文和韩文，在 preedit 存在时测试 Esc、Ctrl-[、`jj`/`jk`、鼠标触发模式动作和候选提交。验收：不丢 preedit、不把拼音残片写入 Normal、candidate 提交后才执行 deferred switch，并保持当前 `handle_key_event` 的 IME-first 顺序。
4. **用户覆盖探针：** Normal 中手动改输入源，然后回 Insert、切 editor、失焦。验收：控制器检测到当前 ID 不再等于 `forced_id` 后不恢复旧值。
5. **正式化：** 上述探针通过后，增加一个默认关闭的 Vim 设置、针对状态转换/owner 校验的 GPUI 测试和 macOS 手工回归说明。task、keymap、WASM 扩展只保留作实验工具。

## 6. Squirrel 当前实现审计：精确桥接必须修改 Squirrel

本节把目标收窄为“Zed 当前焦点所对应的、确切的 Squirrel Rime session”，而不是仅仅选择一个 ASCII-capable input source。上文关于通用 source-ID 方案的可行性仍然成立；但它不能满足这里的 session、`ascii_mode`、composition 和确认顺序契约。

### 6.1 当前 `--ascii`、`--nascii`、`--getascii` 的证据

当前 `Main.swift` 的三个命令都通过 `DistributedNotificationCenter`，名称分别为 `SquirrelToggleASCIIModeNotification` 和 `SquirrelGetASCIIModeNotification`。`--ascii` 只发送 object 为字符串 `"ascii"` 的广播，`--nascii` 只发送 `"nascii"`；二者没有 session ID、client ID、window ID、generation 或 request ID，也没有等待结果。[Squirrel `Main.swift`，命令分支](https://github.com/rime/squirrel/blob/master/sources/Main.swift#L83-L115)

`--getascii` 虽然先注册 `SquirrelASCIIModeResponse` observer，再发送查询，但 observer 的 `object` 是 `nil`，回调只接受任意响应里的字符串，并在最多两秒后打印结果；没有把响应与这次查询关联。没有响应时它直接打印 `"nascii"`。这不是“确认当前状态”，而是一个无法区分“真实 nascii”和“没有活动 session、通知被挂起、Squirrel 尚未启动、响应来自其他查询”的静默猜测。[Squirrel `Main.swift`，查询、超时和 fallback](https://github.com/rime/squirrel/blob/master/sources/Main.swift#L89-L115)

`deliverImmediately: true` 只针对后台 App 的 distributed-notification suspension；源码注释明确说这是为了让 Squirrel 在后台仍收到通知。它不提供送达确认、处理完成确认、发送者身份、请求关联或顺序屏障。[Squirrel `Main.swift`，后台通知说明](https://github.com/rime/squirrel/blob/master/sources/Main.swift#L38-L40)、[发送调用](https://github.com/rime/squirrel/blob/master/sources/Main.swift#L83-L102)

**逐项结论：**

| 性质 | 当前实现 | 结论 |
| --- | --- | --- |
| session-scoped | 广播通知只带命令字符串；没有 `RimeSessionId` | 否 |
| correlated | 查询和响应都没有 request/correlation ID；observer 接受任意同名响应 | 否 |
| acknowledged | set 命令发送后立即退出；get 只有一个无关联的字符串响应 | set 否；get 不是有效 ack |
| failure-safe | `--getascii` 两秒后伪造 `"nascii"`；set 无错误返回 | 否 |
| first-key ordering | Zed 若 shell 出程后继续处理按键，没有跨进程 ack barrier | 否 |

### 6.2 Delegate 与 controller 实际把广播映射到了哪里

`SquirrelApplicationDelegate.addObservers()` 用 `object: nil` 订阅上述三个 distributed notification。`rimeToggleASCIIMode` 收到广播后，只向进程内的普通 `NotificationCenter` 发布 `SquirrelSetASCIIModeNotification`；`rimeGetASCIIMode` 则发布 `SquirrelReportASCIIModeNotification`。这一步仍然没有目标 session。[`SquirrelApplicationDelegate.swift`，observer 注册](https://github.com/rime/squirrel/blob/master/sources/SquirrelApplicationDelegate.swift#L224-L249)、[toggle/get handler](https://github.com/rime/squirrel/blob/master/sources/SquirrelApplicationDelegate.swift#L424-L437)

每一个 `SquirrelInputController` 在 `init` 时都注册这两个进程内通知。set handler 对自己的 `session` 调用 `rimeAPI.set_option(session, "ascii_mode", enableASCII)`；report handler 只检查自己的 `client` 和 session 有效，然后把自己的结果再次广播为 `SquirrelASCIIModeResponse`。因此多个 controller 可以同时被 set，多个 controller 也可以同时响应 get，而 CLI 只取第一个到达的字符串。[`SquirrelInputController.swift`，observer、activate/deactivate 和 composition](https://github.com/rime/squirrel/blob/master/sources/SquirrelInputController.swift#L167-L247)、[ASCII set/report handler](https://github.com/rime/squirrel/blob/master/sources/SquirrelInputController.swift#L621-L640)

这里存在一个容易混淆但很关键的对照：librime 的 `notificationHandler` 确实收到 `sessionId`，并以它更新该 Rime session 的状态；`handleReservedProperty` 也明确检查 `session == sessionId`。这证明 Squirrel 已有 session-scoped 的内部 seam，但当前 distributed/local ASCII 桥没有沿用它。[`SquirrelApplicationDelegate.swift`，librime session notification](https://github.com/rime/squirrel/blob/master/sources/SquirrelApplicationDelegate.swift#L269-L320)、[`handleReservedProperty` 的 session guard](https://github.com/rime/squirrel/blob/master/sources/SquirrelInputController.swift#L294-L318)

`SquirrelInputController` 的 `session` 是 controller 的实例状态：`createSession()` 调用 `rimeAPI.create_session()`，输入事件先用 `find_session(session)` 检查；`activateServer` 更新 client，`deactivateServer` 清空 client，而 `destroySession` 销毁 Rime session。也就是说，只有 Squirrel 自己位于 IMK 生命周期内，才知道“当前激活的 client/controller/session”究竟是哪一个；外部 CLI 不可能从这些广播中推导它。[`SquirrelInputController.swift`，session 字段与事件入口](https://github.com/rime/squirrel/blob/master/sources/SquirrelInputController.swift#L15-L31)、[`createSession`/`destroySession`](https://github.com/rime/squirrel/blob/master/sources/SquirrelInputController.swift#L362-L400)、[`activateServer`/`deactivateServer`](https://github.com/rime/squirrel/blob/master/sources/SquirrelInputController.swift#L167-L225)

composition 也由 controller 持有：`rimeUpdate()` 从该 session 读取 `ctx.composition.preedit`，通过 `setMarkedText` 更新 client；`commitComposition` 则读取完整 input、`insertText` 并 `clear_composition`。特别是当前 `deactivateServer` 无条件调用 `commitComposition`，所以输入源被切走时不能把“source 切换成功”误报为“preedit 已安全保存”。[`SquirrelInputController.swift`，marked text 更新](https://github.com/rime/squirrel/blob/master/sources/SquirrelInputController.swift#L448-L601)、[`deactivateServer`/`commitComposition`](https://github.com/rime/squirrel/blob/master/sources/SquirrelInputController.swift#L221-L250)

因此，对本目标的决定是：**需要修改 Squirrel，答案为“是”**。只改 Zed、调用现有 CLI 或重用现有 distributed notifications，无法让外部进程获得确切 active session、不可误认的 set ack、composition 安全点和首键顺序屏障。修改点不应是继续扩展 `--ascii` 的参数猜测，而应是在 Squirrel 内新增受 session 生命周期约束的桥接服务。

### 6.3 深模块：Squirrel Vim Lease Bridge

模块边界应是一个深模块，例如 `SquirrelVimLeaseBridge`。Zed 只提交目标 Vim mode 和焦点 owner；Squirrel 负责 active IMK controller、`RimeSessionId`、`ascii_mode`、preedit、串行化和状态读回。目标 mode 只有在 bridge 确认后才成为 Zed 的公开最终状态。调用方不直接碰 Rime API，也不把按键名称当作模式事件。

**所有权与最小协议：**

1. Squirrel 在 `activateServer` 后为该 controller 发出不可猜的 `session_token`，同时报告 `session_generation`、client bundle ID 和 `composition = idle|marked`。`session_token` 不暴露 `RimeSessionId`。deactivate、destroy、重新 activate 或 active client 改变时，Squirrel 必须先自动释放该 token 的 lease、按规则恢复，再使 token 失效。
2. Zed 建立长期连接并发送 `hello { protocol_version, zed_instance }`；业务请求只有 `acquire_command { owner_generation }`、`release_to_insert { lease_id, owner_generation }` 和只读 `status`。每条请求都带 `request_id` 和 `session_token`。协议不暴露通用 `set_ascii`，保存、强制和恢复全部封装在 lease 内部。
3. 服务端返回 `applied { request_id, lease_id, session_generation, observed_ascii_mode }`、`rejected { request_id, reason: "CompositionActive"|"StaleSession"|"NotOwner"|... }` 或明确的 `lease_lost`。服务端另发 `session_changed`、`composition_changed` 和 `user_override`；事件也带 token 和 generation。没有“超时就当 nascii”的结果。
4. `applied` 只有在 Squirrel 的串行执行上下文完成内部 `set_option`，并重新读取同一 session 的 `get_option` 确认目标值后才能返回。Zed 把这次同步 request/reply 当作 mode transition 的 prepare barrier，并设置内部硬超时：收到确认后才提交并公开目标 mode，然后返回 native event loop；超时则不提交 mode，直接进入 `Faulted`。这样下一次硬件 key 不可能先于 Squirrel 状态确认到达。若返回 `CompositionActive`，Zed 保持旧 mode，只保留 mode intent；收到真实 composition idle/commit 事件后重新 prepare。

**lease、generation 与幂等性：**

- `acquire_command` 第一次取得 owner 时保存该 session 的原始 `ascii_mode`，并最多强制一次 ASCII；Normal、Visual、Replace、HelixNormal、HelixSelect 内的重复转换只返回同一 lease 的幂等结果，不得覆盖保存值。
- `release_to_insert` 或 owner 失焦只释放该 owner 的 lease。只有当前 session 仍是同一 generation、当前状态仍等于 bridge 强制的值时，才恢复保存值，并对恢复发送一次 `applied`；恢复失败返回明确错误且不伪造成功。
- 若用户在 lease 期间手动改变 `ascii_mode`，Squirrel 以 session token 发出 `user_override`，标记 lease 为 abandoned，释放时不恢复旧值。Bridge 自己触发的 option notification 必须用 request/generation 标记，不能把自己的回写误判成用户覆盖。
- Zed editor/window owner 改变时先 release 旧 owner，再 acquire 新 owner。一个旧窗口的迟到 ack 即使到达，也不能改变新窗口或新 session 的输入法状态。

**composition、焦点与失败语义：**

- `composition = marked` 时，`acquire_command` 返回 `CompositionActive`，不写 `ascii_mode`。不得调用 `discardMarkedText`、`clear_composition`、伪造 `setMarkedText`，也不得截断或猜测 preedit。Squirrel 在真实 `composition_changed(idle)` 或完整 commit 后，Zed 才重新 prepare 并等待新的 `applied`。
- 当前 Squirrel 的 `deactivateServer` 会 commit 完整 composition；Bridge 不得以切 input source 作为同步手段。controller deactivate 时，Squirrel 先对其 lease 自动恢复或明确标记 abandoned，再发送 `session_changed/lease_lost` 并使 token 失效；Zed 丢弃该 generation 的待处理意图。
- active session 是 IMK 的事实，而不是 bundle ID 猜测。两个 editor、两个 Zed window 或同 bundle ID client 必须用 token/generation 区分；Squirrel 只向 active controller 执行 acquire，旧 lease 的 release 则按 token 指向原 session。
- socket 断开、Squirrel 重启、权限/解析失败、session 消失或 option 读回不一致都让 bridge 进入 `Faulted`。Squirrel 对连接持有的 lease 执行自动恢复；Zed 保持最后一个已确认 mode，不再接受新的 coupled transition，也不自动退回 CLI/TIS。UI 必须提供明确错误、重连和“禁用 Squirrel bridge，恢复普通未耦合 Vim”动作；只有用户显式禁用后才恢复未耦合编辑。重连只能重新 hello/同步当前 session，不能重放旧 generation。

### 6.4 IPC 选择

正式实现选用**同用户 Unix domain socket + 长度前缀 Codable JSON frame**，例如位于用户私有 Application Support 目录的 `squirrel-vim-bridge.sock`。Squirrel 创建目录并设为 `0700`，socket 设为 `0600`，校验 peer uid；单条 frame 设上限并拒绝畸形/过大的输入。一个长期连接提供有序字节流、背压、明确 EOF、重连和 request/response multiplex，不需要每次模式切换启动进程。

明确拒绝其他选项：

| IPC | 不选的具体原因 |
| --- | --- |
| 当前 DistributedNotificationCenter | 广播而非寻址；后台 suspension/coalescing；无 sender、session、correlation、ack；正是当前实现无法证明可靠的根因 |
| `--ascii`/`--nascii`/shell CLI | 每次转换创建进程，命令没有 active session 目标和结果；现有 `--getascii` 的两秒 fallback 会把故障伪装成 nascii；不能形成首键 barrier |
| UserDefaults、临时文件或轮询 | 最终状态覆盖中间状态，没有原子处理确认、连接生命周期或 composition 事件 |
| CFMessagePort/Distributed file notification | 旧的命名端点与广播语义仍需另造认证、关联和流控；不能比一个私有 socket 更直接 |
| NSXPC Mach service | 类型化 RPC 本身可行，但需要给当前 InputMethodKit background app 增加 launchd Mach-service 注册、打包/签名和生命周期部署；匿名 endpoint 还要另造安全发现。本项目只有同用户、同机、单服务的窄流，socket 的部署面和故障面更小，故不为“看起来更原生”引入这套额外基础设施。 |

### 6.5 两个仓库的精确落点

Squirrel 侧应修改 `sources/Main.swift` 的 App 生命周期接线（启动、停止服务），`sources/SquirrelApplicationDelegate.swift` 的 endpoint、active-controller 注册、连接认证、generation 和统一串行队列，以及 `sources/SquirrelInputController.swift` 的 token 生命周期、session-scoped lease、composition 状态事件和 bridge/user mutation 标记。新增服务放在 `sources/SquirrelVimLeaseBridge.swift` 并加入 Xcode target；不要把协议继续塞进无 session 的 CLI 分支。

Zed 侧应把 `crates/vim/src/vim.rs` 的模式转换拆成 prepare/commit：先归一化 Helix 目标并向 bridge prepare，收到 `applied` 后才更新公开 mode 和编辑器输入策略；同时覆盖 `focused`、`blurred`、deactivate 和关闭路径。`crates/vim/src/state.rs` 的 `VimGlobals` 保存唯一 owner/lease/generation；新增 `crates/vim/src/squirrel_bridge.rs`（macOS cfg）实现 socket client、协议状态和 barrier。`crates/gpui_macos/src/window.rs` 的现有 IME-first dispatch 必须保留，必要时在 `crates/editor/src/input.rs` 增加窄的 composition 状态事件；相关现有路径见 [`Vim::switch_mode`](https://github.com/zed-industries/zed/blob/main/crates/vim/src/vim.rs#L1208-L1367)、[`observe_keystrokes`](https://github.com/zed-industries/zed/blob/main/crates/vim/src/vim.rs#L1080-L1106)、[`focused`/`blurred`](https://github.com/zed-industries/zed/blob/main/crates/vim/src/vim.rs#L1545-L1631)、[`handle_key_event`](https://github.com/zed-industries/zed/blob/main/crates/gpui_macos/src/window.rs#L2438-L2499)。

### 6.6 可观察验收标准

1. **session 隔离：** 构造两个 Squirrel controller/Rime session；向 session A acquire/release，只有 A 的 `get_option`、状态图标和 ack 改变，B 不收到广播副作用。
2. **确认与首键顺序：** 每次最终 mode transition 都能看到唯一 request ID 的 `applied` 或明确 `CompositionActive`/其他 rejection；`applied` 前 Zed 不公开目标 mode，也不返回 native event loop；`CompositionActive` 时保持旧 mode；迟到旧 generation ack 不产生任何状态写入。
3. **lease 恢复：** Insert→Normal→Visual→Replace→Insert 只保存一次、强制一次、恢复一次；重复相同 request 是幂等的，恢复值等于 lease 开始时真实读取的值。
4. **composition 完整性：** 中文、日文、韩文 preedit 期间的 mode action 只产生 `CompositionActive`，不清除 marked text；候选提交后再执行 pending set；现有 IME-first key dispatch 和完整 composition 内容保持不变。
5. **焦点与窗口：** 两个 Zed window/editor 及切换、blur、Squirrel deactivate、重新 activate 后，只有当前 token/generation owner 能写入；旧 owner 的 response 全部可观测地丢弃。
6. **用户覆盖：** lease 期间用户手动切换 `ascii_mode` 后收到 `user_override`，后续 release/Insert 不恢复旧值；新用户选择保留。
7. **故障安全：** socket 不可用、Squirrel 重启、无 active session、权限拒绝、composition 一直 marked 或 option 读回不一致时，Squirrel 自动释放连接 lease；Zed 不伪造状态、不执行未确认转换并进入可见的 `Faulted`。恢复只能是成功重连，或用户显式禁用 bridge 后回到普通未耦合 Vim，不能静默降级。

这就是与上文“只保存通用 source ID”的边界：若需求只接受 source-ID，仍可不改 Squirrel；若需求是本报告目标的**精确 Squirrel session 桥接**，则必须修改 Squirrel，且最终架构不能是 CLI、keymap 猜测或无关联通知。
