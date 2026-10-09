# 当前开发状态

最后更新：2026-10-09 01:52 UTC。以当前源码和实际命令结果为准。

## 当前基线与验证边界

- 项目：Citrus Studio 0.4.0，clean-room Rust DAW；尚未达到完整 FL-class 商用品质。
- 本次 Git 基线：`9c159953163763a354634b3a9f95f84de174641b`。
- 已完成源码核对：Playlist 分组/Slip、Audio Crossfade、Realtime Master Capture 已有实现与 UI 接线，不能列为完全未实现。
- 本地 Rust/Cargo 1.99.0 已可用，fmt 检查通过。Linux `cargo check --locked --no-default-features` 在依赖 alsa-sys 构建阶段因缺少 alsa.pc 停止，尚未编译项目源码；完整开发质量门禁在 GitHub Actions Windows MSVC 运行。
- **已验证 Rust 测试：775 通过，0 失败、0 忽略**，对应提交 `bcaf5e68dd8b865b9122ecb25423d010d7f60002` 的 Windows MSVC all-feature/all-target tests；helper target 为 0 tests。后续 Clippy 修正仍需在新提交重跑。
- Windows MSVC Rust/Cargo 1.99.0 的第二次 CI 已通过 fmt 和 775 项测试，Clippy 在 9 处检查失败；对应修正已完成，等待重跑。all-bin build/no-default-features check 被跳过；Windows GUI、真实音频设备、真实 VST、Release 打包仍未验证。独立分支 `ci/windows-reliability-20261009` 已推送源码与文档，提交 `0146efd6cdc4832a3eaa9f6e99a252913c592144`。工作流已发布于 `90226a5e01e1f30299fb716fb1dd129006a9fd8d`，前两次 Windows CI 已运行；未发布 Release。

## 源码已经具备的能力

- `src/playlist.rs::create_audio_crossfade`：两条同轨、非嵌套重叠 Audio Clips，按重叠长度设置淡出/淡入；工具栏及 Clip 菜单在 `src/app.rs` 接线。`src/clip_fade.rs` 的等功率 envelope 供实时音频与离线导出复用。真实媒体、TempoMap 与鼠标回归仍待执行。
- `src/master_capture.rs`：有界 SPSC 和后台 PCM24 WAV 写入、同步后无覆盖发布、连续性诊断；`src/audio.rs::capture_rendered_master` 捕获已渲染 Master，`src/app.rs` 提供启动/停止/结束写入状态。
- Master Capture 是实时输出录制，**不能据此宣称 VST 离线 bounce、自动 tails、stems 或实时/离线等价已完成**。离线 exporter 对可能遗漏的启用插件/sidechain 仍会阻止导出。

## 本轮已完成的源码修改

1. 存储可靠性：`src/model.rs` 已增加保存前有限数检查与错误清理，避免 NaN/Infinity 将原文件替换为不可回读 JSON；新增四项测试（含 60 个字段/值组合），已包含在上述 775 项通过结果中。
2. Playlist 编辑可靠性：已在 `src/playlist.rs` 加固组缩放最小边界、Slip 数值溢出与循环半开区间、Audio Slip 极值以及 Fade/Crossfade 非有限终点；新增边界回归测试，已包含在上述 775 项通过结果中。`src/app.rs` / `src/playlist.rs` 的 Playlist 手势快照也已扩展为 Clips、Automation lanes 与 Clip mixer routing 一起恢复，补充 Undo/Redo 测试。
3. 构建验证：已通过第二次 CI 的 fmt 与 tests；9 处 Clippy 问题已修正，完整门禁待重跑。运行证据见 [CI #2](https://github.com/wrench1997/DAW/actions/runs/37871292792)。
4. 音频/导出安全：`src/master_capture.rs` 已修复停止时最终队列帧可能变成静音的竞态；`src/export.rs` 拒绝超出 8000..=192000 Hz 的采样率与非有限音频数据，替代静默裁剪采样率。新增四项测试，已包含在上述 775 项通过结果中。
5. 文档同步：README、能力矩阵、构建文档、开发路线图与工作日志已同步。独立静态审查完成；它不替代编译、测试或实机验证。

本轮新增 15 个 `#[test]` 标记：存储 4、导出/捕获 4、Playlist/手势历史 7；总标记数从 760 到 775。源码计数本身不是执行证据；本次通过数另由上述 CI 日志建立。Automation point 拖动已显式开/关事务，分割、删除、静音、点插入/删除改为独立提交，避免依赖延迟通用快照。

## 已识别但未修复

Audio Clip 在淡入/淡出内部切分时，当前 split 会复制归一化 fade 到两段，无法保证原 envelope 完整保持。精确修复需要 envelope 原点/范围信息及模型、迁移、实时/离线渲染和 UI 协同；不能把该缺口列为本轮已修复。

## 下一步与验收

完整优先级、完成条件见 [开发路线图](docs/DEVELOPMENT_ROADMAP.md)；逐次变更与执行证据见 [工作日志](docs/WORK_LOG.md)。构建和发布流程见 [构建文档](docs/BUILD_AND_RELEASE.md)。旧环境交接原文保存在 `docs/HISTORICAL_DEV_STATE.md`，仅用于追溯，不能替代当前验证。
