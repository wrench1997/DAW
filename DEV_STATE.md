# 当前开发状态

更新：2026-10-15（Asia/Shanghai）。Citrus Studio 0.5.0-alpha.1 仍为开发版。
开发分支：`ci/windows-reliability-20261009`；本轮起点：`b0d8d5c0`。
当前重点是 Windows 上可复现的“写旋律 → 播放 → 保存重开 → 导出”。
完整流程尚未验收；源码、单元测试、短工程实测和声卡监听分别记录。

## 当前支持边界

| 路径 | 已实现 | 当前结论 |
| --- | --- | --- |
| 实时 Master 录音 | 录制实际播放经过的 Master 输出 | 已有 NOIRE Pure 录音及用户试听确认；不等于离线导出 |
| 选中 MIDI 片段转音频 | Windows/Linux 实验 helper、状态捕获、无覆盖发布、取消和历史恢复 | NOIRE 实测仍被两次 opaque 捕获差异明确拒绝；原 MIDI 保留 |
| 整曲离线 WAV | 普通内置音源；实验模式下单 VST 的完整编曲、静态路由及 Master 自动化 | 短工程及原 405 音符整曲的后台离线导出通过；效果器链和通用图 PDC 未验收 |

普通构建不静默降级导出活动 VST 的错误声音；不支持的图和自动化明确拒绝。
实验整曲路径要求 `experimental-fresh-helper`、单个活动 VST、无 insert 效果器。
Master 音量/声像/静音自动化已接线；尾音须明确确认固定窗口。
未知或无限插件 tail 保留提示，不能把固定尾音窗口称为完整自然释放保证。

## 普通插件扫描的清理与失败恢复（本次提交）

普通 Windows/Linux 构建的默认 class 扫描现已复用实际子进程/管道所有权与取消机制，保持原 `None` class、48k/Q128 配置和 5 秒响应窗口；一次加载，不自动重播请求。
只有元数据成功且 helper 自然成功退出、实际 reap 和管道完成、没有清理错误时才标记 verified。已清理的单插件元数据失败保留 Unknown/unverified；取消、资源占用、清理未完成或异常退出则停止整次扫描，保留旧索引、rescan 标志和磁盘缓存原字节，错误在 toast 消失后仍可见。
清理 Pending 保留原 owner；再次扫描只推进该 owner 的清理，实际 Reaped 前不创建新 helper。普通 Probe 和实验 strict client 共用类型及 transport，严格渲染的实验门、class 选择、状态检查及块大小未放开。

Source57 实际通过 306 项针对性 Rust 测试：默认插件后端 187、扫描/App 5、默认 Probe 12、普通 transport 45、strict client 26、strict backend 31。默认/all-feature/no-feature 的 root `Clippy --all-targets -D warnings`、普通双 bin 构建、精确 `vst2,vst3` 配置检查及 root/vendor 格式检查通过。实际依赖清单验证 483 packages，root 仅 `vst2,vst3`，vendor 不启用实验 feature；这不是 release 预览包验收。
Source58 仅修正测试辅助函数返回值及 Unix 夹具的分平台错误文字，生产行为不变；当前默认 Clippy 和 12 项 Probe 回归再次通过，原清理断言保留。SDK 的 `-D unused_must_use`、最终 root/vendor/独立夹具格式检查通过；该 SDK 检查保留既有样式警告，不宣称整个 vendor 零警告。完整 vendor 补丁实际重放 73 个源文件逐字节一致；正确 discover 入口实跑 170 项打包测试全部通过。

首次门禁的 `result_large_err`/`collapsible_if`、测试 `err_expect`、离线 metadata 缺缓存、错误 unittest 导入入口和额外 rustc 参数多 target 拒绝均保留原始失败记录；已分别修正源码或调用方式。strict 取消/断管道夹具的 `OSError: [Errno 22] Invalid argument` 及 vendor 既有平台警告也保留，不当作 NI 实机结果。
本轮没有启动 Kontakt、桌面或声卡。新扫描在真实 NI、Linux/macOS 及播放时的表现仍未测；Source52 原曲音频结果属于此前冻结源码。作品、状态、WAV、日志、构建及完整本机收据仍在 Git 外，初始三个受保护文件和 Saved13 原工程本轮未改。
证据：`target/backend-reliability-20261014/source57-scan-*`、`source58-*` 和两个 provenance 目录；只提交源码、必要回归及文档。

