# 当前开发状态

最后更新：2026-10-09 17:32 UTC。以当前源码、集成状态和实际执行结果为准。

## 当前目标与状态


- **新 scoped-session + reset-origin 合并源码：`f08617092f74903862a9e7bf04698c2db49b523c`。** 审查通过的96410ec与fd6f5b1已合并，独立语义审查确认普通/reset SDK调用共用restricted AtomicBool RAII gate；两种私有facade仍!Send/!Sync，reset只可在session rejoin后由owner执行。新增三路径gate回归与六项reset编译期权限检查；精确Surge instrument1.3.4 reset256兼容修正236f784已合入，136源码hash一致。最终组合源码门禁及新的真实图/UI/state验收通过；新Windows CI须发布后独立执行。
- **Reset边界：** DAW专用Q128 reset将待处理event/host parameter归零偏移并保留FIFO，单独丢弃旧epoch和reset生成的processor输出，保留native UI feedback与真实SDK确认，拒绝unsupported/不确定helper回复。DAW准备max一般要求128..=2048；实际加载的精确Surge instrument UID/version1.3.4使用单次reset256并要求max至少256，其他插件/版本/SurgeFX仍128。Standalone17/47等仍支持。它不改变strict latency/identity fence，不加入DSP线程或通用tail-clear。详见[reset-origin](docs/PLUGIN_RESET_ORIGIN.md)。
- **最终组合门禁通过：** Linux 1,179 app +25 helper +6 reset +5 editor +2 transport protocol、1,167 core，零失败/忽略；395 available vendor cases（同一缺失fixture明确排除）、26 doctests、53/53 private compiler contracts、239 Python（含四项真实Unix descriptor、无skip）、fmt/两套严格rootClippy/build/helper smoke/两种MSVCsource checks通过。Direct vendor Clippy仅保留既有deprecated/drain_collect例外。中断的首次contract日志保留；仅完整重跑的53项计为通过。官方archive+patch重建46文件。
- **新真实插件验收通过：** source236f784与f086170逐字节对应，helper421a8d7d…73c41实际重新绑定。五次genuine调用全部exit0：自动reset后的G4 offsets0/1/127持续发声，held/future+sustain清理和max128拒绝、八个图phase、故意FX32 drift/actualRetry、blank-tail和fresh-state通过。原始旧1/128 immediate-note失败保留；修正仅针对精确instrument1.3.4。Changing debug callback仍有12/14 ordinary和15/17 routed overruns，不代表实时性能认证。
- **新native UI/state smoke通过：** 新Surge停止编辑/20轮poll/save/fresh数值、same-window/closed-used guard、Stochas used-state重开及关闭通过；同一新GUI blob的17/47/128/256首音通过，无positive warmup。未重复fixture/EOF/crash/mixed-Wayland/hardware验收，历史346.9ms resize stall仍在。Reset256在48k只额外推进内部DSP/连续时钟5.333ms，不推进timeline位置或改变PDC。

- **历史 checked-event admission 合并源码：`7d7294b36df8f877c30a16a14afb98cf0f1ac047`。** 只集成审查通过的7d8a416，41 source hashes及native receipt一致。先检查event容量/元数据再copy，明确回传拒绝；普通音符计数、tracked ID和panic release obligation仅在接受后提交，拒绝保留重试责任。App/helper wire/timing/reset保持b92c394不变；完整组合源码门禁及20a9dfc Windows source/preview已通过；native paint仍失败。
- **历史event完整门禁：** Linux1170app+22helper+5+2protocol、1166core，默认stack/零失败忽略；362available vendor（一个明确缺失fixture排除）、26doctests、239Python、fmt/两套严格rootClippy/build/实际helper smoke/两种MSVCsource checks全部通过。Fresh copied helper59b6bcbd…0d086与独立native/default2048验收一致，124production hashes匹配已测event源码；未重复宣称新性能测量。
- **已验证切片与边界：** 362 available vendor tests（一个明确缺失fixture排除）、26 doctests、22helper+5+2protocol及source/native功能检查通过。Exact helper59b6bcbd已通过四项default2048/fresh-state，12/14 changing debug overruns保留。Admission只表示入队，不表示SDK已经处理；payload仍分配/释放，mutex及legacy忽略结果的caller仍在。未加入payload arena、DSP线程或性能提升保证。

