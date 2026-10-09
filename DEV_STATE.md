# 当前开发状态

最后更新：2026-10-09 03:59 UTC。以当前源码、集成状态和实际执行结果为准。

## 当前目标与状态

Citrus Studio 0.5.0-alpha.1 是 clean-room Rust DAW 开发版，尚未达到完整 FL-class 商用品质。开发不会停在 CI 通过：当前优先完成「做一首歌 → 无损编辑 → 保存 → 找回缺失媒体 → 导出 → 重新打开」这一条可复现工作流。

- 当前 Windows 完整验证提交：`a760313d599f458976ad6f9fd8e31acd688d2906`，**862 项 Rust 测试、14 项 Python helper harness、实际 helper smoke 及全部开发门禁通过**，覆盖媒体恢复、v11 split、WAV 选项与堆构造修复。
- 新集成源码候选：`bb5817118b16fde04689d21131fdf1510a177713`，包含 Audio Clip 切分保真/v11、WAV 选项和直接堆初始化修复，应用版本明确为 **0.5.0-alpha.1**。旧 v10 文件可加载；保存将写 v11，旧构建不能重新打开。评估前保留原项目备份。完整候选 Windows 验证已通过；具体目标/feature 的测试数与 Linux 分开记录。
- 合并源码的真实 Linux no-default all-target typecheck 和完整 **860 项 Rust 测试全部通过，0 失败/忽略**，保持默认测试栈。已通过直接堆初始化修复 packet 构造栈溢出；仅将旧 Windows 路径 fixture 在非 Windows 改为本地路径，Windows 原覆盖保留。未调大栈或删测试。使用已安装 ALSA 运行库，不代表 Windows/all-feature VST/GUI/物理设备验收。
- Preview 的第一次 Windows 运行在 Python synthetic Cargo.lock 的 CRLF 哈希 fixture 上失败，尚未构建实际 package；fixture 已修正，52 项 packaging/helper Python 测试已在 Linux 和 Windows 通过；随后 VS shell 初始化出现路径引用错误，包装脚本已修正，实际静态 CRT 构建/包校验待重跑。默认不上传 artifact，不改变固定 gnullvm Release 契约。
- 本轮继续按可复现歌曲工作流完善；`main` 未合并修改，未发布 Release。
- 后续验收顺序：完成上述两个源码切片及新回归 → 串联导出/重开歌曲场景 → 固定 Release 包及真实 Windows 设备验收。详细完成条件见 [开发路线图](docs/DEVELOPMENT_ROADMAP.md)。

## 已执行的基线验证与边界

- [Windows MSVC CI](https://github.com/wrench1997/DAW/actions/runs/37881375638) 在 `a760313d` 全部通过：Windows Server 2025 / x86_64-pc-windows-msvc，Rust/Cargo 1.99.0。
- Windows all-feature Rust **862 通过、0 失败、0 忽略**；helper Rust target 0 tests。另有 **14 项 Python harness 测试通过**，实际 helper 协议 smoke、fmt、Clippy `-D warnings`、application/helper debug build、no-default all-target check 全部通过。
- Linux 优先用于开发和本地验证；当前 no-default 完整 suite **860 通过**。Windows 继续覆盖兼容性、Windows-only 功能及独立 preview。GUI/真实设备验收单独记录，不能与无设备单元测试混同。
- helper smoke 只证实三次 JSON 回复、无效命令恢复、stdin 打开时 Shutdown 以 0 退出及子进程回收；没有加载真实插件。Python 仅供开发验收，不是应用运行依赖。
- 本地 fmt 通过；早期 ALSA metadata 缺失阻碍已通过识别现有真实运行库解决，当前合并源码的 Linux no-default all-target typecheck 已通过。
- **尚未执行当前候选的 GUI、真实设备/插件、固定 gnullvm Release、干净系统安装/启动验收。**「源码已实现」「指定提交代码测试通过」「GUI/设备场景通过」「Release candidate 通过」「商用成熟度」是不同证据层级。

## 当前源码已经具备什么

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