## 本轮已实现的功能

- Channel Rack、钢琴卷帘和 Playlist 的视图布局、缩放、定位/放置与按钮反馈已调整。
- 钢琴黑白键几何、按下/松开及播放反馈、键盘试听和试听取消已接入实际状态。
- 播放启动与循环重启等待 Timeline、参数和插件实际确认，避免错误首音/旧 epoch。
- Master 音量/声像/静音控制和自动化进入实时及离线路径，保留两条路径各自的验证。
- Kontakt 实时输出选择以实际总线布局为依据，保留声明拓扑并拒绝不确定的布局。
- Windows 插件窗口初始尺寸/可见性、关闭/重开、modal 生命周期及 fault 后打开指引已修复。
- Windows 片段转换使用真实异步 named pipe 与原生文件身份/无覆盖发布；不靠去掉平台判断。
- 片段转换与整曲导出共用停止/捕获所有权，保留失败、取消、保存和历史操作屏障。
- 新 fresh source 的 pending/running/失败/强制退出 guard 保留至实际 Join，见下方回归。
- 新选定 Kontakt 元数据请求、完整输入加载及测试 owner 基础已应用；门禁见“最新源码”。

上述 UI 与后台源码变化不代表最新构建已完成本机视觉、流畅性或完整歌曲验收。
现有快捷键、复制/量化/力度/和弦/撤销功能继续沿用；未以重新实现替代已有功能。

## 实机结果

| 项目 | 实际结果 | 限制 |
| --- | --- | --- |
| 扫描/空实例 | 本机 Kontakt 8.4.1 元数据、约 3 秒空实例加载及 MIDI/PCM 请求通过 | 空 rack 的静音不算 NOIRE 发声 |
| 插件窗口 | 用户看到 NOIRE Pure；旧构建实际打开、关闭、重开独立 Kontakt 窗口 | 新构建缩放/播放卡顿仍需复测；截图失败不能证明界面坏了 |
| 声卡监听 | 用户确认钢琴声“听到了，音量正常” | 输出为 `扬声器 (2- CB4ND HW 1/2)`，厂家 Icon ProAudio，具体型号未确认 |
| 保存重开 | 静音最小工程保存重开通过；Saved13 正常保存并在 native14 冷打开 | 静音检查不证明 NOIRE；未验证 Felt 和全部内部音色参数 |
| 试听文件 | 独立曲子有 MIDI 和实际 Master 录音派生试听文件 | 派生文件是实时录音，不是片段转换或整曲离线导出 |
| 短整曲离线 | Whole run02 实际通过并独立核对全部 PCM/WAV | 2 patterns、2 placements、2 beats；不等于原 405 音符整曲 |
| 完整原曲离线 | Source52 full-song run02 实际 exit0；原 405 音符/5 patterns/160 beats、782 条 MIDI 及全部最终样本审计通过 | 后台设备无关验证；不等于本轮声卡试听、新保存冷重开、转换撤销、Felt 或效果器/PDC 验收 |

## 完整原曲首次离线通过

Source52 使用原 Saved13、真实生产 App/controller/StrictBackend 和新诊断 helper，未缩短乐谱、改状态或使用替代 renderer。
实际得到 43983 个 Q128、662248 条完整流式观察，spool/state writer 均实际 Join；源 worker Join 一次，clone disposal/destruction 先于 typed report。
48k Float32 stereo WAV 为 5629714 frames、117.285708 秒、45037770 字节，SHA-256 `60f1c023b139114c10764e47b1b0d499f4f1a8f89184913e1528ce150076a396`。
独立审计比较全部 11259428 个最终样本，固定容差 `2e-6`，最大绝对误差 `3.72529e-9`；浮点 bit 差异如实记录。
实际音乐非零，非有限样本/削波均 0，最大同声道相邻差约 `.001802`；末 100 ms 和固定 3 秒尾窗为静音。
原曲保留 128–160 beats 的空 coda，尾部约 23 秒静音不当作丢音；不据数字检查宣称听感无爆音或未知自然 tail 全部释放。
原工程、音符、状态、历史、路由、自动化及二进制 pins 均未变。实际 sidecar 验证源加载原 591772 字节，捕获和 clone 加载的 591761 字节逐字节相同，不证明 NI 内部参数语义。
本轮总耗时 396.064 秒（test 395.07 秒），外部 280 秒观测阈值跨过后只记录 Pending，同一 PID 3496 最终 exit0，没有重启或扩大原预算；不能称总耗时/性能达标。
扫描本次通过不解释 source50 的超时；scan/source helper 独立严格 reap、音色名/所有内部参数、GUI、声卡、新保存冷重开、Felt、转换/撤销及效果器/PDC 仍未测或未通过。
证据及音频在 `target/backend-reliability-20261014/windows-noire-full-song-run-02`，保持在 Git 外。
结果中的 production_export 是发布时的阶段快照，其 audit pending 为 true；最终 result.sound_audit_passed 和独立审计均为 true，不改写已冻结的阶段证据。