- **历史 bounded parameter storage 合并源码：`fb7b91a82d31226485a22e9e5e0b73d1f9a1fe3f`。** 只集成审查通过的bf573a4，41 source hashes一致；App/helper dispatcher/wire/timing不变。Input8192 queues/total points、output4096，检查输入接受与SDK Process结果；输出丢失为Process/SaveState sticky fault，需fresh实例恢复。已通过切片native/首音/cost审查，完整组合源码门禁通过；new-helper2048四项及fresh-state复验通过，两个调用exit0。13/14 changing debug core overruns保留，不代表实时性能提升。
- **上一轮parameter checkpoint完整门禁：** Linux1170 app +22 helper +5+2 protocol、1166 core，零失败/忽略，默认stack；339 available vendor tests（一个明确缺失fixture排除）、26 doctests、231 Python、fmt/两套严格root Clippy/build/实际helper smoke及两种MSVC source checks全部通过。新helper为a29e4942…aee64，与native/首音切片验收helper字节一致。
- **边界：** 无分配/释放只覆盖host owner仍存活时测试到的parameter-container操作，不覆盖整个Process或owner销毁后的插件final Release。构造器合计请求1,573,104bytes，另加外层wrapper/allocator开销；大量populated suffix与部分小输入/读取仍有成本回退。Mutex/event/metering、GUI单线程及历史346.9ms resize stall仍在。

- **历史 native-edit 合并源码：`87ceb06ce3dc23d817b1623093c8a51fddc2bea3`，完整源码门禁通过，default2048/fresh-state限定复验通过。** 审查通过的48d3f97加入固定容量、generation/sequence标记、明确DSP应用确认的原生编辑通道；display/gesture polling不再消费待交给DSP的编辑。真实停止编辑→20轮polling→zero-sample SaveState→fresh实例数值/组件/首音已通过独立切片验收。helper仍单线程；350ms fixture只证明通道consumer独立，不代表真实resize stall或全进程实时安全改善。新helper已独立通过default2048四项/fresh-state复验；旧优化结果保持历史归属。

- **历史 timing/guard 合并、源码门禁及组合真实插件限定验收通过：`4fdfbc2ea5c828fd9c329f31e9be203810fa62c7`。** 审查通过的 Surge guard `c056dc5` 已与五个 timing 提交合并。Guard 为精确 factory UID + version 1.3.4、首次 positive Process/native open 尝试后拒绝 LoadState，在 helper editor closure/状态改动前返回错误。旧实例/窗口保持的真实测试与 fresh restore 通过；未实现静默 settle 或通用 restore 修复，legacy full-chain Admin 在 backend LoadState 前关闭 editor，并在拒绝返回后 fault 其 slot。公共 worker timing 已加入：whole-callback admission、独立 health/fault latch、停止状态 Retry/new epoch，以及带 epoch 的 live-state capture/prevalidated replacement。默认2048；128/256/512 明确 Experimental。先前 timing-only debug真实矩阵 quiet13/16、CPU4 load10/16，2048四种配置均通过两轮，小 profile仍有 DeadlineMiss，不能宣称低延迟/无掉音。详见 [状态恢复边界](docs/PLUGIN_STATE_RESTORE_LIMITS.md)。

Citrus Studio 0.5.0-alpha.1 是 clean-room Rust DAW 开发版，尚未达到完整 FL-class 商用品质。Linux 优先开发与验证，Windows 持续检查兼容性。当前产品目标仍是「做一首歌 → 无损编辑 → 保存 → 找回缺失媒体 → 导出 → 重新打开」。

