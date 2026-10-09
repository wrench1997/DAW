# 当前开发状态

最后更新：2026-10-09 05:25 UTC。以当前源码、集成状态和实际执行结果为准。

## 当前目标与状态

Citrus Studio 0.5.0-alpha.1 是 clean-room Rust DAW 开发版，尚未达到完整 FL-class 商用品质。Linux 优先开发与验证，Windows 持续检查兼容性。当前产品目标仍是「做一首歌 → 无损编辑 → 保存 → 找回缺失媒体 → 导出 → 重新打开」。

- **当前 Linux 已验证源码：`8c09f457bc994271d9d8bff824b38960bb240131`。** 包含真实电平表、本地 WAV Browser、真实 app UI 输入/布局回归、Windows VST3 native editor 接线及延迟导入修复。完整 Linux all-feature/all-target 测试为 **936 application + 14 helper + 5 protocol**；no-default/all-target 为 **934 application**。fmt、两种 feature 配置的严格 Clippy、all-feature app/helper build 及真实 Linux helper smoke 均通过；默认测试栈。完整 Python discovery **153 项通过**。Linux native editor 仍明确不支持。
- **真实 Mixer 电平表已集成：** `21989d0` + `ad565d7`，读取实际 post-fader stereo 图缓冲；Master 在 tanh 保护前测量，显示 dBFS、hold、CLIP/fault。稳定 track ID/graph/epoch 防错位，排队峰值、超时失效、点击 reset 和暂停时 live-MIDI 路径均有回归。已移除 UI 假 sine 动画。详见 [MIXER_METERING](docs/MIXER_METERING.md)。
- **最近已完成 Windows 验证：`53494d518bfc4bad7304f25c127491a0c67d7cd8`。** [Quality 37886481852](https://github.com/wrench1997/DAW/actions/runs/37886481852) 成功，**909 项 Rust 测试**、14 项 Python helper tests、实际 helper smoke 及全部开发门禁通过，包含真实 UI 输入和窄窗口布局修复。随后 `b76ba322` 的 Windows native-editor 候选通过 933 application + 13 helper + 5 protocol Rust 测试及全部源码门禁；实际 fixture attach/resize/stdout isolation 通过，但 PrintWindow 按钮捕获失败，后续交互/state round-trip 未执行，整个 quality run 未通过。
- **首个完整 Preview 验证已通过。** 同一 `53494d5` 的 [Run 37886481857](https://github.com/wrench1997/DAW/actions/runs/37886481857) 通过 57 项 Python 测试、909 项 Rust 测试、optimized static-CRT 构建、PE/导入审计、ZIP/哈希校验与解压后隔离 PATH 的实际 helper smoke。此前精确 OS DLL 白名单修复已获执行验证，未知 DLL 拒绝规则保持。上传步骤跳过，GitHub artifacts 为 0；不等于固定 gnullvm Release 或干净系统验收。新 vendor provenance 包含原始 MIT 与四文件精确哈希，等待本候选 Windows 重跑。
- 项目格式 **v11**，旧 v10 文件可加载；新保存需新版本重新打开。评估前保留原项目备份。`main` 未合并，未发布 Release 或二进制 artifact。

## 已执行的验证与边界

- Linux `cargo test --offline --locked --all-features --all-targets`：**936 application + 14 helper + 5 protocol passed**；`--no-default-features --all-targets`：**934 application passed**，均零失败/忽略。fmt、两套严格 Clippy 与 all-feature app/helper build 通过。依赖内部已有一处 upstream deprecated warning，不是项目 Clippy 门禁放宽。
- 为实现严格 Linux 门禁，将仅 Windows backend 与测试使用的 MIDI helper 用 `cfg(any(windows, test))` 编译，并把 BTreeMap import 移到 Windows 模块。没有关闭 warning、删除测试或改动 Windows backend 行为；Linux 物理 MIDI backend 仍未实现。
- Windows 历史完整门禁使用 Windows Server 2025 / MSVC / Rust 1.99，包含 all-features app/helper、严格 Clippy、helper smoke 和 no-default all-target check；测试数量按平台/feature 分别记录，不能互相代替。
- 普通 helper smoke 只证实 JSON 协议、错误恢复、Shutdown/子进程回收；不加载真实插件。另有仅加载仓库 MIT 源构建 fixture 的 Windows native lifecycle/state/stdout harness，执行结果独立记录；Linux 返回 77（UNSUPPORTED / NOT VERIFIED）。Python 是开发验证工具，不是应用运行依赖。
- Linux `4045cb24` 实际创建了原生 X11 窗口，但 Mesa `eglSwapBuffers` 在 `xcb_shm_attach_checked` 报 `EGL_BAD_SURFACE`，界面无法显示。create/edit/undo/save/reopen、WAV native picker/cancel 与真实电平表 GUI 流程均 **BLOCKED / NOT RUN**；没有把启动进程或单元测试当作 GUI 验收。云端也没有物理音频设备。真实设备/插件、固定 gnullvm Release、干净系统安装/启动及完整歌曲场景仍待验收。

## 当前源码已经具备什么

- [Windows VST3 native editors](docs/NATIVE_VST3_EDITORS.md)：helper-owned HWND、Open/Close、真实状态与 dirty revision、停播时 detach/flush 后保存状态和已有 parameter bases 的精确保留已接线。自动化关联的实例禁止 native Open，编辑/capture 期间的 generic editing 与项目快照/拓扑冲突有明确屏障；Linux/VST2 native editors 不支持。Windows fixture runtime、真实 vendor/hardware GUI 仍待独立验收。
- 新发现并修复旧 Browser 基线中的并发问题：MIDI teardown 后排队的 generator 全 Project 候选可能覆盖刚完成的 WAV 导入。现由 import/native Open 共用 project snapshot-transition predicate；实际 app 回归覆盖成功/失败 replacement 和导入独立 Undo，消费结果后仍保留 generation/session 检查。此问题在本轮集成审查发现，不冒充早先已验证。

- 真实 UI 输入/布局回归：运行生产 CitrusApp::ui、真实 egui 鼠标/键盘事件；独立 Linux Vulkan 离屏输出 17 个真实 render checkpoints。由此发现并修复最小窗口 Plugins/Mixer 与 Group/Snap 重叠；About 显示真实平台/运行 host，无 stream 则显式 offline。详见 [Headless UI QA](docs/HEADLESS_UI_QA.md)。这是 offscreen app renders 与输入回归，不是 native desktop screenshots，也不替代原生文件对话框或硬件验收。

- 本地素材浏览器：SOUNDS 已替换硬编码假条目/假预览，增加显式文件夹选择、有界单层 WAV 列表、搜索、Up/Refresh/Cancel 和复用真实 decoder 的 Playlist 导入；成功导入独立 Undo。实现与验收边界见 [Local sample browser](docs/LOCAL_SAMPLE_BROWSER.md)，最终集成源码 Linux no-default/all-target **901 项通过**，fmt、严格 Clippy 和 debug build 通过；该 Browser checkpoint 已通过 Windows 900 项测试；不宣称 audition、GUI/设备验收或完整素材库完成。
- 编辑：Pattern/Piano Roll、Playlist 分组、Slip、Fade/Crossfade、手势 Undo/Redo 已接线。`playlist::create_audio_crossfade` 支持同轨、非嵌套重叠的两条 Audio Clips；`clip_fade.rs` 的等功率 envelope 由实时与离线路径复用。
- 保存/恢复：blank project、Save/Save As、New/Open/Quit 未保存变更保护、插件状态屏障、同步后原子替换、autosave 和恢复/丢弃对话框已存在。不能把本轮媒体恢复工作描述成首次加入自动保存。
- 导出：`export.rs` 支持 plugin-free Pattern/WAV arrangement 的 stereo PCM24 WAV；会阻止可能漏掉启用插件或 sidechain 的离线导出。`master_capture.rs` / `audio.rs` / `app.rs` 已连接实时 Master Capture，包含有界队列、PCM24 后台写入、停止确认和无覆盖发布。
- 导出进度/Cancel、单任务/session 保护、最终发布竞争判定及 active unsupported automation 拒绝已通过 805 项 Windows checkpoint。新 [WAV 选项](docs/WAV_EXPORT_OPTIONS.md) 已集成：PCM16/PCM24/float32、文件采样率、legacy peak attenuation 或 preserve-level 显式选择/复核。默认保持旧 PCM24/0.95 peak 策略；preserve-level PCM 超限会拒绝，float 保留有限超限样本。新选项与 v11 渲染已通过当前 Windows 全部门禁；真实 native Save/GUI/audio 场景仍待验收。
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