## 短整曲通过与转换拒绝

Whole run02 使用真实 App/controller、StrictBackend 和 NOIRE source，未用 mock renderer。
84 BPM、48 kHz、MasterVolume `.85 → .35`、固定 3 秒尾音；源停止/捕获后实际渲染。
clone 1661 个 Q128 回复，自然 exit0、实际 reap/destruct 先于 typed Complete/发布。
最终 WAV 为 Float32 stereo，212571 frames、4.4285625 秒。
独立分析核对全部 425142 个最终样本，最大绝对误差约 `2.05e-9`，固定容差 `2e-6`。
非有限值/削波为 0，37 个 padding frames 为 0；末 100 ms 为静音，实际尾音非零。
浮点 bit 差异如实记录；该检查不保证听感无爆音，也不保证未知 tail 全部释放。

NOIRE 片段转换 run07/08 已实际取得短 WAV，但 production recapture 保护拒绝提交。
实际报错：`The second actual source capture differs from the consumed render capture`。
run08 的 NI component 177 字节不同；已知 host envelope 相同，期间无原 source 正数帧处理。
独立三次同源捕获仍连续不同；尚未解释 NI payload 含义，不能猜测 volatile 字段并删保护。
原 MIDI、工程、状态、历史及失败产生的证据保留；NOIRE 转换/撤销重做尚未实测通过。

## 当前回归及最新源码