- **上一轮 Linux 已验证源码：`87ceb06ce3dc23d817b1623093c8a51fddc2bea3`。** 全部41 reviewed helper/vendor/Cargo hashes一致；App/audio/timing/wire保持244f622不变。完整all-feature/all-target为 **1170 application +22 helper +5 editor protocol +2 transport protocol**，no-default为 **1166 application**，零失败/忽略；fmt、两套严格root Clippy、build、实际helper smoke、两种MSVC source checks及 **215 Python tests** 通过。**304 available vendor tests** 串行通过（一个缺失upstream Dexed fixture明确排除），全部 **26 vendor doctests** 通过。新helper SHA256为dca08353…8cbe8，与独立native/PCM验收版本字节一致。
- **真实 Mixer 电平表已集成：** `21989d0` + `ad565d7`，读取实际 post-fader stereo 图缓冲；Master 在 tanh 保护前测量，显示 dBFS、hold、CLIP/fault。稳定 track ID/graph/epoch 防错位，排队峰值、超时失效、点击 reset 和暂停时 live-MIDI 路径均有回归。已移除 UI 假 sine 动画。详见 [MIXER_METERING](docs/MIXER_METERING.md)。
- **最新终态 Windows checkpoint：`20a9dfcb75c0b8aad2b0f43568258679e4eb1690`。** [Quality 37955606457](https://github.com/wrench1997/DAW/actions/runs/37955606457)通过1165app+15helper+5+2protocol、14helper/61editor Python及源码门禁；native state/control/lifecycle通过，paint前后失败、repaint跳过，整个quality仍失败。[Preview 37955606487](https://github.com/wrench1997/DAW/actions/runs/37955606487)通过239Python cases（235passed/four Unix-only skips）、上述Rust tests、optimized/static-CRT/vendor/license/PE/ZIP/extracted-helper smoke；上传跳过、两run artifact API为0。当前session/reset组合需自己的源码/真实插件/Windows复验。
- 项目格式 **v12**，旧 v10/v11 文件可加载，MIDI ports 默认 Off、source audio monitor 默认启用；新保存需 v12 构建重新打开。评估前保留原项目备份。`main` 未合并，未发布 Release 或二进制 artifact。

## 已执行的验证与边界

- Linux `cargo test --offline --locked --all-features --all-targets`：**1170 application +22 helper +5 editor protocol +2 transport protocol passed**；`--no-default-features --all-targets`：**1166 application passed**，均零失败/忽略。fmt、两套严格 Clippy 与 all-feature app/helper build 通过。依赖内部已有一处 upstream deprecated warning，未放宽项目 Clippy。
- 当前 native-edit 切片不改 App/audio/UI，完整 application suites 已重跑既有 app/input 回归。最近一次完整真实离屏画面为4fdfbc2的 **123 entries /49 genuine Vulkan frames**，保持原源码归属，不重复宣称新截图。新build已验证与独立native/PCM helper dca08353…字节一致。
- 为实现严格 Linux 门禁，将仅 Windows backend 与测试使用的 MIDI helper 用 `cfg(any(windows, test))` 编译，并把 BTreeMap import 移到 Windows 模块。没有关闭 warning、删除测试或改动 Windows backend 行为；Linux 物理 MIDI backend 仍未实现。
- Windows 历史完整门禁使用 Windows Server 2025 / MSVC / Rust 1.99，包含 all-features app/helper、严格 Clippy、helper smoke 和 no-default all-target check；测试数量按平台/feature 分别记录，不能互相代替。
- 普通 helper smoke 只证实 JSON 协议、错误恢复、Shutdown/子进程回收；不加载真实插件。另有仅加载仓库 MIT 源构建 fixture 的 Windows native lifecycle/state/stdout harness，执行结果独立记录；Linux 返回 77（UNSUPPORTED / NOT VERIFIED）。Python 是开发验证工具，不是应用运行依赖。
- Linux `4045cb24` 实际创建了原生 X11 窗口，但 Mesa `eglSwapBuffers` 在 `xcb_shm_attach_checked` 报 `EGL_BAD_SURFACE`，界面无法显示。create/edit/undo/save/reopen、WAV native picker/cancel 与真实电平表 GUI 流程均 **BLOCKED / NOT RUN**；没有把启动进程或单元测试当作 GUI 验收。云端也没有物理音频设备。真实设备/插件、固定 gnullvm Release、干净系统安装/启动及完整歌曲场景仍待验收。

## 当前源码已经具备什么

- [Native VST3 editors](docs/NATIVE_VST3_EDITORS.md) 保留 Windows HWND、Linux standalone X11/XWayland-compatible 生命周期和 state/snapshot/automation guards。历史 ownership checkpoint exact helper 的真实 fixture、fresh-instance Surge/Stochas paint/input/state 检查通过，但 **reused Surge 首音丢失、controller getter 过期与内容/容器尺寸不一致仍失败**；旧/新 helper 均复现，属于已有缺陷。新 guard 在明确限定的已使用/已打开编辑器 Surge1.3.4 实例上拒绝这个危险操作；拒绝保持原窗口/状态，不能表述为原操作已成功修复。组件音量在后续正数帧处理后正确应用，不能误报永久 state 丢失。详见 [state limits](docs/PLUGIN_STATE_RESTORE_LIMITS.md)。历史 resize 346.9ms stall 与 Windows paint failure 保留，未宣称 realtime continuity。
- [Plugin ownership preparation](docs/PLUGIN_PROCESSOR_DOMAINS.md) 将 control owner 与 prepared processor storage 分离，提供不可跨线程移动的 facade 和 exclusive borrowed lease；LoadState/bus/topology 重建及 loader/error-unwind lifetime 已加固。仍为单线程同步执行；固定容量编辑通道移除了processor消费GUI edit/display mutex的依赖；新parameter容器在限定生命周期内准备固定storage，但mutex、事件分配及data-exchange/metering仍待处理；真实 helper DSP/GUI 线程拆分未包含；公共 callback timing 已加入但不解决原生 resize stall；Surge 定向 guard 已加入，但只是拒绝危险操作，通用成功恢复仍未实现。
- 新发现并修复旧 Browser 基线中的并发问题：MIDI teardown 后排队的 generator 全 Project 候选可能覆盖刚完成的 WAV 导入。现由 import/native Open 共用 project snapshot-transition predicate；实际 app 回归覆盖成功/失败 replacement 和导入独立 Undo，消费结果后仍保留 generation/session 检查。此问题在本轮集成审查发现，不冒充早先已验证。

- [FL-inspired 视觉](docs/FL_INSPIRED_NATIVE_THEME.md) 与[多窗口工作区](docs/MULTIWINDOW_WORKSPACE.md)：四个真实 editor 同时存在，拖移/缩放、关闭重开、Arrange/Cascade、最大化/还原、layout/focus/stacking 持久化已接线。新 [compact refinement](docs/COMPACT_WORKSPACE.md) 降低 chrome/row 密度、修复 reset geometry 并提供 release-only 边缘对齐；Piano 增加当前 session 的 select-all/Copy/Cut/Paste、typed bounded validation 和单步 Undo，文本框继续独占文字剪贴板。原生 OS clipboard round-trip、跨 DAW/MIDI interchange 和 OS-detached editors 仍未实现或未验收。

- 本地素材浏览器：SOUNDS 已替换硬编码假条目/假预览，增加显式文件夹选择、有界单层 WAV 列表、搜索、Up/Refresh/Cancel 和复用真实 decoder 的 Playlist 导入；成功导入独立 Undo。实现与验收边界见 [Local sample browser](docs/LOCAL_SAMPLE_BROWSER.md)，最终集成源码 Linux no-default/all-target **901 项通过**，fmt、严格 Clippy 和 debug build 通过；该 Browser checkpoint 已通过 Windows 900 项测试；不宣称 audition、GUI/设备验收或完整素材库完成。
- [播放节拍器](docs/METRONOME.md)：Record 旁 CLICK OFF/ON，旧偏好与新安装默认 Off；独立 app preference，不改变 Project/history/dirty。callback 原子开关停止新 click 并清除活动 envelope，保留 transport phase；已进入 PDC/FX/device 的声音可继续衰减。离线 WAV 不包含 click，实时 Master Capture 包含启用时实际渲染的 click；设备恢复/实听仍独立待验收。
- 当前创作重点：[Piano keyboard](docs/PIANO_KEYBOARD_EDITING.md) 支持 Ctrl/Cmd+D deselect、Ctrl/Cmd+B phrase repeat、移动/半音/八度、quick quantize、discard length、ghost toggle；[Piano mouse](docs/PIANO_MOUSE_WORKFLOW.md) 支持按下即 Draw、临时 Ctrl selection、Shift clone、按键时序轴锁与 Draw length、继承触碰音符长度。active Channel/group、ghost exclusion、有限值/全局 ID/bounds 与按手势 Undo 均受保护。发现并修复旧流程中 held-pointer Undo/Redo 可能弹出更早 history 的风险；production undo()/redo() 在 release 前阻止操作。新 [Note expression](docs/PIANO_NOTE_EXPRESSION.md) 支持 Alt+wheel / Ctrl+Alt+wheel 相对力度与细调，边界采用共同 delta 保留强弱差；Inspector 与双击共享一套 draft-only 属性窗，单音已有模型字段、多音相对 pitch/velocity、Reset/Cancel/单步 Apply。Imported timing、frozen targets、stale identity、wheel tail、press-time modifier 和 modal/save/import 屏障均有回归；新 [Range/snap](docs/PIANO_RANGES_AND_SNAP.md) 支持独立 ruler edit range、range-width repeat、Off/fine/triplet local snap；paste 统一使用 viewport 左边所在四拍小节，不依赖 PAT/SONG playhead。窄 Piano 的 NOTE EDIT / SCALE 菜单保留 480×420 minimum 的 note/velocity 空间。Range 不写 Project/history，也不是 playback loop。
- 编辑：Pattern/Piano Roll、Playlist 分组、Slip、Fade/Crossfade、手势 Undo/Redo 已接线。`playlist::create_audio_crossfade` 支持同轨、非嵌套重叠的两条 Audio Clips；`clip_fade.rs` 的等功率 envelope 由实时与离线路径复用。
- 保存/恢复：blank project、Save/Save As、New/Open/Quit 未保存变更保护、插件状态屏障、同步后原子替换、autosave 和恢复/丢弃对话框已存在。不能把本轮媒体恢复工作描述成首次加入自动保存。
- 导出：`export.rs` 支持 plugin-free Pattern/WAV arrangement 的 stereo PCM24 WAV；会阻止可能漏掉启用插件或 sidechain 的离线导出。`master_capture.rs` / `audio.rs` / `app.rs` 已连接实时 Master Capture，包含有界队列、PCM24 后台写入、停止确认和无覆盖发布。
- 导出进度/Cancel、单任务/session 保护、最终发布竞争判定及 active unsupported automation 拒绝已通过 805 项 Windows checkpoint。新 [WAV 选项](docs/WAV_EXPORT_OPTIONS.md) 已集成：PCM16/PCM24/float32、文件采样率、legacy peak attenuation 或 preserve-level 显式选择/复核。默认保持旧 PCM24/0.95 peak 策略；preserve-level PCM 超限会拒绝，float 保留有限超限样本。新选项与 v11 渲染已通过当前 Windows 全部门禁；真实 native Save/GUI/audio 场景仍待验收。
- [VST3 metadata scanner](docs/VST3_SCANNING.md) 已集成：通过隔离 helper 读取实际 default class 的 name/vendor/category/MIDI capabilities，不再用文件名猜测 VST3 类型；旧 cache 请求 rescan，超时/失败保留 Unknown 与可见原因。Surge XT Effects 的错误 Instrument 分类已修正。精确 e6bd216 已重新执行官方 Surge instrument/Effects metadata 与 cache round-trip；fresh helper hash 与旧已验证 helper 相同。完整 [source-only receipts](docs/REAL_VST3_VALIDATION.md) 保留先前 b57076a controlled-offline instrument/FX/audio-chain 和配置后 Stochas MIDI 生成结果；该历史 bundle 本身不证明 downstream MIDI routing；原始数据与 source identity 保持不变。新的路由证据单独记录如下。
- [Plugin MIDI ports](docs/PLUGIN_MIDI_ROUTING.md) 已集成：播放时一个 producer 对一个或多个 exclusive-input instruments，Off/0..255、source audio monitor、停止时编辑和 shared snapshot/history barriers。限制为 constant tempo、bus 0、非零 producer latency 拒绝；stop/seek/loop 重新预滚，无 seamless loop 或停止时 live chain。[真实 production graph 验证](docs/PLUGIN_MIDI_ROUTE_VALIDATION.md) 在精确 e54a6e4 源码上通过 5/5，涵盖官方 Stochas → Surge、FX latency fence/recovery、held/retrigger release 与刻意 overload fail-closed/restart。该历史版本的两个 bridge 在48kHz增加4352 frames /90.667ms；当前 timing default2048则为4864 frames /101.3ms，明确按版本区分；旧 Off 路径 15/10 deadline misses、debug 128-frame callback 3.116ms 超时及原因未确定的历史并发 source-batch loss 均保留。没有据此宣称低延迟、真实设备、Harmony Blueprint 或 native GUI 通过。
- 实时捕获不等于 VST 离线 bounce、自动 tails、stems 或实时/离线完全等价。真实媒体、TempoMap、鼠标、硬件和长期运行仍需单独验收。

## 当前正在补齐的两个缺口

### M1：切分后保持声音与编辑语义

已集成 `audio_clip::split_audio_clip` 与逐边 envelope/source-phase/exact-length 引用，覆盖模型 v11、旧 v10 兼容、JSON 精确浮点回读、实时 Timeline/callback、离线导出与 UI 编辑。取消/进度/不支持 automation 的导出保护保留。切片独立验证有 200 项生产模块测试、严格 Clippy、3 项真实 callback 和4项 App/history 测试通过；当前合并源码已通过本地全目标 typecheck 与860项完整测试，完整 Windows 已通过，GUI/设备验收待执行。细节见 [切分保真](docs/AUDIO_SPLIT_FIDELITY.md)。

### M2：项目媒体诊断和安全重定位

已集成 File → Project media / relink：持续显示路径/加载错误、后台校验候选文件、显式 Apply/Cancel、稳定 asset ID 的单步 Undo/Redo。候选需匹配 sample rate、channels、frame count，防止改变已有 native-frame offsets；旧 session/path/generation 的结果会拒绝。状态为 **源码已集成，Windows 全部门禁通过，GUI 验收待执行**，未改 schema。15 项模块回归与2项 App 新回归均包含在 792 项通过结果中；工作流与限制见 [Project media](docs/PROJECT_MEDIA.md)。

这不是整套 crash recovery 或便携项目打包完成。必须串联「保存 → 媒体丢失 → 定位 → 重开 → 导出」与恢复点 Restore/保存场景，保留实际结果。

## 基线已完成的可靠性工作

- `model.rs` 保存前有限数检查与失败清理；四项新增回归包含 60 个损坏字段/值组合。
- `playlist.rs` 组缩放最小边界、Slip 极值/半开区间、Audio offset 溢出、Fade/Crossfade 非有限终点加固；`app.rs` / `playlist.rs` 手势快照包含 Clips、Automation lanes、Audio routing，离散编辑独立提交。
- `master_capture.rs` 修复停止时最终队列帧可能变成静音的竞态；`export.rs` 拒绝 8000..=192000 Hz 之外的速率和非有限音频数据。
- Rust 1.99 的 9 处 Clippy 兼容性问题修正，未降低门禁或更改固定 Release 工具链。上述 15 项新增 Rust 回归已包含在 775 项 Windows 通过结果中，不能据此推断后来新增测试通过。

## 交接与记录

[开发路线图](docs/DEVELOPMENT_ROADMAP.md) 跟踪优先级、完成条件与实现/设备/发布边界；[工作日志](docs/WORK_LOG.md) 追加实际变更和验证结果；[构建文档](docs/BUILD_AND_RELEASE.md) 记录固定发布流程。[历史交接](docs/HISTORICAL_DEV_STATE.md) 仅用于追溯。当前文档持续跟随实质变更，不以 CI 通过作为开发结束标志。
