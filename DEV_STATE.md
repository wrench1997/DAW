# 当前开发状态

最后更新：2026-10-09 03:09 UTC。以当前源码、集成状态和实际执行结果为准。

## 当前目标与状态

Citrus Studio 0.4.0 是 clean-room Rust DAW 开发版，尚未达到完整 FL-class 商用品质。开发不会停在 CI 通过：当前优先完成「做一首歌 → 无损编辑 → 保存 → 找回缺失媒体 → 导出 → 重新打开」这一条可复现工作流。

- 当前集成源码：独立分支 `ci/windows-reliability-20261009` 的 `4174649`，已在 `598cd48` 基础上集成项目媒体诊断与重定位。其全应用 Windows 验证待执行；旧基线结果不能替代新源码验证。`main` 未合并本轮修改，未发布 Release。
- 项目媒体恢复已集成：15 项精确生产模块回归与子集 Clippy 已通过，本地全仓 fmt 通过；2 项 App 新回归及完整 Windows 门禁待执行。Audio Clip 切分保真仍在独立工作树开发，尚未集成。
- 另有独立 Windows preview 打包流程正在开发：MSVC/static CRT 优化构建、PE/import 校验、白名单 ZIP、哈希/构建信息及解压后 helper smoke。尚未产出或验证 Windows 包，不替代固定 gnullvm Release 契约。
- 后续验收顺序：完成上述两个源码切片及新回归 → 串联导出/重开歌曲场景 → 固定 Release 包及真实 Windows 设备验收。详细完成条件见 [开发路线图](docs/DEVELOPMENT_ROADMAP.md)。

## 已执行的基线验证与边界

- [Windows MSVC CI](https://github.com/wrench1997/DAW/actions/runs/37873807018) 在 `acdcf23` 全部通过：Windows Server 2025 / x86_64-pc-windows-msvc，Rust/Cargo 1.99.0，Python 3.12.10。
- Rust **775 通过、0 失败、0 忽略**；helper Rust target 0 tests。另有 **14 项 Python harness 测试通过**，实际 Windows helper 协议 smoke 通过。fmt、all-feature/all-target Clippy `-D warnings`、application/helper debug build、no-default-features all-target check 全部通过。
- helper smoke 只证实三次 JSON 回复、无效命令恢复、stdin 打开时 Shutdown 以 0 退出及子进程回收；没有加载真实插件。Python 仅供开发验收，不是应用运行依赖。
- 本地 fmt 曾通过。Linux Cargo check 在 `alsa-sys` 因缺少 `alsa.pc` 停止，尚未编译项目源码；不能视作 Linux 通过。
- **尚未执行当前候选的 GUI、真实设备/插件、固定 gnullvm Release、干净系统安装/启动验收。**「源码已实现」「指定提交代码测试通过」「GUI/设备场景通过」「Release candidate 通过」「商用成熟度」是不同证据层级。

## 当前源码已经具备什么

- 编辑：Pattern/Piano Roll、Playlist 分组、Slip、Fade/Crossfade、手势 Undo/Redo 已接线。`playlist::create_audio_crossfade` 支持同轨、非嵌套重叠的两条 Audio Clips；`clip_fade.rs` 的等功率 envelope 由实时与离线路径复用。
- 保存/恢复：blank project、Save/Save As、New/Open/Quit 未保存变更保护、插件状态屏障、同步后原子替换、autosave 和恢复/丢弃对话框已存在。不能把本轮媒体恢复工作描述成首次加入自动保存。
- 导出：`export.rs` 支持 plugin-free Pattern/WAV arrangement 的 stereo PCM24 WAV；会阻止可能漏掉启用插件或 sidechain 的离线导出。`master_capture.rs` / `audio.rs` / `app.rs` 已连接实时 Master Capture，包含有界队列、PCM24 后台写入、停止确认和无覆盖发布。
- 导出已核实的后续缺口：静态离线路径未渲染非 Tempo automation，现有插件/sidechain 检查也未拒绝这些 lane；后台导出没有运行中进度/取消操作。Peak 超过 0.95 时还会整曲衰减，不能默认视为与 Master 完全等电平。
- 实时捕获不等于 VST 离线 bounce、自动 tails、stems 或实时/离线完全等价。真实媒体、TempoMap、鼠标、硬件和长期运行仍需单独验收。

## 当前正在补齐的两个缺口

### M1：切分后保持声音与编辑语义

集成基线在淡入/淡出内部 split 时会向两段复制归一化 fade，无法保持原 envelope；native-source offset 的整数取整还会改变非同采样率切点后的 PCM。正在实现 Audio Clip envelope 原点/范围与 source phase 的模型、迁移、实时/离线渲染及 UI 协同，Pattern split 不在本切片范围内；状态为 **开发中，未集成/未验收**。验收需要覆盖内部/重复切分、右段移动到零点、resize reveal、变速/不同采样率、保存重开、Undo/Redo，以及实时与离线参考输出比较。

### M2：项目媒体诊断和安全重定位

已集成 File → Project media / relink：持续显示路径/加载错误、后台校验候选文件、显式 Apply/Cancel、稳定 asset ID 的单步 Undo/Redo。候选需匹配 sample rate、channels、frame count，防止改变已有 native-frame offsets；旧 session/path/generation 的结果会拒绝。状态为 **源码已集成，完整 Windows 验证与 GUI 验收待执行**，未改 schema。15 项精确生产模块回归与子集 Clippy 已通过；工作流与限制见 [Project media](docs/PROJECT_MEDIA.md)。

这不是整套 crash recovery 或便携项目打包完成。必须串联「保存 → 媒体丢失 → 定位 → 重开 → 导出」与恢复点 Restore/保存场景，保留实际结果。

## 基线已完成的可靠性工作

- `model.rs` 保存前有限数检查与失败清理；四项新增回归包含 60 个损坏字段/值组合。
- `playlist.rs` 组缩放最小边界、Slip 极值/半开区间、Audio offset 溢出、Fade/Crossfade 非有限终点加固；`app.rs` / `playlist.rs` 手势快照包含 Clips、Automation lanes、Audio routing，离散编辑独立提交。
- `master_capture.rs` 修复停止时最终队列帧可能变成静音的竞态；`export.rs` 拒绝 8000..=192000 Hz 之外的速率和非有限音频数据。
- Rust 1.99 的 9 处 Clippy 兼容性问题修正，未降低门禁或更改固定 Release 工具链。上述 15 项新增 Rust 回归已包含在 775 项 Windows 通过结果中，不能据此推断后来新增测试通过。

## 交接与记录

[开发路线图](docs/DEVELOPMENT_ROADMAP.md) 跟踪优先级、完成条件与实现/设备/发布边界；[工作日志](docs/WORK_LOG.md) 追加实际变更和验证结果；[构建文档](docs/BUILD_AND_RELEASE.md) 记录固定发布流程。[历史交接](docs/HISTORICAL_DEV_STATE.md) 仅用于追溯。当前文档持续跟随实质变更，不以 CI 通过作为开发结束标志。