| 检查点 | 实际执行结果 | 适用范围 |
| --- | --- | --- |
| source22 历史完整套件 | 1963 passed /0 failed；主程序 1932、helper/protocol 31，5 个 NI 用例忽略 | 历史冻结源码，不认证后续改动 |
| source29 fresh 生命周期 | 新增 12 项全部通过；相邻整组 50 passed /1 failed | 原 full-ring fixture 失败记录保留 |
| source31 修正夹具 | clip-glue 全组 51 passed /0 failed；两套严格 Clippy、fmt/include/diff exit0 | 历史范围验证；不等于后续完整套件 |
| 私有流式基础 | spool 24、state sidecar 24，共 48 passed；格式检查通过 | `target` 内 std-only 基础，尚未接真实 SDK/整曲 owner |
| source32 新代码 | 编译 E0433 共 4 处；该检查点未执行测试或 Clippy | 新测试遗漏 `strict` import；原日志保留 |
| source33 回归 | 编译通过；strict backend 29 passed /2 failed | 旧 IPC 断言及 Windows mock 写错误漏记录；原日志保留 |
| source34 root 完整套件 | 2015 passed /0 failed；主程序 1977、helper 25、协议 6/5/2，7 个 NI/显式本机用例忽略 | 该源码实际终态 exit0；后续未变的主程序/协议结果复用，不是 native 验收 |
| source34 vendor 未过滤首轮 | 574 passed /1 failed | 上游 Dexed metadata 外部素材缺失；原失败保留 |
| source34 vendor 单项排除续跑 | 575 passed /0 failed；仅 1 项 Dexed filtered，probe 0 tests；exit0 | 574 library +1 legacy helper；不是未过滤完整通过 |
| source34 fixture | check exit0；独立 fmt-check exit1 | `native_windows.rs` 三块排版，原失败和改前字节保留 |
| source35 已完成门禁 | fixture check/fmt exit0；Python 259 passed（package 170、extract 28、editor 61） | 本次实际执行；root 2015/vendor 575 仍属 source34 |
| source35 严格 all-feature Clippy | exit101：`result_large_err` 两处 | `src/bin/strict_metadata_bundle/mod.rs:50/67`，Err HostResponse 至少 160B；原失败保留 |
| source36 当前 helper/质量 | helper 25 passed /0 failed；严格 all-feature/default Clippy、root/vendor/include fmt、diff 全 exit0 | 内部 Failure 表示紧凑化后实际覆核；25 项不再重复计入 source34 的 2015 |
| source36 实际 Saved13 load-only | 1 passed /0 failed；body 0.40 秒，含编译 18.79 秒 | 只读原完整工程，不创建 SDK/NOIRE 实例，不等于 native 重开或导出 |
| source37 完整 owner | 针对性 14 passed；严格 Clippy exit101：`new_ret_no_self` | 原失败记录保留；模型/所有权策略测试，不构造实际 NOIRE worker |
| source38 当前 owner/质量 | 14 passed /0 failed，body 1.76 秒；严格 all-feature/all-target Clippy、root/include fmt、diff 全 exit0 | 构造函数改名、释放时先销毁 callback；248 项源码输入冻结，不等于原曲实测 |
| 私有文件审核 candidate02 | PCM/WAV 35 passed、state sidecar 28 passed | 实际 Windows 文件夹具；不证明 SDK、writer join 或 NOIRE 声音 |
| source39 原曲初始化夹具 | 8 项证据回归 passed；Clippy exit101，5 处 `cloned_ref_to_slice_refs` | 保留原失败；尚未运行原生源 |
| source40 修正及构建 | 同 8 项 passed；严格 all-feature/all-target Clippy、fmt/diff、正式 helper build 全 exit0 | 仅把路径和单记录列表改为借用；249 项源码输入冻结 |
| source40 完整原曲 init run01 | 实际 28.4428 秒、exit101；扫描报 5 秒 helper 响应超时 | 0 source/StateLoad/clone/Process/WAV；原 405 notes/160 beats、工程和二进制 SHA 未变 |
| source41 提交前回归 | 完整工程相关 34 passed /2 ignored；helper 35 passed；Clippy exit101：`write_literal` | 新诊断测试的一处格式写法；原失败保留，未启动 NI |
| source42 提交门禁 | helper 35 passed；严格 default/all-feature/all-target Clippy、fmt/include/diff、helper build 全 exit0 | 仅修正测试中的动态格式参考；完整工程 34 项复用 source41 未改源码，不是完整套件或原生发声验收 |
| source42 完整原曲 init run02 | 实际 57.10 秒、exit0；102 条观察、原状态加载确认、实际源 worker Join 一次 | 完整 405 notes/160 beats、历史/路由/自动化/文件和二进制 SHA 未变；无试听、保存冷重开、转换或 WAV 验收 |
| source43 流式基础迁入源码 | 115 passed；Clippy exit101，5 项新 decoder/样式/枚举大小问题 | 原失败保留；测试实际 1.26 秒，不证明 NI 吞吐或完整歌曲 |
| source44 流式基础修正 | 116 passed（1.27 秒）；strict default/all-feature Clippy、fmt/include/diff 全 exit0 | 257 源码输入冻结；固定 64×2KiB 池、64KiB 完整记录、健康背压与真实 writer Join 已测；尚未安装实际流式观察 sink |
| source45 全部流式模块首次门禁 | 198 passed（1.57 秒）；strict Clippy exit101，15 项未使用代码和样式问题 | 263 源码输入冻结；实例字典、状态 sidecar 和 stream sink 已注册为测试模块；原失败日志保留，无 NI/完整歌曲验收 |
| source46 基础模块提交门禁 | 200 passed（1.46 秒）；root strict default/all-feature/all-target Clippy、fmt/include/diff 全 exit0 | 移除无实际实例对应的 FinalSource；新增真实取消/recovery/同 writer Join 验证；263 源码输入冻结，无 NI/完整歌曲验收 |
| source47 独立全曲审计迁入源码 | 42 passed（1.14 秒）；完整 405-note/810-boundary/782-wire 输入检查及 35 项文件/PCM 回归 | 冻结 builder 字节保持原 SHA；音乐静音不能算 NOIRE 发声；未提供真实完整 raw/WAV |
| source47 真实流式接线首次编译 | exit101：`E0063`，完整歌曲入口缺 `WavExportOptions.sample_rate` | 274 输入冻结；未执行新 Rust 回归或启动 NI；失败和后续补齐 48k 配置分别保留 |
| source48 接线后 Clippy | exit101：未使用的 replay 测试 helper、raw reader 循环样式 | 编译已通过，尚未执行新 Rust 回归；原失败保留 |
| source49 接线门禁 | 235 stream +34 full-owner passed；3 个显式本机用例忽略；strict default/all-feature/all-target Clippy、fmt/include/diff exit0 | 默认上限、23 类回读、原分配与实际 Drop/recovery/Join 回归通过；独立总审计另 16 passed，无原生音频验收 |
| source50 完整原曲 native run01 | 实际 27.767 秒，test exit101；再次在生产 scan 的 5 秒 helper 响应处失败 | 源/clone/stream/WAV 均未启动；完整工程/历史/592KB 原状态及二进制 SHA 未变，不能算原曲导出通过 |
| source51 vendor 诊断源码重放/打包 | 固定上游重放 70 个源文件逐字节一致；apply check/apply exit0，Python 170 passed | 原收据解析 CRLF 误判保留；测试进程实际 exit0，未重跑，仅从原日志修正绑定，不证明 NI/运行时行为 |
| source51 首次诊断 Clippy | 实际 exit101：`dangling-pointers-from-temporaries` | 新 helper 回归中临时 Box.as_ptr；未执行 Rust 回归/NI，原日志保留 |
| source52 诊断修正门禁 | 39 helper +5 SDK +34 full-owner passed；3 显式本机用例 ignored，9 项质量/构建检查 exit0；Python 打包 170 passed | 仅修测试返回 Box 存活期，生产诊断代码未变；新补丁 70 文件完整实际重放 |
| source52 完整原曲 native run02 | 实际总 396.064 秒、exit0；完整输入、782 MIDI、全部 PCM/WAV、流式/状态 Join 审计通过 | 280 秒仅外部观测阈值跨越，不称性能通过；单 VST/无 insert/L=0/明确固定 3 秒尾窗，完整工作流仍未验收 |

source34 root 完整命令耗时 805.36 秒，主程序测试耗时 635.87 秒，真实终态已保存。
vendor 失败项 `internal::module_info::tests::reads_sdk_generated_bundle_metadata` 要求外部
`vendor/test_plugins/Dexed.vst3`；该素材缺失，生产读取正常返回 `Ok(None)`。
既有 [processor domains](docs/PLUGIN_PROCESSOR_DOMAINS.md) 已声明该单项素材排除边界，
不是本轮未过滤套件通过，也不是历史 exact CLI 证据；原未过滤失败保留。
source35 相对 source34 仅独立 fixture 的空白和 `MB_OK` 调用尾逗号变化，root/vendor 源码未变；
保留 source34 已执行完整结果，不为这次格式变化冒称重新执行完整套件。
source35 历史 MIDI 库存 49 payloads 核验 PASS，仅验证历史库存；Kontakt `--help` exit0，未 spawn 插件。
source36 相对 source35 仅 helper 内部 Failure 表示变化，wire、查询顺序与准入保护保持。
source38 新增显式完整工程测试 owner；未修改普通构建的 UI/音频行为，未重新跑 source34 完整套件。
source38 首次 runner 引用了旧 manifest，预检 exit1 且未启动 Cargo；修正 runner 后全部门禁通过。
source40 原生 run01 在独立空实例扫描时失败，尚未进入完整源初始化；不能归因为 NOIRE 状态加载。
source42 首次 runner 误引用 source41 manifest，预检 exit1 且未启动 Cargo；原记录保留，修正引用后门禁通过。
本次提交门禁对应 source42 helper/质量及未改的 source41 完整工程回归；source34 完整套件、source35 fixture/Python 仍为各自历史结果。
实际 load-only 保留原音符、状态、路由和自动化；所有这些检查均不构成新声学/NI 实测。

fresh guard 的回归覆盖安装入队拒绝、真实 callback 安装失败、晚完成、panic、引擎丢失、
New/Open 后身份复用、强制 App Drop 与早退 guard Drop；普通 legacy 行为保持。
Pending/超时或 OS 进程消失均不能替代实际 Join；创建回收线程失败也不报告成功。
source30 的 E0425 和 source29 的原组失败保留；source31 后来的通过不改写历史原因。

source32 的选定元数据 bundle 从源码计数上将 Q128 wrapper IPC 11 次减至 5 次。
149 次真实 SDK 查询/调用及顺序保持；尚无新 NI 实测速度、延迟或预算可行性结论。
完整输入加载器保留 Saved13 全部 405 notes、5 patterns/placements、160 beats 含 coda。
原 source 直接接 MASTER，channel `.55`、Master base `.7`、全局 `.7 → .56` 自动化保留。
完整工程 owner 已接入显式 Windows 测试模块，沿用生产队列、实际回调和 worker 确认。
保留全部 Project/history/dirty/saved path，仅允许源的会话 runtime_status 更新；source42 run02 已实际初始化原曲源。
类别按 `|` 分隔的精确 Instrument token 匹配；清理未完成时整 owner 留在失败 quarantine。
只有实际 join 完成后才允许释放；quarantine 的继续恢复接口尚未实现，禁止超时当成功。
完整状态比较放在阶段边界，避免每 8ms 复制原曲及历史；source52 现已执行并审计完整流式原曲导出。
既有观察/状态/回复容量和 35/280/300 秒期限未通过简单放大绕过。
独立 state-file auditor 首轮 23 项有 11 failures/6 errors，保留原记录；candidate02 已实跑 28 passed。
完整 PCM/WAV auditor candidate02 已实跑 35 passed：各 API 自身身份基线、短读后锁定失败。
这些都是私有文件夹具审核，不代表 SDK、写入线程实际退出或真实 NOIRE 长曲音频通过。
source44 已将控制/App/native 编解码、完整身份外层和 spool 迁入测试源码并实际检查。
source46 已注册并实测实例字典、状态 sidecar 和 stream sink；取消后保留原分配，实际 Join 后仍拒绝成功。
source49 已通过 23 类借用 tap、完整 owner Drop/recovery、逐条 typed replay、原始 PCM 派生及完整歌曲生产 driver 的针对性合并门禁。
默认内存观察器的限制和原始 prelude 没有提高、清空或重置；流式预算独立，成功高频记录不再复制 PCM/RecordKind。
source50 完整 NOIRE 实机入口实际失败于 scan，未执行渲染、收口及全部样本审计；源码接线和夹具通过不代表原生歌曲通过。
018cb92 已提交并推送 19 个相关源码、回归、文档及字节保留配置文件，覆盖 source49/50 已验证的整曲测试接线和独立审计。
当时另 5 个新写的 vendor 加载阶段诊断文件未验证，保留在工作区，没有混入该提交。
14 个本地作品及 target 验证记录、插件状态、PCM、二进制由现有忽略规则排除并保留。

加载定位的新源码已接入 31 个固定 SDK 阶段和 4 个 helper 阶段的 before/returned 标记。
仅已有精确启动 opt-in 安装回调，共用实际 load 序号和时钟；默认门关闭先返回，错误结果和原 SDK 调用/顺序/期限不变。
原始 HRESULT 只在实际取得时记录；probe view Release、错误 teardown 和 ExitDll 分开观察，不把 Drop 正文或 ExitDll 当作 FreeLibrary 完成。
TLS 已销毁时不产生诊断 panic。5 个库局部门回归和新增 4 个 helper 边界回归在 source52 已通过，Clippy/格式/构建及完整原曲实测通过。
source51 的旧 PowerShell 执行器策略拒绝、复制 runner 漏改 source49 清单导致的两次预检失败均保留；这些尝试均未启动 Cargo。
校正后的 source51 277 输入门禁报新测试临时 Box 指针错误；source52 保留原失败并绑定返回 Box 后通过。单任务 CPU 门禁与另一聊天的功能 UI 收尾已通知协调；不作为性能测试。
完整歌曲的新结果字段区分 owned_native_resources_joined 与 source_readiness_verified；无源的清理完成不再命名为音源 Join，扫描 helper 的严格 reap 仍未证明。

## 失败复现与下一步

- 转换：在独立已加载 Pure 工程停止 source，启动短片段转换；两次状态捕获不一致时明确拒绝。
- 初始化：Whole run03 在 source ready 前报 `qualification poll/pump deadline or bound exceeded`。
  18 条观察、0 capture/clone/Process、无 WAV；cleanup 缺实际 source Join，原因未确定。
  已补有界只读初始化诊断和 fresh guard 保留；不能仅提高期限后称问题解决。
- 完整源 run01：以固定 source40 私有 test/helper 和原 Saved13 运行初始化 selector，
  production scan 返回 `Timed out after 5s waiting for helper response (plugin may have hung)`。
  NI 自身两条初始化日志相距 3.151 秒，不代表整个宿主 LoadPlugin 已返回。
  待定位 LoadPlugin、Windows plugin_changed 与回复写入/读回；未放宽扫描或初始化期限。
  已新增 Windows helper 加载/回复计时，进程启动时仅精确值
  `CITRUS_VST3_HELPER_LOAD_DIAGNOSTIC=windows-load-response-timing-v1` 启用。
  记录实际返回、序列化、写入和 flush 结果；默认关闭，不输出路径、音色状态、参数或音频。
  新增 10 项纯诊断回归通过；run02 已对 Kontakt 实跑并初始化完整原曲源。
  扫描与实际源从各自 handler 进入至 flush 返回分别为 3.404 秒、2.367 秒，两个实际 PluginInfo 写入均成功。
  本次成功不能解释 run01 超时；同轮资源警告 `cannot resolve resource: resources_ENG` / `lib_ENG` 保留，未更改 NI 文件。
- 回归：source32 的测试 import 遗漏导致 E0433；source33 编译已通过。
  source33 两项失败分别为旧 `LatencySamples:1` IPC 断言和 mock stdout `OSError: [Errno 22] Invalid argument`。
  source34 修夹具后 root 通过；vendor 缺素材、fixture 格式及 source35 Clippy 失败仍保留。
  source35 格式与 source36 内部 error 修正现已通过对应门禁，不改写此前失败。
- 优先完成完整原曲离线验证、全部恢复和转换可撤销路径；再扩展效果器/复杂图 PDC。
- 新构建需本机复测窗口尺寸、按键与视图缩放、播放期间编辑器卡顿、尾音和监听。

## 证据与保留约定

原三处未提交改动：`src/main.rs`、`scripts/qa_windows_kontakt.py`、`src/windows_kontakt_qa.rs`。
先审查并备份后才编辑；两个 Rust 文件后续有格式/窗口/接线变更，不能称原字节始终不变。
原脚本保留；没有 reset、覆盖作品、改系统安全设置或擅自合并 main。
原作品、状态、音频与本机 receipt 留本地，不随源码发行，也不承诺干净 clone 自带证据。

本机证据目录（仅原验证机器）：

- `target/kontakt-noire-local-20261011`：最初 Kontakt probe。
- `target/melody-stability-20261011`：原改动副本、独立原生工程、保存/监听/录音。
- `target/backend-reliability-20261014`：冻结清单、真实终态、失败日志与独立审核。
- `target/backend-reliability-20261014/pre-push-docs-archive-01`：本次精简前两份完整历史原字节。

## 其他既有模块

既有功能和历史限定验收以专题文档为准；以下链接不代表本轮重新实测：

- [钢琴快捷键/鼠标](docs/PIANO_KEYBOARD_EDITING.md)、[范围与吸附](docs/PIANO_RANGES_AND_SNAP.md)。
- [多窗口](docs/MULTIWINDOW_WORKSPACE.md)、[紧凑布局](docs/COMPACT_WORKSPACE.md)、[FL 功能盘点](docs/FL_STUDIO_PARITY.md)。
- [媒体恢复](docs/PROJECT_MEDIA.md)、[离线导出](docs/OFFLINE_EXPORT_WORKFLOW.md)、[WAV 选项](docs/WAV_EXPORT_OPTIONS.md)。
- [插件时间与恢复](docs/PLUGIN_TIMING.md)、[状态边界](docs/PLUGIN_STATE_RESTORE_LIMITS.md)、[原生编辑器](docs/NATIVE_VST3_EDITORS.md)。
- [MIDI 路由](docs/PLUGIN_MIDI_ROUTING.md)、[真实电平表](docs/MIXER_METERING.md)、[构建发行](docs/BUILD_AND_RELEASE.md)。
- Windows 本轮支持与验收步骤见 [Windows melody workflow](docs/WINDOWS_MELODY_WORKFLOW.md)。
