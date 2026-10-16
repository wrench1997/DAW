# 当前开发状态

更新：2026-10-16（Asia/Shanghai）。Citrus Studio 0.5.0-alpha.1 仍为开发版。
开发分支：`ci/windows-reliability-20261009`；本轮起点：`b0d8d5c0`。
当前重点是 Windows 上可复现的“写旋律 → 播放 → 保存重开 → 导出”。
完整流程尚未验收；源码、单元测试、短工程实测和声卡监听分别记录。

默认生产Whole工厂的独立联合回归已本机首次通过：真实catalog/default加载→callback播放/停止→磁盘保存→原App销毁/3worker Join→新App磁盘Open/3ACK/opaque.625→默认工厂导出，完整96768帧独立PCM一致。新genuine exact8.69s通过，旧audit exact8.47s通过；strict Clippy/no-default/fmt均0。genuine逐tick确认两starter未覆盖，实际typed Complete/termination ACK/admission drained；严格子进程3次自然reap/max1/1005直接观测仍归旧audit，不混为新路径独立观测。原10分钟CI步骤新增显式exact及失败退出。6050d36首云quality38872515281/preview38872515243实际FAIL：旧clip等待和identity观察失败、旧catalog退出检查失败；新SDK/Clippy/PrintWindow和release/ZIP未到达。Linux fixture8/9+host2等独立通过，原日志保留；用户03/NI/声卡不动。

四文件窄修本机已通过276项定向（identity3/catalog5/observer258/clip10）及strict Clippy/no-default/fmt，独立6050源码候选不含正在开发的Generator自动化。身份测试显式按控制线程收集，默认全局observer不变；扫描精确保留原拒绝，实际强杀仍为UncleanRetirement，成功metadata健康门不变。受控child146748实际EOF原始行、强杀/reap/exit1均已消费；clip只补原5s阶段诊断，根因仍未证明。新修补云验收待跑。

后台联合测试已由代码`bdf6901418f89fe5d0f4ac763a498c432156dacb`实际云验收：普通catalog/default加载→无设备callback实际发声/停止→磁盘ManualSave→原App/3worker Join→新App磁盘Open/3ACK/实际opaque.625→Whole完整96768帧独立PCM。quality38863519248与preview38863519233均首轮SUCCESS：2552 root/18ignored、fixture8/9、builder11、显式App SDK4及strict门通过；新联合SDK云12.64s通过，96帧单次crop/768固定尾音/3strict自然reap/Effect实际Offline保持。Windows真实session2 PrintWindow PASS，非SKIP；preview372Python=360PASS/12条件SKIP、staticCRT release7m53s/ZIP/提取helper通过，upload=false。

首推3b79的文档闭环和旧near-MAX单测失败已窄修：精确源码链接保留发布白名单；测试只在原5s期限等待Waiting→Ready，Invalid仍失败，新增真实heldmetadata反例。原始四次联合失败、编译/lint与云失败均保留，详[完整流程实证](docs/WINDOWS_MELODY_WORKFLOW.md#sdk-melody-play-disk-save-cold-app-open-and-whole-wav-2026-10-16)。本轮是源码SDK/新App实例证据；NOIRE暖态Save、真实程序重启及监听仍待验收。不占GUI/NI/声卡，不替换用户03。

已推`89ef2cb32a7b722c16c3672ef6f53a67305cf68f`：单源串行FX Whole已接入既有Master音量/声像/静音自动化，按最终PDC裁剪后的输出帧计时，尾音保持N−1值；其它自动化/路由边界不变。修复正常metadata查询误报源变化，以App在原最终CAS前核验完整源Ready，Waiting只等待，保留原取消/超时/Drop及发布合同。本机81定向/双尺寸实帧/严格构建通过；云quality38846965192及preview38846965091均首轮实际SUCCESS：2551 root/17ignored、Serial19/边界7、显式旧静态+新Master SDK、strictClippy/no-default/双bin通过；Linux新执行fixture6/host2，Windows真实session2 PrintWindow PASS。preview371Python=359PASS/12条件SKIP、staticCRT release8m41s/ZIP/提取helper通过，upload=false→仅上传SKIPPED。详[当前范围与完整实证](docs/WINDOWS_MELODY_WORKFLOW.md#master-automation-and-source-checked-publication-2026-10-16)。

当前可用副本：桌面“Citrus Studio 新版测试03”，`D:\DAW-Preview\Citrus-Studio-20261015-03\Start-Citrus.cmd`。PAT/SONG、拖播放头、AUTO范围及Playlist/Piano/Rack联动已通过本机定向回归/真实离屏/Clippy；新v13工程请另存副本，旧01/02保留。见`docs/PLAYLIST_TRANSPORT.md`；此前联动63d2f99云quality和preview均实际SUCCESS。

Follow长度源码`b2e56dc`本机验收通过：新Draw/Paint默认Follow真实Pattern周期；实际resize/Length编辑转固定窗口，Inspector可恢复Follow。Project只读extent统一驱动几何、AUTO终点、Timeline及支持导出；复制/移动/Slip保留policy，split固定两片；旧工程/旧Rust调用和精确MIDI Take窗口保留。格式14写入、旧文件读取；03/02不能读新14，另存副本。用户仍测试03、这里只改代码；没有替换副本或启动主GUI/声卡/NI。

首跑`af28a1e`实际App2415PASS/1FAIL/14ignored（678.99s），新增24/25通过。Length输入6后Esc的完整Project不变性失败；egui双帧lost_focus令release帧提交取消草稿，修为只清该widget草稿。`b65e14a`实际12输入/focus全部PASS，含逐press/release/idle不变性、Enter/真实点击失焦提交与单步Undo；原Saved13只读/私有copy完整payload+源字节保留门1PASS/0.75s。随后Clippy真实101报预览参数8/7；改传既有Clip、复用同一clock helper，未放宽lint。

最终`b2e56dc`实际fmt/严格all-feature all-target Clippy/preview unit1/no-default all-target/test artifact/匹配双bin全部exit0；主68,142,080B/helper5,054,464B。8真实WGPU/无设备callback用例15.58s全部PASS、自然exit0，Root检查9张1498×936实际帧，增长/缩短、手动窗、Follow恢复和局部cursor均正确。首跑全suite峰值14.14GiB private/32.80GiB global触发软预算，后续未启动；定向续门复用未变通过证据，最终CPU峰值3.95/22.42GiB、guard=false。原失败日志保留；所有观察工作实际terminal，资源交回peer。云`f6678ae`实际quality38804721761和preview38804721840均SUCCESS：2481 root/14ignored，25新增全PASS；factory33/SDK15/Unix1、367Python=355PASS/12条件SKIP、静态CRT release8m24s/ZIP/提取helper通过。云session2真实PrintWindow前/后/重绘PASS，非SKIP；未上传附件、未新增用户副本。NI/完整原目标仍未完成。详`docs/PLAYLIST_TRANSPORT.md`与target内source116/117记录。

后台`0c4872c`已补既有strict helper的真实双声道Q128 Effect输入：独立Fx身份/完整bus与event、私有admission、实际latency/tail/async/native revision复核，修改状态/参数/配置即失效，同一child保留到实际reap。独立审查发现通用reload可清角色绑定，已修成sticky并补真实子进程反例/旧入口正例。云quality38813369205实际SUCCESS：Win/Linux各27项Effect回归及真实SDK4+生产helper1均通过；1280样本/35探针、延迟32/末tap160/自然reap一致；根2481PASS/14ignored、严格Clippy/双bin及session2真实PrintWindow通过。preview38813369197实际SUCCESS：368Python=356PASS/12条件SKIP、27 Effect、同2481 root、静态CRT release8m18s/ZIP/提取helper通过，无上传。额外本机vendor全test-target严格Clippy仍为75条既有glue lint失败101，日志保留；未用根Clippy通过抹去它。

已推送`b73b57a`的停止后Effect整链私有捕获与exact-f64恢复已通过云quality38824285643（2026-10-15T23:54:32Z实际SUCCESS）：2497 root/15ignored、真实near-MAX worker、Win/Linux各29 Effect与非f32 SDK4+helper1、Windows显式私有source SDK1、Root严格Clippy/双bin/no-default及session2真实PrintWindow全部通过。私有捕获绑定实际callback/epoch/admin fence，包含不可清除的`epoch_resets` ABA防护和Effect-only sticky溢出；旧Instrument/Save入口保留。源Gain0.12345678912345678精确恢复，源DSP0、原guard实际Join、state40、双声道3×Q128、latency32/末tap160及strict child自然reap均有实际证据。preview38824285597/job116727965422已实际SUCCESS（updated2026-10-16T00:04:46Z）；static-CRT source/archive单步及ZIP/提取后sanitized-PATH helper步骤均实际SUCCESS，upload请求false，因此upload步骤SKIPPED。

首个整链/f64云候选`689e0af`的quality38821625393与preview38821625349实际FAIL保留：Windows29 Effect、2496 root/15ignored通过后，新增`fetch_update`在Root严格Clippy报deprecated；Windows新source SDK/PrintWindow及preview release/ZIP未执行。`b73b57a`改用checked weak-CAS，保留旧工具链、原AcqRel/Acquire及旧Save/Instrument合同；新增near-MAX真实worker已在上述quality通过。未加allow或放宽失败门。前快照本机15新回归、Gain0.375 SDK及旧29/10/19组的证据不冒充新精度验证。

代码`b9f2709`的Whole静态单源路径已接通单Generator、唯一PostFader主串行路径到Master的有序FX；active仅全wet，inactive/bypass跳过DSP，分支/send/sidechain/offpath FX/其它source/FX路径active自动化/partial wet最早明确拒绝。保留原无FX Master自动化、Instrument和Occurrence合同；24M-frame有界双Vec保留原始前导、NoteOff、尾音与Q128 padding，逐strict stage实际reap后才开下一stage，累加DSP latency后只最终crop一次。strict raw保留finite超满幅；Whole按现有实时端点语义把PCM逐样本clamp到[-8,8]（含所有slot均inactive/bypassed的端点），再依序应用精确控件，最终只做一次Master tanh/最终CAS发布。健康Cancel只排空原typed ACK与clone清理，保留同一live源；engine loss/AppDrop等才接管原guards直至实际Join。源与各endpoint早前receipts持续重验；独立审查的P2预算续期已修为原300s与capture fence的较早deadline，并补held100ms真实边界回归。

代码`b9f27091fbf5dd91e70ee318dcd897434fb5f6cf`现已实际双PASS：quality38834734658 attempt2（2026-10-16T01:55:07Z）和preview38834734476（01:50:57Z）。2529 root/16ignored、新Serial8、旧identity2及genuine_app_observer258通过；Root严格Clippy/双bin/no-default、真实fixture6+host2、Windows显式private App source1与Whole SDK1均通过。Default/session2真实PrintWindow前后/重绘/交互/状态/清理通过，非SKIP。preview的static-CRT release、ZIP/PE/import/provenance及提取后helper通过；upload=false→SKIPPED，无新用户副本。

quality attempt1的旧factory32PASS/1FAIL（3s deadline、phase为空）完整保留；Root未改源码，仅单次同SHA重跑失败Windows job，不能称调度根因已证明或偶发故障已修。8d join观察失败、eae E0599及本机10定向PASS历史仍保留。完整实证与静态单源/全wet/唯一PostFader边界见`docs/WINDOWS_MELODY_WORKFLOW.md`和target内source121–123记录。用户仍测03，主GUI/声卡/NI未操作；通用路由/FX自动化/PDC/自然尾音和完整NOIRE流程仍未完成。

用户新增Playlist↔钢琴卷帘联动：共享Pattern数据/所有placement预览本来已有；现补Pattern双击自动打开对应Piano/Rack、主Pattern切换时一次性定位含notes通道、SONG位置按placement/offset映射本地游标，以及真实callback模式下Rack活动步。已有手动trim不自动拉长；Source114/81b2572实际225 UI/无设备callback全PASS（含新增9联动+1基线正反控制），严格Clippy/test artifact/匹配双bin exit0；9真实WGPU用例PASS，Root逐张检查4帧目标/共享预览/局部cursor/Rack活动步。Source115云quality38746810086与preview38746809594实际均SUCCESS：2391 App+52 helper+13协议=2456 root/14 ignored；factory33、367 Python=355PASS/12条件SKIP、Clippy、静态CRT release8分50秒、ZIP/提取后helper通过，云session2真实PrintWindow PASS。未上传附件。完整source115原始日志/终态保留。已独立交付03，主68,087,808B/helper5,054,464B，helper smoke exit0/reaped；未打开GUI/NI/声卡，格式仍13与02兼容。

`4102953`新云quality38740984624实际SUCCESS（factory33、2446 root/14 ignored、Clippy/build、Linux15+1及session2真实PrintWindow PASS）；preview38740984829的367 Python=355PASS/12条件SKIP、SDK4+15通过，App2380PASS/1FAIL/14ignored，标尺用例126行`!dirty`失败，release/ZIP未跑。同SHA质量该例通过不覆盖此失败。已定位fixture换掉startup demo后只同步history指纹、未同步保存基线，跨750ms检查便合法dirty；已只修fixture并加确定计时/真实编辑dirty对照，Source114实际通过，生产规则不动。原日志/终态保留source113记录。

历史01独立测试副本在`D:\DAW-Preview\Citrus-Studio-20261015-01`，桌面快捷方式`Citrus Studio 独立测试版.lnk`指向`Start-Citrus.cmd`。复制Source102实际匹配主/helper为普通独立文件，不占target/debug；既有`CITRUS_STUDIO_DATA_DIR`把布局/音频偏好/缓存/自动保存/录音隔离到该目录UserData，测试作品建议存Projects。副本helper无插件协议smoke实际exit0；未替用户打开GUI/声卡/NI。用户已实际使用并反馈Playlist不能连续拖播放头、歌曲长度不应受视图限制、SONG/PAT当前Pattern及范围反馈不足，这些反馈已在02修复并完成本机软件/离屏验收。它是本机MSVC调试版，不是正式发行包，不随附插件/音色/作品/系统DLL。Root保持不做CU/NI/声卡自动验证，不修改或停止用户副本；peer目前按用户在另一聊天的授权操作其VK3D界面，后台CPU单独协调。来源/说明/检查随副本保存在Build-Info.json、README和Helper-Smoke.log。

PrintWindow CI 修补已实现（首版`8f4f35c`）：独立检测OS、当前进程session、WTSActive、可见window station及input/thread desktop；Linux/macOS、session0或明确无交互桌面才退出77，CI只将该码映射为成功SKIPPED。API异常、坏数据、真实截图失败及检测后桌面丢失仍FAIL。环境检测和每次PrintWindow均在5秒子进程超时内kill/reap，native step另有6分钟上限；Summary逐项记录PASS/FAIL/SKIPPED与原因。未改代码检查、Rust测试、Windows构建和preview打包门。

本机真实失败进一步定位为测试目标错误：同一交互桌面，直接PrintWindow标准child Button返回false/纯黑，而自有STATIC顶层flags0/1均成功且真实非纯色；不以该失败判无GUI，也不清全局class style。现对经过PID/class/parent/GA_ROOT/几何前后验证的真实helper顶层容器执行原PrintWindow，再按MapWindowPoints裁取原Button真实区域；仍要求裁区非纯色及编辑前后像素变化，其它容器像素/诊断目标/API回退不能替代。最终完整Python363项＝359PASS＋4既有Unix条件SKIPPED（含editor80/80），AST/PowerShell解析/diff通过。实际本机session1/WTSActive/visible/Default匹配，最终完整native harness约6.14秒实际exit0，paint前后、repaint、交互、状态及三种cleanup全部通过，fresh重进程0后交还peer。最终代码`9bb3110`云端两流程已实际结束：preview SUCCESS（363 harness＝351PASS＋12SKIPPED（4 Unix条件、8因私有Saved13输入未随Git分发）、SDK4＋15、2333 App＋52 helper＋13 protocol＝2398 root/12 ignored、严格Clippy、静态CRT release8分11秒、ZIP完整性和提取后helper smoke全部通过，未上传附件）；Linux SDK15＋Unix1通过。quality真实FAIL：App2332通过/1失败/12 ignored，`plugin_catalog_transport_tests.rs:107`的`receipt.healthy_retirement()`失败，截图及后续门未执行，不冒充云截图通过/跳过。该用例在同候选preview通过，不覆盖quality失败；已reap但具体不健康字段未记录，100ms自然退出窗口只是候选原因，尚未证明。扩大范围诊断这个独立Rust问题已向用户询问；当时截图修补的云运行验收未完成。原始`ci-9bb3110-{windows-quality,windows-preview,linux-helper-readiness}-01.log`及`printwindow-final-*-actual-terminal-01.json`保留。环境首版`8f4f35c`两条旧run因最终修补push被concurrency替换，实际cancelled，不算通过。保留首次stale receipt拒绝及child capture真实失败，不用新成功抹掉。证据：`printwindow-ci-environment-handoff-01.json`、`printwindow-container-crop-handoff-01.json`、`printwindow-owned-target-probe-01.*`、`printwindow-local-{native,summary,terminal}-03.*`，均在target/backend-reliability-20261014。

同一代码在文档提交`666ce96`后的quality现已实际SUCCESS（run38706859200）：32 factory、SDK4＋15、2333 App＋52 helper＋13 protocol＝2398 root/12 ignored、严格Clippy、匹配双bin、14 helper harness、80 editor harness、fixture/无默认feature全部通过；Linux SDK15＋Unix1通过。云runner实际为session2/WTSActive/visible/Default匹配，所以正常执行真实PrintWindow而非SKIPPED，paint前后、真实Button裁区repaint、交互、状态和三种cleanup全部PASS。结合最终代码`9bb3110`的preview SUCCESS，当前截图修补的本机及云端真实验收完成；OS/无桌面跳过和真实失败/超时继续失败的分支有上述自动回归覆盖。原独立单测失败未修改、未降级，也未据一次重过宣布根因已修。保留`ci-666ce96-windows-quality-01.log`、`printwindow-docs-quality-{run,jobs}-actual-terminal-01.json`；收尾只更新此实证文档，复用相同代码的有效结果。

SDK-B短source资格入口已准备：以原Saved13的opaque/generic map为输入，只在私有副本生成两音/两placement，消费本次实际source worker的metadata与opaque ACK，再进入真实callback、stopped capture、生产Whole/Strict；原full Saved13仍保留独立CheckedScan/健康卸载门，不以catalog伪造。新增run-owned单clone两phase摘要仅复制已有snapshot返回，无新SDK查询、sleep、stream记录或readiness前提；None与观察到全0有别。11个源码文件rustfmt/静态diff通过，15新增非ignored＋1ignored NI入口。冻结`dd204d9`本机已实际编译：observer8和FullOwner16通过；short-model3通过/1失败，synthetic valid metadata在实际validate门被拒，未弱化门。unused_mut另待修；Strict36、Clippy、cohort和两轮NI均未执行。独立审查另发现初始source原始错误与receipt写失败可能丢失证据，原owner正复用有界诊断/fallback修补并补回归。runner130088/exec42591实际101，全部held children退出，fresh heavy0后已交还peer；峰值private5.002/global23.165GiB，未触预算。日志/终态：`source98-local-gates-01-*`、`source98-local-resource-handback-01.json`。source thread join与helper OS exit分开，缺失PID/exit为null，Root须另持真实进程handle补证，不用测试exit0代替。候选/配方：`target/backend-reliability-20261014/noire-sdk-b-short-source-candidate-01/`。这只是后台短资格，不替代普通GUI保存冷开/声卡监听/完整参数/Felt验收。

短资格repair-02现已静态完成：修正synthetic有效夹具把已声明input误标aux的错误，保持实际metadata门不变并增加aux拒绝对照；初始诊断保留原head64，另保留按原序最近8条实际失败及总数/截断，回归覆盖75成功后才出Load/Prepare/Metadata/SlotFault和超限失败；receipt Err/panic/写失败独立createNew fallback，二次写失败stderr保留原因。移除唯一Root unused_mut，short回归4→7；五源码rustfmt/完整patch重放/diff通过。冻结`a4850d6`后实际Rust编译通过、short7全通过；Strict36为35通过/1失败，旧cancel/original-deadline blocked-stage矩阵在等待marker时超时（stage DiscardProgressNotifications:1），保留的mock audit只有forced-reap exit1/disposal/drop，没有planned blocked标记，具体原因尚未证明。原owner正读实际fixture定位，不通过增大deadline/无信息重跑覆盖。Clippy/cohort/NI未执行。runner113380/exec64377实际101，所有held子进程退出，fresh09:51:15Z heavy0后交还peer；完整`source99-local-gates-01-*`保留。observer8/FullOwner16输入未变，沿用已通过结果。材料：`noire-sdk-b-short-source-repair-02/`（repair-01保留）。

Strict矩阵已补单文件局部诊断（未改生产/原2s、20s deadline和15s reporting预算）：目标stage未进入而同receipt先终态时，落盘实际primary typed failure、具体owner stage/request/delivery/limit以及独立cleanup，再保持FAIL；正常目标stage路径也在原断言前留真实outcome。新增确定性LoadPlugin拒绝先于planned Discard的真实mock回归，要求同child实际reap、reservation释放、无后续IO。只影响原失败exact矩阵和此新exact；其它35通过用例通用wait不改。冻结`bbd4c3a`的Source100已实际编译，两exact各1通过（0.35s/24.42s）；Strict Clippy实际101，唯一新错误是短QA的`chunks_exact(2)`触发1.98的`chunks_exact_to_as_chunks`。现改用项目既有`as_chunks::<2>().0.iter()`，保留完整双声道/finite前置检查，补frame0左右声道和完整帧加奇数尾部拒绝断言；两onset回归与Clippy待执行，未调MSRV/未加allow。artifacts/build/NI未执行。runner114200/exec42942实际1，19 held children全部实际终止，10:13:21Z fresh heavy0后交还peer；峰值private4.017/global23.549GiB，未触预算。保留`source100-local-gates-01-*`、`source100-local-resource-handback-01.json`、`strict-blocked-stage-terminal-evidence-01/`及`short-onset-as-chunks-lint-repair-01/`。原fixture15无startup ledger witness，尚不能重建根因；这次矩阵通过不证明此前偶发故障已修，不称Python慢或B引起。

Source101（`16f7ff5`）已实际通过changed onset2、严格all-feature/all-target Clippy、actual test artifact与匹配主程序/helper构建，runner70624/exec75399实际0，15 held children全部终止；复用Source100两exact及未变的observer8/FullOwner16/其余short5/Strict35，不冒充重跑完整套件。由实际Cargo artifact冻结`bin-source101-sdk-b-short-01`后，真实NI首轮actual101/56.70秒，body报`complete source initialization deadline: complete actual callback has not reached its exact paused Timeline epoch`。0capture/0clone/无WAV；原始records49/50显示1次未绑定capture/epoch的128帧paused OrdinaryProcess实际returned Ok，聚合排除该未绑定记录不代表0 DSP，也未证明非静音音频。worker Join完成，外层实际观察helper97680自然exit0；test47092/runner110816/exec21772全部终止，未强杀/未触预算，首轮失败后未启动第二轮，10:19:36Z fresh heavy0已交还peer。原owner正沿真实callback/Timeline epoch接线调查，不放大40秒初始化预算或去掉exact门。保留`source101-local-gates-01-*`、`source99-noire-short-native-round01/`、对应stdout/stderr/terminal及`source101-native-short-resource-handback-01.json`；source99是旧runner命名，实际源码明确为16f7ff5，尚未验收SDK-B起音/异步报告或普通工作流。

Source102归因代码（`79a32f6`）只补清理前固定只读Timeline/callback/worker epoch证据，不增加pump/SDK/DSP/命令，诊断失败不跳过cleanup。新增真实无插件compile→callback exact paused/同legacy barrier迟到命令及缺失身份/错误保留回归，各1已实际通过；严格Clippy、test artifact和匹配双bin实际0（runner20432/exec93994，18 held全终止）。单次NI归因actual101/56.99秒，仍0capture/0clone/无WAV；决定性字段phase=Faulted、desired/compiled/source generation均2且fingerprint相同、compile error为空，真实sync error为`Compiled generator route is not callback-ready: Generator channel 1 instance 1 has no callback-confirmed Running endpoint`。最终running1/pending0且worker settled，但callback尚无generation/graph exact/confirmed identity。FullParent在登记source pending之前已首次编译并逐条泵mixer命令，与这个早期missing route锁存故障吻合；原owner正按真实Pending→callback Running时序修复/回归，不清空所有Faulted或提高期限。helper143552自然OSexit0，test105916/runner133104/exec95516全部实际终止，无强杀/预算，10:41:39Z fresh重进程0已交还peer。保留`source102-local-gates-01-*`、`bin-source102-sdk-b-short-01`、`source102-noire-short-native-round01/`与`source102-native-short-resource-handback-01.json`及`source-callback-epoch-attribution-01/`。独立盘点另确认PAT跨第5小节补到第8小节已接同一算法且10项历史回归通过；旧Playlist Clip长度不会自动跟随，`FL_STUDIO_PARITY.md`已纠正文案，本机NOIRE跨界体验未验收。

已推送`7ab716a`两套CI均实际SUCCESS：quality run38719646323通过2352 App＋52 helper＋13 protocol＝2417 root/13 ignored，32 factory、SDK4＋15、严格Clippy/双bin、14 helper与80 editor harness、fixture/无默认feature和真实PrintWindow；Linux SDK15＋Unix1通过。preview run38719646340实际通过363 harness＝351PASS＋12SKIPPED（4 Unix条件、8私有Saved13未随Git分发；不是GUI跳过）、SDK4＋15、同2417 root/13 ignored、严格Clippy/无默认feature、静态CRT release8分10秒、最终ZIP及提取后sanitized-PATH helper smoke；未上传构建附件。历史9bb preview也是12 skip，已按原日志修正文案，不能把本机4 skip数量套到云端。NI入口保持ignored，绿色不冒充NI资格。完整`ci-7ab716a-{windows-quality,windows-preview,linux-helper-readiness}-01.log`和`source101-{quality,preview}-actual-terminal-01.json`保留；不涵盖79a归因代码或尚未完成的时序修补。

FullParent初始化时序修补已静态完成：保留原mixer-first/native-load顺序，130条初始mixer命令只复用原callback-work服务，不在真实source Pending入队前运行Timeline控制轮询。普通成功GUI Open本来同步登记source再返回周期poll，本次不改生产严格路由门。新增真实compile复现旧sticky Faulted及真实held-prepare worker的Pending→callback Installed→exact paused回归，FullOwner共20项；原deadline/pump次数规则不变，源不存在时的mixer泵不能预热NOIRE。两文件完整重放/rustfmt/diff通过，Rust/Clippy和修后NI待实跑；材料`source-admission-mixer-order-fix-01/`。用户独立测试期间只做无NI软件门禁，NI与声卡实测等用户结束，不替换其副本。

用户已授权一并修独立catalog退休测试。当前单文件诊断候选保留原五场景及全部健康/失败/强杀/reap断言，新增真实scenario/elapsed/原错误/stopped/完整lifecycle输出，另补Error后等待EOF再延迟退出必须强杀/reap且绝不健康的真实子进程回归；catalog共5项。错误terminal分支的100ms自然退出与Python正常teardown是已确认的边界差异，尚不能重建旧CI失败的具体字段或据此宣称根因。未改生产/vendor/期限，格式/diff通过，下一门一次执行catalog5读取实际receipt再决定修补，不重跑掩盖。材料`catalog-retirement-diagnostics-01/`。

Source103（`3e711cc`）软件门已实际全PASS：FullOwner20（3.07秒，含两项因果时序回归）、catalog5（4.06秒）、严格all-feature/all-target Clippy、actual test artifact与匹配双bin。runner27256/exec80781 actual0，22 held children全终止，峰值private3.936/global25.004GiB。catalog原0/1/4实际61/67/61ms自然exit0、无kill/pipe/wait错误；新迟退Error约156ms实际kill/reap/UncleanRetirement且保留原拒绝。旧CI故障未重现，原owner正去除正常负面fixture对无关Python teardown的竞速依赖，保留100ms生产期限和强杀不得健康的控制。修后NI仍未执行。用户已实际打开独立副本PID14900（11:04:46Z），它在fresh清单中正常存活，不是Root遗留进程，未采用/停止。仅Root后台CPU窗口已交回，桌面/声卡仍由用户手工测试使用。保留`source103-local-gates-01-*`与`source103-local-resource-handback-01.json`。

catalog fixture修补已静态完成：仅原0/1/4场景在完整真实请求/回复/EOF断言与flush之后明确`os._exit(0)`，不再把Python完整finalization塞进terminal的100ms自然退出窗。健康Shutdown、exit9、sleep30、原健康断言与生产/vendor期限保持。新受控真实child对照给两条路径注册同一30秒atexit finalizer：协议完成直接退出必须健康，普通解释器退出必须保留原错误并实际kill/reap、不得健康，且stderr标记已进入EOF后finalizer。catalog仍5项。rustfmt/diff通过，改后catalog/Clippy未执行；材料`catalog-retirement-fixture-exit-repair-01/`。这是fixture不必要teardown依赖的修正，历史CI具体不健康字段仍未知，不称生产故障根因已证。

本轮Playlist/PAT/SONG在源码`885306f`完成本机定向验收。PAT绿/SONG橙，当前Pattern与实际播放范围、标题及标尺范围线一致；标尺按住连续seek、跨轨道不误Draw，松手后新点击仍正常放入Pattern。新UI工程AUTO按有效片段/全局自动化末端整小节推导，播放/Timeline/支持的导出共用；1/4/8 BARS/FIT只改视图，4096拍仍为编辑资源边界。已放Clip修剪长度保留，旧MANUAL及原Draw/import/record延长合同不改。工程格式13阻止旧版静默忽略AUTO。

**实测通过**：24项新增非ignored回归及1项显式Saved13只读/私有复制；原headless215中214有效通过、唯一短Clip失败经修补exact通过，未重复未影响项。model53、TempoMap12、AudioClip6、ClipFade8、native automation6、完整Audio split6及其余未变导出覆盖通过；FullOwner20通过。Source111严格all-feature/all-target Clippy、actual test artifact、匹配主/helper构建全部exit0。真实WGPU12项及修后mode/layout两个exact通过，Root逐张检查最终4帧：PAT提示无过期名，1280/1920逻辑屏宽的前景Playlist/AUTO真实可见、无toolbar裁切。所有观察到的Root工作已实际终止；无新CU/NI/声卡验收。

Saved13原文件1,298,729B/opaque591772B、5 clips/1 automation，format12/Manual160原样保留；新私有Auto副本派生128，两个副本全部persisted音乐payload相等且原bytes未变。generic parameters=0，这证明数据往返，不是NOIRE全参数/发声恢复。旧片段不会因源Pattern增长而全部自动拉长。

修补保留的反例：Source105缺Storage trait导致编译失败；Source106首轮runner的.NET null保留空env（207个capture绝对路径assert），只修runner；后续真实215项唯一失败为Inspector把合法0.125 Clip夹到0.25，已窄修并逐press/release/idle保留精确值。短Manual Audio tail编辑越播放map的问题用独立非实时几何时钟解决，沿真实后续tempo源、保留262144 checkpoint准入，不扩大歌曲；v10独立oracle按真实输出裁剪。v11移动继承淡出尾片的合法静音有exact-zero PCM/WAV及同source仅关淡出的强非静音对照，未降低原阈值。Source109/110的两个initializer Clippy错误已按原值修复，无allow/MSRV变更。完整失败/原报错/复现selector与资源记录保留于`playlist-transport-progress-history-01.md`及`source105`至`source111`对应原日志，不以新成功抹掉。

独立测试02已交付桌面“Citrus Studio 新版测试02”，目录`D:\DAW-Preview\Citrus-Studio-20261015-02`，Source111实际匹配68,053,504B主程序/5,054,464B helper普通独立复制，无插件helper smoke exit0/已reap；未替用户打开主GUI。独立UserData/Projects与旧01/作品互不覆盖，仍是本机MSVC调试版。新v13保存旧01不能打开，已有作品先另存副本。用户原生02实际操作/NOIRE试听仍未测。质量431af5d已通过；待文档闭包修补后preview的新实际终态。

前置catalog修补Source104本机5项通过，受控direct exit自然0/健康、普通exit真实finalizer后强杀/reap/不健康且保留原错误。旧后端`233f6f0`preview已实际SUCCESS：363 harness=351PASS/12条件SKIP、SDK4+15、2422 root/13 ignored、严格Clippy/无默认feature、静态CRT release8分45秒、ZIP/提取后helper全部通过，未上传附件；同候选quality早期factory31PASS/1FAIL的原3秒Deadline仍保留，根因未知。后续纯factory fixture隔离站点启动并保留有界phase，原生产/3秒/封印/清理规则不变，factory33本机通过，随后431af5d云quality也实际通过。

本轮证据均在`target/backend-reliability-20261014`：`source111-local-gates-01-*`、`source111-local-resource-handback-01.json`、`source111-playlist-{mode,layout}-render-01-*`、`playlist-transport-render-02-{mode,layout}/`、`user-offline-preview-20261015-02.json`。修补来源与精准测试见`playlist-transport-source-candidate-02/`、`playlist-inspector-short-clip-repair-01/`、`playlist-manual-tail-geometry-repair-01/`、`playlist-inherited-fade-silence-fixture-01/`及`factory-attempt-seal-fixture-01/`。CI原始结果见`ci-233f6f0-windows-{quality,preview}-01.log`及对应Source104终态JSON。

本轮首个云preview（`431af5d`/38737463544）实际FAIL：363 Python中350PASS/1error/12条件SKIP，打包清单漏掉`PLAYLIST_TRANSPORT.md`导致说明文档链接闭包失败；Rust/release/ZIP后续门未跑。本机先复现，再窄修两脚本纳入指南，增加真实ZIP/提取字节及provenance正例、缺失仍拒绝且无output反例，完整相关package179项实际PASS。未删链接或放松闭包/CI规则；原始日志/终态及`playlist-package-document-closure-repair-01/`保留。同SHA质量run38737463353已于13:08:24Z实际SUCCESS：factory33、SDK4+15、2381 App+52 helper+13协议=2446 root/14 ignored、严格Clippy、匹配双bin、helper14/editor80及无默认feature全部通过；Linux SDK15/Unix1通过。云Windows实际session2/Default/WTSActive/visible，真实PrintWindow前后paint及repaint change通过，非SKIPPED。完整source112-quality原始run/jobs/双job日志保留。现统一push文档闭包修补，再验新preview终态。

Source91（`2853db2`）已修复普通Open恢复门引入的撤销/重做回归：保留exact current-owner Ready/Completed证据，包括空表实际SlotReady；历史Project的session-only Unloaded不再抹掉同一worker的完成证据，真正pending/failed、新owner和clip restoration合同保持独立。原4项失败断言未改。本机实际generic8、startup history11、identity5全部通过，严格all-feature/all-target Clippy与匹配主程序/helper构建均exit0；格式/diff通过。Source92文档提交（`2bb9f80`）已推送，新双bin已在Native07实际普通冷Open→Whole成功；首音一致性仍待同源码重复验证。原始日志及实际Cargo终态在 `source91-local-gates-01-*-{stdout,stderr}.log`、`source91-local-gates-01-terminal-recovery.json`。

Source92两套云端流水线均已实际终态。quality通过32 factory、4 SDK offline MIDI、2329 App＋52 helper＋13 protocol＝2394 root /12 ignored，原11项startup history全通过；严格Clippy、匹配双bin、协议/harness、fixture及无默认feature门通过。整体仍为failure，仅独立native paint前/后PrintWindow失败（同进程按钮诊断也false）；control/gesture/dirty、state round-trip、lifecycle及Shutdown/EOF/crash cleanup独立通过，不据此认定本机Kontakt损坏。preview实际success：344 harness、4 SDK、同一2394 root/12 ignored、严格Clippy、无默认feature、静态CRT release、最终ZIP及提取后sanitized-PATH helper smoke全通过；未上传构建附件。归档绑定`2bb9f80`，不验收新候选B。完整 `ci-2bb9f80-windows-{quality,preview}-01.log` 与 `source92-*-ci-*-terminal-01.json`。

离线事件Source90（`8fc027d`）另修正确证SDK语义：普通MIDI原先在Offline也标kIsLive、PPQ恒0；现在只对host-generated普通MIDI，在实际DSP块/chunk、当前transport与最终offset确定后去live并计算PPQ，Realtime/Prefetch及显式PluginEvent字段保持。4项真实COM回归＋15项原HostEventList回归已实际通过；完整75文件vendor patch重放/显式pins及两条CI直接consumer已接线，Source91已包含该实现并完成上述Clippy/双bin及Native07。新候选只实测一次且首音与此前两轮不同，同源码重复一致性仍未测，首音修复尚未验收。

Source93（`63bf538`）已接入候选B：此前实际ComponentHandler不暴露IProgress，通知drain也没有typed active快照；现与HostApplication/processor共用有界typed registry，Strict独立clone在原deadline/cancel内等待async归零，再同锁arm exact async-start revision，每个实际DSP前后及后续块检查，变化即拒绝整个候选、不发布局部WAV。UI-only不挡，旧Realtime/未opt-in入口不新增前提，不跨插件调用持锁。静态审查发现unknown满队列拒绝和参数校验拒绝会丢失start，均已修成非UI fault latch并补两实际COM接口/合法UI对照回归；新增SDK15＋Root4。19源码rustfmt、2项Python合同、4-file bundle validation和完整76文件vendor重放/pins通过。该门跟踪IProgress报告的工作，NOIRE是否报告及实际首音仍需匹配新双bin验证；不与App generic ACK恢复混为一谈。实现/重放证据：`async-progress-readiness-candidate-01/applied-handoff-02.json`、`async-progress-readiness-provenance-02/`。

Source93最新云端已实际终态：Linux SDK15＋Unix ordinary no-respawn1全部通过；Windows quality通过32 factory、4 SDK offline MIDI、15 SDK readiness，App2330 passed/3 failed/12 ignored；preview通过344 harness、4 SDK offline MIDI、15 SDK readiness，App2329 passed/4 failed/12 ignored。新增Root4均通过。两套Windows共同的3失败是旧fixture末尾仍期待TailSamples/旧metadata顺序，没有计入必需snapshot/arm；preview另在held-reset用例成功等待后第二次取样得到Waiting、原期待Ready。本机legacy progress2通过，Strict32 passed/3 failed，Cargo101；后续Clippy/双bin未执行。诊断runner实际exit1、所有观察工作已终止后才交还peer，不称整套通过。完整 `ci-63bf538-{windows-quality,windows-preview,linux-helper-readiness}-01.log`、`source93-*-actual-terminal-01.json`、`source93-local-gates-01-terminal.json` 保留；Source92通过不覆盖本次失败。

Source94（`e50f92f`）修补上述夹具，未改生产readiness/取消/清理行为：Strict断言加入snapshot/arm边界；Whole helper返回结束等待的同一份真实Ready observation，两个直接caller消费该样本，不再把瞬时快照误当后续读取的lease。原held-reset、epoch、零capture、Project bytes及不发布/保留owner/实际reap断言均保留。兼容构造器仅加函数级dead_code说明/豁免；完整76文件vendor重放、4-file bundle、2项Python合同、rustfmt/diff通过。本机5项中4项已实际通过（取消、pending owner、两项Whole observation），严格all-feature/all-target Clippy及匹配双bin均exit0；dry-topology仍实际失败：occurrence1的初始prepare提前拒绝，不应期待post-snapshot，不能把后续armed metadata的边界泛化到它。首次runner exec90217实际1/Cargo101，剩余独立门runner exec46212实际0；各自收据保留在 `source94-local-gates-{01,02}-terminal.json`，不合称全通过。Source94云端现已实际终态：Linux SDK15/Unix1通过，quality通过32 factory/4 SDK offline/15 SDK readiness，preview通过344 harness/同4＋15 SDK；两套App均2332 passed/1 failed/12 ignored，唯一失败同为该初始prepare边界，Whole二次取样失败未复现。后续云Clippy/双bin/release/ZIP未运行，不能用本机结果冒充。完整 `ci-e50f92f-{windows-quality,windows-preview,linux-helper-readiness}-01.log` 及 `source94-*-ci-*-terminal-01.json` 保留，NI未测。修补证据：`strict-cancel-readiness-candidate-01/applied-handoff-01.json`、`strict-readiness-fixture-provenance-01/`、`whole-review-settlement-success-sample-fix-01.json`。

Source95（`92e9d3b`）仅修正dry-topology精确fixture：初始prepare在完整getter后比较capture，漂移即清理，不发post-snapshot/arm；occurrence2..5的armed metadata仍精确要求Snapshot(2*n)，raw-policy早拒不变。没有生产或vendor/pins改动；该完整矩阵用例1项和新输入严格all-feature/all-target Clippy均实际exit0，runner exec76910也实际0，rustfmt/diff通过。Source94未改的4项通过、双bin和SDK证据继续有效；5个受影响用例现均有实际通过证据。Source94构建的SDK-B双bin已按实际Cargo artifact尺寸/时间另存 `normal-noire-workflow-20261015-01/bin-source94-sdk-b/freeze-receipt.json`，尚未NI。所有观察工作实际终止、fresh重进程/本方root0后已交还peer；峰值private3.826/global23.161GiB，未触预算。新源码完整云端资格仍待取得；先保留Source94两套实际终态再发布下一候选。日志/终态/资源收据：`source95-local-gates-01-*`、`source95-local-resource-handback-01.json`；候选依据在 `strict-dry-topology-prepare-boundary-candidate-01/`。

Source96（`1687ec5`）最新quality已实际终态：32 factory、4 SDK offline MIDI、15 SDK readiness通过，2333 App＋52 helper＋13 protocol＝2398 root通过/12 ignored；此前4项失败及完整dry-topology矩阵均通过。严格Clippy、匹配双bin、全部后台harness/fixture/无默认feature门通过；Linux SDK15＋Unix no-respawn1也实际通过。整体仍为failure，仅native paint前/后PrintWindow及同进程按钮诊断返回false；attach/resize/DPI、modal IPC、control/gesture/dirty、stopped-edit state及Shutdown/EOF/crash清理独立通过。本机NI不能用该云截图失败判坏，SDK-B的NI仍未测；preview随后实际success：344 harness、4 SDK offline/15 SDK readiness、同2398 root/12 ignored、严格Clippy、无默认feature、静态CRT release、最终ZIP及提取后sanitized-PATH helper smoke全部通过；release耗时8分23秒，未上传构建附件。完整 `ci-1687ec5-{windows-quality,windows-preview,linux-helper-readiness}-01.log` 和Source96实际终态JSON保留。

此前Source89（`48ea977`）两套云CI均App2325 passed/4 failed/12 ignored，8项新恢复用例通过；quality32 factory和preview344 harness通过，后续门未跑。Source90本机又实际复现同4失败（startup7 passed/4 failed，Cargo101），这是Source91修补的直接依据；完整失败日志/收据保留，不被最新定向通过覆盖。两轮诊断wrapper分别误等conhost和MSVC VCTIP基础设施，在确认产品Cargo/测试/build终态后仅中断本方wrapper，两次wrapper均实际exit1；独立fresh检查重进程/root/console0后交还peer。Source91五个Cargo实际0，不把wrapper1说成整套runner0；50次资源采样峰值private4.076GiB/global23.665GiB，未触预算。下次observer须排除console及已确认的VCTIP基础设施，不改系统设置或终止无关进程。原脚本与 `source{90,91}-local-gates-01-terminal-recovery.json` 保留。

Native08已实际验证旧Source87冻结双bin的普通冷Open→Play/Escape→不Save/不editor→Whole：同验证02、iCON48k/XRUN0，播放采样位置002:02:02后停止归零；原生Save进入新目录后实际发布 `native08-play-only/Untitled.wav`。独立PCM审计为10.500秒/504000帧/48k双声道PCM24，8起音窗非零，无削波，末20ms和最后帧为0。首音−100dBFS阈值frame394/8.208ms，比05晚256帧；首400ms对应位移后差0，差异限frame129..33151，其后及尾音逐样本相同。未试听新WAV；不把固定3000ms窗当成完整自然尾音保证。

Native08 clone107928实际消费Shutdown，组件terminate返回0、ExitDll约839.011ms返回；Main消费至Windows run返回1098.569ms。正常Alt+F4后App83328/source76024/clone107928及observer exec5501全部实际exit0，fresh进程/窗口0后交还peer；峰值private2.222/global27.917GiB。此次成功只证明Play前置至少一次可用，Native04的Save因素和间歇失败仍未排除；偏好来自pre-Native06而非05启动前像，不称严格单变量因果证明，未用SDK-A/B或message-pass。证据：workflow内 `launch-08.json`、`cold-open-play-only-and-export-native08-01.json`、`stderr-08.log`、`normal-close-process-terminal-08.json`，新输出目录3份PCM/比较/trace审计JSON，上级 `native08-resource-handback-01.json`。

Native07已使用Source91实际匹配新双bin（含SDK-A、未含IProgress候选B），保持同验证02、无Play/新Save/editor、exact timing关闭、diagnostics1、48k/24-bit/固定3000ms尾音。普通GUI实际新建 `native07-offline-midi/Untitled.wav`，10.500秒/504000帧/3,024,044B；8个音符窗口非静音、有限、无削波、末20ms为0。App3772、source1872、clone2640及observer1340均实际exit0，fresh进程/窗口0后交还peer，峰值private2.309GiB/global23.977GiB。首音−100dBFS阈值为frame266，比Native05晚128帧、比Native06早256帧；对应首400ms重叠窗口位移后差均0，差异最晚至frame33151，其后及尾音完全相同。保留冻结双bin做同源码重复验证，不加固定warmup/延迟补偿抵消；暖态、试听、全参数/自动化和Felt仍未验收。证据：独立workflow目录 `bin-native07/freeze-receipt.json`、`launch-07.json`、`cold-open-and-export-native07-01.json`、`normal-close-process-terminal-07.json`、新输出目录两份PCM审计/比较JSON，上级 `native07-resource-handback-01.json`。

Native06此前已完成关闭exact timing的普通GUI冷开整曲导出，未重建：复用Native05冻结双bin、同一验证02、无播放/无新Save、相同48k/24-bit/3000ms固定尾音，实际新目录 `native06-no-exact-timing/Untitled.wav` 输出10.500秒/504000帧。独立PCM检查8个音符窗口非静音、无削波、峰值−26.74dBFS、末20ms为0；clone40840、原helper89188、App110476和observer62451均实际exit0，fresh进程/窗口0后已交还peer。`PLUGIN_DIAGNOSTICS=1`保留，message-pass未启用；这证明一次成功不依赖精确时序开关，不解决Native04暖态播放/Save后的间歇退出失败。

两份真实WAV的逐样本比较发现新缺口：Native06第一音前400ms比Native05精确晚384帧（8ms），位移后差0；差异仅frame129..33147，其后音频完全相同。首音位置一致性尚未验收，原因正在沿clone准备/首MIDI/延迟补偿路径调查，不能归因为日志开关或宣布PDC完成。证据：同独立目录 `cold-open-and-export-native06-01.json`、`native06-no-exact-timing/ordinary-whole-native06-pcm-audit-01.json`、`native05-native06-pcm-comparison-01.json`、`normal-close-process-terminal-06.json`；上级 `native06-resource-handback-01.json`。

后台此前完整云端实证：Source87（`769525d`）quality实际32 factory、2321 App+52 helper+13 protocol＝2386 root /12 ignored全部通过，含2项数字输入和3项新失败证据回归；严格all-feature/all-target Clippy、匹配双bin、协议/harness、fixture及无默认feature门通过。quality仅独立native paint的前后PrintWindow失败，不把云截图失败当本机Kontakt损坏。preview已实际success：344 harness、同一2386 root /12 ignored、严格Clippy、无默认feature、静态CRT release、最终ZIP及提取后sanitized-PATH helper smoke全部通过；未上传构建附件。factory两条真实子进程路径均实际输出完整阶段，本次未复现旧Deadline，根因仍未证明。证据：`ci-769525d-windows-{quality,preview}-01.log`、`source87-*-ci-*-terminal-01.json`。

Source88文档提交（`25567fb`，生产源码仍Source87）的quality随后实际终态：32 factory与同一2386 root/12 ignored、严格Clippy及全部后台门通过，仍只独立native paint前后PrintWindow失败。保留 `ci-25567fb-windows-quality-01.log` 与 `source88-quality-ci-*-terminal-01.json`；本轮没有触发preview，不冒充新恢复候选已被编译。

Native05已完成一次普通GUI冷重开→NOIRE整曲离线WAV：相同Source87匹配双bin，正常Open Native04保存的独立验证副本，无NKI重载；实见4小节/16拍、8音/1clip、Kontakt Loaded、Channel→FX1、Icon48k/XRUN0。本次不播放、不重新保存，导出48k立体声24-bit、7.5秒旋律＋明确接受的3秒固定尾音。原生Save前确认默认目标不存在，实际新建 `Untitled.wav`；原文件保留，另以overwrite=false复制为 `First-Light-NOIRE-Pure-ordinary-whole-native05.wav`。这是真实普通GUI整曲导出，不是实时录制或片段转换。

实际独立PCM解码：504000帧/10.500秒、3,024,044B完整RIFF，8个音符起始窗口均非静音，无削波；峰值0.046038747（−26.74dBFS），末500ms峰值约−119.39dBFS，末20ms及最后一帧均为0。最大相邻样本差0.002964仅作线索，尚未试听这份WAV，不能据此宣布无爆音；未知/无界插件尾音警告保留，不把3秒固定窗推广成任意自然尾音已验收。

Native05 clone112960实际Shutdown/teardown/ExitDll均返回（ExitDll约751ms），准确held handle确认clone、原helper84196及App69668均exit0；正常Alt+F4后observer66907实际exit0、fresh heavy=[]，资源已交回peer。该次启现有exact timing诊断，未启message-pass；与Native04相比还改变了播放/Save前置和冷开状态，不能称间歇清理失败已修。关闭exact timing的对照已由Native06实际完成；暖态播放/保存仍待单轴验证。完整参数/自动化/Felt恢复、片段转换的177字节recapture差异、效果器/PDC和编辑手感仍待完成。证据在 `target/backend-reliability-20261014/normal-noire-workflow-20261015-01/`：`cold-open-and-export-native05-01.json`、`ordinary-whole-native05-pcm-audit-01.json`、`normal-close-process-terminal-05.json`；资源收据为上级 `native05-resource-handback-01.json`。

Native04实际普通GUI导出已越过自动取消故障：匹配all-feature主程序/helper在唯一jobs1构建exit0后，从实际Source80普通Save复制的独立 `First-Light-NOIRE-Pure-validation-02.citrus` 冷Open，无手动NKI重载；Kontakt Loaded、Channel→FX1、2304spl、Icon48k/XRUN0。SONG游标经过8音片段，普通Ctrl+S显示Saved且精确保存副本路径；本轮无新监听回复。实际Song Length从64改成16（只输入首位1后clamp，不能称逐位16输入已验收）；尾音通过真实文本输入3000/Return，Review实见3000ms。保存审计仅song_length/plugin_instances变化，8音/1clip/所有路由不变；generic参数仍0，opaque591683→591681不等，不称全参数一致。该新副本随后在Native05实际冷开。

Native04 Whole在48k/24-bit/peak attenuation、明确接受固定3000ms尾音后，普通Save返回即启动clone41712并进入 `Rendering full master / disposing clone`；其后无Escape/Stop/Cancel。后续显示 `Clone cleanup remains pending; ownership is retained`、`Strict clone required forced termination`、`Cleanup grace elapsed before termination was acknowledged`，终态 `Strict clone did not terminate gracefully`。无WAV，时长/尾音/爆音PCM验收未做。UIA文件名显示新完整WAV路径，宿主实际进度却为同测试目录 `Untitled.wav`，暂记工具/原生对话框提交差异，不认定产品文件选择故障。正常Alt+F4后App23408/原helper112504均实际exit0，clone41712实际exit1已回收；被动observer4724实际exit0、fresh heavy0，资源明确交还peer。observer没有主动Kill，不代表产品clone健康退出。证据：`normal-noire-workflow-20261015-01/cold-open-and-export-attempt-native04-01.json`、`normal-save-model-audit-04.json`、`normal-close-process-terminal-04.json`，以及 `native04-resource-handback-01.json`。其后的ExitDll实证与下一判别步骤见Native05，原失败保留。

Source86（`1200196`，含Source85数字输入测试）quality实际提前失败：独立factory门31 passed/1 failed，`factory_and_default_load_share_one_attempt_seal_in_both_directions` 在 `default_class_probe_tests.rs:159` 的首次ProbeFactory返回 `Failure { kind: Deadline, delivery: MayHaveApplied }`。该轮root/Clippy/build未运行，不能用此前Source84成功覆盖。preview实际344 harness通过、App2317 passed/1 failed/12 ignored：Song Length新用例通过，tail用例在Review文本观察断言失败；helper/protocol、Clippy、release/ZIP未运行。完整日志及实际终态收据在 `ci-1200196-windows-{quality,preview}-01.log`、`source86-*-actual-terminal-01.json`。

Source87候选保留这些真实失败。factory首轮日志不能区分Python启动/import、收读/flush或父端pump/解码延迟；仅给原用例两条既有路径补stderr阶段、PID/耗时/原Failure及同owner实际cleanup，原3秒预算、单次请求、seal/cleanup和失败断言不改，不声称根因已修。完整vendor patch重放41 upstream→74 current sources、48变化路径/33新增/0删除通过，4项显式provenance完整性及常量测试1项通过；notes/pins同步，冻结时Rust回归未跑；最新quality结果见上。tail测试确定误读观察字段：egui的Role::Label把文本存入value而非label；候选改查实际Role/value并保留逐键、focus、Enter、真实destination options==3000全部断言；冻结时未运行，现已在quality通过。

Source87的strict clone修正只保留原有证据，不声称修好了NI退出：Whole错误框保留原primary首行，并展示backend/cleanup typed context及原cleanup明细；现有 `CITRUS_STUDIO_PLUGIN_DIAGNOSTICS=1` 下仅一次 `WHOLE_EXPORT_RENDER_FAILED`，不输出插件文本、路径、state或PCM。Shutdown失败标记 `dispose.shutdown`、forced-exit首错标记 `dispose.wait_for_exit`，首错优先和所有owner/fence/预算不改。新增实际App+mock renderer清理/准备失败、无payload/不造delivery诊断及cleanup首错不变共3项测试，并扩充已有hold_exit stage断言；现已通过两套CI回归/Clippy。Native05使用既有精确 `CITRUS_VST3_HELPER_LOAD_DIAGNOSTIC=windows-load-response-timing-v1` 后实际发布WAV，reader/main/slot-clear/SDK退出均返回，未启用会改变清理行为的 `SHUTDOWN_MESSAGE_PASS`；不把诊断观察当作退出修复。

最新云端实证：Source84（`42bbf9e`）quality 实际32 factory、2316 App+52 helper+13 protocol＝2381 root /12 ignored通过，含2项新增Fixture回归和此前失败的forced-App-drop；严格 all-feature/all-target Clippy、双bin、协议/harness、fixture及无默认feature门全部通过。quality仍只在独立native paint前后PrintWindow失败，其他native control/state/lifecycle/cleanup通过；不把云截图失败当作本机Kontakt损坏。preview实际success：344 harness、同一2381 root/12 ignored、严格Clippy、无默认feature检查、静态CRT release、最终ZIP完整性及提取后sanitized-PATH helper smoke全部通过，未上传构建附件。实机修复后NOIRE导出尚待执行。完整日志与实际终态收据为 `ci-42bbf9e-windows-{quality,preview}-01.log`、`source84-*-actual-terminal-01.json`。

此前Source83（`857d17b`）quality 实际32 factory、2314 App+52 helper+13 protocol＝2379 root /12 ignored通过；严格 all-feature/all-target Clippy、双bin、协议/harness、fixture及无默认feature门全部通过。quality仅独立native paint的PrintWindow前后捕获失败，同进程按钮诊断也返回false；control/state/lifecycle/cleanup通过，不把截图失败解释为本机Kontakt界面坏了。preview的344 harness通过，App实际2313 passed/1 failed/12 ignored：旧 `actual_whole_forced_app_drop_keeps_the_cancelled_capture_and_exact_source_until_worker_join` 在开始导出前的 `settle_initial_source` 失败，未进入该轮Clippy/release/ZIP。新增8项取消回归和同快照admin-gate安全回归在两套云job均通过，普通UI帧的完整mock WAV也通过。

Source84是测试夹具修补，不改生产逻辑：原 `Fixture::until` 在成功跳出循环后再次调用条件；瞬时Ready并不是下一次读取仍Ready的租约。修补只断言结束等待的同一次采样，原5秒预算、tick及清理owner保留；新增一次性成功与真实Ready后被foreign请求替代两项确定性反例，后者同时确认旧观察不能授权新请求。超时诊断补实际耗时/采样次数和settlement判定量。Source83原日志缺少这些量，不能据此断言那次失败必由双采样引起。Source84实际quality结果见上；完整工作流继续待验收。此前证据保留在 `target/backend-reliability-20261014/ci-857d17b-windows-{quality,preview}-01.log` 及 `source83-*-actual-terminal-01.json`。

Source85只增加2项真实App::ui数字输入回归：Song Length 64→16和Release Tail 0→3000，使用实际文本焦点、Ctrl+A、逐个数字键/文本事件与Enter，核对工程或Review中的实际值及无额外编曲/播放副作用；不以AccessKit SetValue或直接改目标字段代替输入。没有修改生产控件，目前未证明普通键盘输入存在产品缺陷。格式/diff通过，尚未执行Cargo/Clippy/native；Source84 preview实际结束后再推送，由云端执行新候选。本机仍交由peer，无新增Cargo/NI/CU占用。

Source80（`9f3b608`）把双类 catalog 构造提取成本测试模块私有的 `fresh_catalog_descriptor`，移除非法私有模块调用，不扩大生产API或删减断言。本机实际12项定向回归（backend5、App4、旧identity3）、严格 all-feature/all-target Clippy和匹配双bin构建通过。云端 quality 实际32 factory、2305 App+52 helper+13 protocol＝2370 root /12 ignored及全部后台门通过，仅独立 native paint PrintWindow失败。preview的344 harness通过，App实际2304 passed/1 failed/12 ignored：`stopped_epoch_safety_discards_inflight_old_output_and_rejects_queued_old_events` 在 `audio_stopped_epoch_safety_tests.rs:1048` 的 `paused_work_quiescent(SAFETY_REVISION,2)` 断言失败；该轮未进入Clippy/release/ZIP。新增9个fresh/default意图与backend用例在两套云job均实际通过，不以quality成功覆盖preview失败。完整日志和终态收据在 `ci-9f3b608-windows-{quality,preview}-01.log`、`source80-*-actual-terminal-01.json`；本机在 `source80-local-gates-terminal-01.json`。

Source80 匹配的独立bin通过普通GUI冷Open实际保存的 `First-Light-NOIRE-Pure.citrus`，没有手动重新加载NKI。原生窗口实见NOIRE Pure、此前调至左上的Color、1350×680完整界面；FIT NOTES实见8音，Channel→FX1、Icon48k/XRUN0。普通再保存成功，前一版本先以overwrite=false保留；新文件除plugin_instances外所有顶层内容一致（8音/1clip/路由均不变），generic参数两次均为空，opaque591680→591683字节不同，不能据此声称全参数/字节恢复一致。无自动化，新声卡监听回复和数字PCM仍缺。

真实整曲导出在48k Float32/Preserve level、明确接受实际0ms附加尾音后，选择新WAV路径并Alt+S，宿主报告 `Whole-song export cancelled; destination was not changed; playback remains paused`，没有WAV或clone helper。用户未点Cancel，Save返回后无Escape/Stop输入。源码确认普通App update每帧调用 `dispatch_armed_window_close`，原实现遇到clip/Whole fence就取消，未先检查真实close/exit intent。64拍范围输入16和尾音输入3000本轮均未生效，实际仍64拍/0ms，不记为通过。证据：`normal-noire-workflow-20261015-01/cold-open-and-export-attempt-source80-01.json`。

正常Saved后Alt+F4，事先持有的App19460/helper93060句柄均实际exit0；close observer16196和resource observer83574实际exit0、fresh heavy=[]后交还对等聊天。Source81源码已冻结：close dispatcher先检查exit armed；新增7项真实App::ui帧回归，覆盖普通帧保持Whole/片段capture、完整mock WAV、Escape/Cancel/Stop及迟到回执；原held-clone关闭用例也改走真实UI。首次被接受的取消保留来源/phase，终态可见，诊断开关下单行记录原会话/音源身份，不记录状态内容。另保留测试wait时实际满足原条件的同一完整快照，新增真实admin gate反例，不改生产安全条件或期限；原云端失败的具体变动字段未记录。格式/diff通过，两个修正仍待云端编译/回归及新实机导出。

Source81（`f807fb0`）两套云端job随后均在 root 测试编译报E0063：新 `ParameterEditFailure` 夹具漏填 `was_pending`（`whole_song_export_cancel_tests.rs:263`）；quality32 factory、preview344 harness通过，不记root测试/Clippy/build通过。完整失败日志及终态收据在 `ci-f807fb0-windows-{quality,preview}-01.log` 和 `source81-*-actual-terminal-01.json`。Source82（`098e3f5`）明确补 `was_pending=true`，对应已入队编辑的迟到超时；本机实际编译成功，新cancel组6 passed/1 failed，原held-capture→Escape→重复Stop用例期望Cancelled、实际Failed。runner89774实际exit1/Cargo101、observer35329实际exit0、fresh0后按约交回peer，其余旧回归/audio25/Clippy/build未运行，未启动NI/CU。证据：`source82-headless-targeted-01.log`、`source82-local-gates-terminal-01.json` 和 `source82-local-resource-handback-01.json`。

Source83修正该真实取消冲突：sealed Stop只在原actor pause仍是同engine/project的当前mailbox请求、且App/clock/callback均已停止时复用，不再无意义重发false使原 `transport_request_id` 失效。foreign pending Play或真实播放仍发布新pause，原stop-intent/快照/capture/cleanup fence不改。保留Escape+重复Stop原断言，并在Whole/片段检查原request不变；新增foreign Play反例，即使snapshot仍paused也必须覆盖，真正合同失效仍保留capture到ACK并报告Failed。cancel组8项在两套云job实际通过；严格Clippy及其他门的实际范围见本页最新结果，修复后实机导出尚待执行。

Source77（`da68aaf`）当时云端结果未通过：preview 的完整 root 套件实际 2364 passed / 12 ignored，含迁移后的 3 项 Windows backend 身份回归；随后严格 Clippy 在 `GeneratorReadiness` 的 208-byte Ready 载荷报 `large_enum_variant`，没有进入 release/ZIP 验收。quality 同一源码首先在 `actual_fresh_source_guard_drop_retains_late_worker_and_records_healthy_or_panicked_join_once` 报 `fresh Guard Drop lost its exact held worker`，随后出现保留夹具的 sticky 拒绝；最终 2162 passed / 137 failed / 12 ignored。预期注入的 backend shutdown panic 不能被移除以掩盖该失败。完整日志和实际 terminal run/jobs 收据分别保留为 `ci-da68aaf-windows-preview-01.log`、`ci-da68aaf-windows-quality-01.log` 及 `source77-*-actual-terminal-01.json`。后续修复及实测结果分列记录，不以 preview 单次套件通过代替并发稳定性；Source77 native 准备稿保持未执行。

Source78（`78d5986`）把 Ready 身份载荷改为 `Option<Box<LoadedGeneratorIdentity>>`，不添加 Clippy 豁免，getter 保留原返回类型和当前 owner 绑定。测试侧在原 5 秒断言前一次性输出 mock Drop/panic 展开阶段、同一 worker 的实际 is_finished/Join/observer 登记进度及原 observer/owner 状态；不新增 poll/Join/线程或延长任何期限。正常和注入 panic 两种实际 Join 结果也核对观测终态。生产编译不包含 runtime 观测字段与写点。本机已实际通过 21 项定向回归、root 严格 all-feature/all-target Clippy 及匹配主程序/helper 构建；云端 quality 实际通过 32 factory、2364 root / 12 ignored 及全部后台门，仅独立 native paint 的 PrintWindow 捕获失败。preview 已实际 success：344 项打包/协议 harness、2364 root / 12 ignored、严格 Clippy、无默认 feature 检查、静态 CRT release、最终 ZIP 及提取后 helper smoke 全部通过；没有上传构建附件。原线程清理超时本轮未复现，诊断没有证明其根因或宣称修复。日志及终态收据：`source78-headless-targeted-*`、`source78-clippy-all-features-*`、`source78-all-feature-main-helper-build-*`、`ci-78d5986-windows-{quality,preview}-01.log` 与对应 actual-terminal JSON。

Source78 随后在已协调的本机窗口实际启动独立数据目录和匹配的 all-feature 主程序/helper。GUI 新建空白工程、普通插件管理器扫描完成并发布 9 个条目；Kontakt 8.4.1 保持 Unknown/unverified、无 loaded-default metadata，catalog 含真实 Instrument 声明与 Compatibility 类。输出实际显示 `扬声器 (2- CB4ND HW 1/2)`、48000Hz/480fr/XRUN0；没有播放试听。明确选择 Channel01 并勾选实验 dry-bounce source 后，LOAD 被旧前置条件拒绝：`Rescan this VST3 to obtain its actual factory class before fresh-source preparation`。目录重扫不会提供运行时默认类，后续候选须在真正 None-class LOAD 后取得并校验实际类，再进入原完整 fresh prepare，而不是选 catalog 首类或移除 class 门。未加载 NOIRE，也未验证新工程保存、冷重开或导出。新空白 App 已正常 Alt+F4、实际 PID 消失且 children=[]；原作品未写入。证据在 `normal-noire-workflow-20261015-01/catalog-after-real-ui-scan-01.json`、`native-observation-and-close-01.json`。本机 Sky 入口已实际使用；首个索引点击定位错误，经激活与新截图坐标恢复，不能据此认定本机 Computer Use 不可用。

Source78 第二会话未勾 fresh，普通 None-class LOAD 实际打开完整 Kontakt 窗口，从已注册 NOIRE 库双击加载 Pure，实见 `NOIRE PURE` 与约315.4MB instrument memory。修改 Color 后正常关闭编辑器；钢琴卷帘写入8音/4小节，Playlist 放入16拍 Pattern。SONG 游标经过该片段，Escape 回到零拍；监听回复和数字 PCM 仍缺，不能凭游标称发声通过。普通 Save 显示 SAVED、无 stale 警告，模型实核8音/1clip、Unknown role、591680字节 opaque state、Channel→FX1 路由。仍为128BPM/64拍且无自动化；16拍整曲范围输入未生效，不称通过。

原生保存对话框坐标误报15×15，UIA set_value 显示目标完整路径，但 Alt+S 实际在旧对话框目录新建了 `desktop-song-14/Untitled.citrus`，未覆盖现有 native13/native14。该真实普通保存文件已无改动、overwrite=false 复制到新独立目录的 `normal-noire-workflow-20261015-01/First-Light-NOIRE-Pure.citrus`；审计见 `normal-save-model-audit-01.json`。事先持有准确句柄再正常 Alt+F4，App13528/helper36632 均实际 exit0，观察器结束且 fresh heavy0 后交还对等聊天。这个 runtime 退场结果不替代旧 component-scan ExitDll 失败。冷开、自动化及离线导出仍待验收。

Source79（`56e9c89`）只把 catalog 新实例登记为 LoadedDefault 意图：实际 None-class LOAD 校验真实 UID/version 后，再进入原完整 fresh prepare；旧 Exact、总线/首次Start timing、owner/restoration 和 captured CID strict clone 合同保留。新增5项跨平台 backend 测试及 App 无降级矩阵，格式/静态审查通过。实际云端 quality 通过32项 factory、preview 通过344项打包/协议 harness，随后两者均在 root 测试编译报 `E0603: module catalog_tests is private`，定位 `src/app/clip_render_glue_tests.rs:730` 的跨模块测试夹具调用；root 回归、Clippy、release/ZIP和实测 checkbox未执行。完整日志和终态收据为 `ci-56e9c89-windows-{quality,preview}-01.log` 与 `source79-*-actual-terminal-01.json`。Source78 普通 Pure 会话不能当作该修复验收。

Source76（`389f9af`）最新云端 quality：独立 factory 回归 32 passed；完整 root 套件主程序 2279、helper 52、协议 13，共 2344 passed / 12 ignored。严格 Clippy、双 bin 构建、helper 协议 smoke、无默认 feature 检查和独立 fixture 构建通过。原产品期限、状态、历史和 ownership 断言保留；Rust 1.97.1 发布工具链不变。
quality 整体仍为 failure：最后的 native paint 前/后捕获失败，同进程按钮的 PrintWindow 诊断也返回 false；control/state/lifecycle/cleanup 独立通过。这不证明 Kontakt 界面故障，也不是 GUI 验收通过。
Source75 preview 同样完成 2342 passed / 12 ignored，随后完成严格 Clippy 和无默认 feature 检查；静态 CRT release 编译期间被 Source76 提交触发既有并发取消。该轮没有 release 构建完成、最终 ZIP 或提取后 helper smoke 通过记录。
Source76 preview 已实际 success：2344 passed / 12 ignored、严格 Clippy、无默认 feature 检查、静态 CRT release 构建、最终 ZIP 完整性及解压后 sanitized-PATH helper smoke 全部通过。归档绑定 `389f9af`，不是后续 Source77 的发行验证；按现有配置未上传预览包，没有可下载的构建附件。
Source73 preview 已完成静态 CRT release 构建，随后包内链接校验失败：`Broken package document link: DEV_STATE.md -> docs/WINDOWS_MELODY_WORKFLOW.md`；该轮最终 ZIP smoke 和上传未执行。Source74 已将工作流文档补进必需清单，新增真实文档闭包、ZIP 复制/提取与缺文档拒绝回归；完整打包测试 175 passed。Source74 preview 最终 cancelled：在后续提交触发既有并发取消前，两个 whole-export 测试及 observer cap 测试已报告 FAILED，四个 verifier 测试超过 60 秒且未结束。原日志没有最终 panic 详情，不能仅凭用例名断言原因；该轮 root 测试、Clippy、release 构建和包验收均未完成。

Source73 本机新默认 helper/QA 实际构建 exit0。协调窗口内的唯一 factory-only NI 测试 exit101：`setHostContext failed: 0x80004001`，声明 getter 尚未执行。helper 自然 exit0、实际 Reaped、没有 kill 或 shutdown/wait/pipe 错误；这是该失败路径的清理通过，不是元数据或普通扫描通过。无组件/控制器/view、音色状态、MIDI/DSP、声卡或作品请求。
Source74 保留 context offer 和卸载前生命周期，仅接受该 setter 的 SDK `kNotImplemented`，其他 setter 错误及必需 getter 失败仍拒绝，通用成功判定未放宽。依据是 [Steinberg 官方宿主实现](https://github.com/steinbergmedia/vst3_public_sdk/blob/master/source/vst/hosting/module.cpp)：提供 context，但不把 setter 返回值作为声明查询前提。此修复已编译并通过上述云端回归。

Source74 新默认 helper/QA 本机构建均 exit0。唯一 factory-only NI 测试 PID2400 实际 exit0、wrapper0：返回 Kontakt 8.4.1 的两个真实声明，包含预期 Audio Module Class CID `5653544E694B386B6F6E74616B742038`。helper PID20596 自然 exit0、实际 Reaped、no kill，shutdown/wait/pipe 无错；test 和两条 pipe 完成，NoExit 宿主实际 exit0、fresh heavy0 后已明确交还窗口。这次没有 CU、GPU、声卡、作品、组件/控制器/view、音色状态或 MIDI/DSP 请求；不能当作 NOIRE 恢复/发声/导出通过。

Source75 的普通模块目录接线已提交并通过上述云端后台门禁。每个 path 一个条目，目录声明与实际运行验证分开，默认 LOAD/工程恢复仍传 None class，不挑首个声明冒充默认类。严格转换/完整工程 QA 保留实际类元数据门。Source78 已经实际完成生产 GUI 目录扫描并发布 Kontakt 条目；独立 ignored catalog selector 尚未执行，Source69 的组件扫描 ExitDll 问题也未修复。Source74 vendor provenance 已从原始 archive 实际重放 74 个源文件逐字节一致；本轮不占用桌面或声卡。

Source76 只修测试隔离和失败诊断：两个 worker-identity 测试及 cap 测试与 App 共用 observer admission；失败时原 worker/observer 仍保留并置 sticky failure。Verifier 排队现在可观察已保留 owner 的失败，拒绝后续夹具而不无限等待；两个确定性回归均从实际 WouldBlock 分支确认排队。两个 whole 失败用例新增即时 stderr，保留原 panic、断言、5 秒 Join/10 秒观察期限。最终候选已实际编译，五组针对性回归共 45 passed；格式与 diff 检查通过。该提交云端 quality 的完整并行 root 测试及后续后台门禁也已通过，整体仍因 paint 捕获失败；原 Source74 的缺失断言仍不能据此倒推。证据：`source76-fixture-targeted-02.log`、`source76-fixture-targeted-terminal-02.json`、`ci-389f9af-windows-quality-01.log`。

## 当前支持边界

| 路径 | 已实现 | 当前结论 |
| --- | --- | --- |
| 实时 Master 录音 | 录制实际播放经过的 Master 输出 | 已有 NOIRE Pure 录音及用户试听确认；不等于离线导出 |
| 选中 MIDI 片段转音频 | Windows/Linux 实验 helper、状态捕获、无覆盖发布、取消和历史恢复 | NOIRE 实测仍被两次 opaque 捕获差异明确拒绝；原 MIDI 保留 |
| 整曲离线 WAV | 普通内置音源；实验单VST；单Generator→唯一PostFader主路径→Master有序全wet FX及Master音量/声像/静音自动化 | bdf代码quality/preview双PASS，新磁盘Save/Open联合SDK云端完整PCM通过；通用图PDC/真实NOIRE完整流程未验收 |

普通构建不静默降级导出活动 VST 的错误声音；不支持的图和自动化明确拒绝。
实验整曲路径要求 `experimental-fresh-helper`。已推b73仍要求单个活动VST、无Insert；本轮串行FX范围及拒绝条件见上文，不属于b73或03副本。
原无FX及当前串行FX路径均支持Master音量/声像/静音自动化；FX参数、其它非Master自动化和Tempo变化仍拒绝，尾音须明确确认固定窗口。
未知或无限插件 tail 保留提示，不能把固定尾音窗口称为完整自然释放保证。

## 普通插件扫描的清理与失败恢复（本次提交）

Source75 普通 UI 的 `start_scan` 和同步 `scan_catalog_checked` 改走 factory 声明查询，不调用完整组件 LOAD，也不在失败后自动回退。与原路径共用遍历、Probe、一次请求、5 秒响应窗口、取消、管道及保留式清理，没有第二个 helper pool。
成功条目保留原 ID/path，新增独立 catalog；`verified=false`、无实际 default-class 元数据、音频/MIDI/GUI 能力未知。两个目标 picker 均允许用户尝试加载，Browser 打开显式 Channel/Mixer 目标选择，不因未知分类误投 Mixer；工程保存 Unknown role，仍由原 `class_uid=None` 路径实际加载。
目录进入索引仍要求 helper 自然成功退出、实际 reap、管道完成及清理无错。充分健康回收的单模块查询失败保留 Unknown/unverified 和持久 scan_error 后继续；取消、资源占用、清理未完成、强杀或异常退出则终止整次扫描，保留旧索引、rescan 标志及磁盘缓存。Pending 保留原 owner，再扫先推进同一清理，实际 Reaped 前不新建 helper。
缓存加法接受合法 catalog，保留旧 VST2 和实际默认类元数据；仅有旧 verified 标记的条目仍要求重扫。原 `scan_checked` / `scan` 继续作为显式 loaded-default 验证，保持 None class、48k/Q128 及原严格消费者合同，不作为目录扫描 fallback。实验 strict 渲染的类选择、状态、预算和块大小未放开。

目录/缓存、真实 mock-child 协议、目标选择、保存重开、默认 LOAD 不重试及严格准入拒绝回归已通过 Source75 云端完整 root 套件，并添加独立本机 selector `windows_kontakt_production_catalog_and_reap`。Source78 已实际完成普通 GUI 的 NI 目录扫描；该独立 selector 仍未执行。Source74 底层 factory 结果与这两项分别记录。

Source77 已接入新目录来源的实际运行身份桥接：真实默认 LOAD、完整 prepare 和首次 timing 成功后，取得实际 CID/version/分类及活跃 MIDI input，随原 Ready 绑定当前 generator 的 session/channel/endpoint/instance/slot/control。Unknown 只有实际 Instrument+MIDI 证明才进入整曲 Review；Save 返回及 Stop/Capture 前后复核，捕获 CID/version 漂移时不启动 clone。已分类 Instrument 保留原导出合同，不因新增可选身份附加门槛。身份查询失败不使原普通加载失败，Ready 携带明确不可资格原因。
证明只存当前运行会话，不修改 Project role、历史、dirty 或 catalog verified；Ready/Installed 两顺序和 restoration 已消费的唯一 Ready 均保留原 control/guard 绑定。现有 stopped capture、状态/参数/总线核对、strict clone、取消和实际 Join 均继续生效；不放宽片段转换/完整工程 QA。实际 all-feature 主测试程序编译通过，17 项 Windows 新增回归通过（App5、Whole10、restoration1、eligibility1）。另 3 项 backend 用例原受 Unix/shebang 夹具限制，初次 Windows 过滤器均为 0 tests；现已迁入 Windows/Unix 共享模块及实际 Python helper 夹具，不改生产启动规则。Windows 在 Shutdown 前持有真实 Python 进程句柄，要求 worker Join 后该进程实际 exit0，不能用 CMD 退出冒充。迁移后的 3 项用例现已在两个云端 Windows job 实际通过，preview 完整套件 2364 passed / 12 ignored；quality 的 retained-worker 断言失败及 preview 的 Clippy 错误见本页最新结果。证据：`source77-loaded-identity-targeted-01.log`、对应 terminal 及两份 `ci-da68aaf-windows-*-01.log`；修复候选和真实 NOIRE 闭环尚未验收。

Source57 实际通过 306 项针对性 Rust 测试：默认插件后端 187、扫描/App 5、默认 Probe 12、普通 transport 45、strict client 26、strict backend 31。默认/all-feature/no-feature 的 root `Clippy --all-targets -D warnings`、普通双 bin 构建、精确 `vst2,vst3` 配置检查及 root/vendor 格式检查通过。实际依赖清单验证 483 packages，root 仅 `vst2,vst3`，vendor 不启用实验 feature；这不是 release 预览包验收。
Source58 仅修正测试辅助函数返回值及 Unix 夹具的分平台错误文字，生产行为不变；当前默认 Clippy 和 12 项 Probe 回归再次通过，原清理断言保留。SDK 的 `-D unused_must_use`、最终 root/vendor/独立夹具格式检查通过；该 SDK 检查保留既有样式警告，不宣称整个 vendor 零警告。完整 vendor 补丁实际重放 73 个源文件逐字节一致；正确 discover 入口实跑 170 项打包测试全部通过。

首次门禁的 `result_large_err`/`collapsible_if`、测试 `err_expect`、离线 metadata 缺缓存、错误 unittest 导入入口和额外 rustc 参数多 target 拒绝均保留原始失败记录；已分别修正源码或调用方式。strict 取消/断管道夹具的 `OSError: [Errno 22] Invalid argument` 及 vendor 既有平台警告也保留，不当作 NI 实机结果。
Source57/58 验证期间没有启动 Kontakt、桌面或声卡；Source61 的真实普通扫描失败记录见下文。Linux/macOS 及播放时的扫描表现仍未测；Source52 原曲音频结果属于此前冻结源码。作品、状态、WAV、日志、构建及完整本机收据仍在 Git 外，初始三个受保护文件和 Saved13 原工程本轮未改。
证据：`target/backend-reliability-20261014/source57-scan-*`、`source58-*` 和两个 provenance 目录；只提交源码、必要回归及文档。

## 本轮已实现的功能

Source59 接线、Source60 验证：真实完整工程的两个 QA 入口使用 checked scan，先保存完整 Completed/Stopped 收据，再验证单模块 metadata；后续失败仍保留扫描 helper 的实际 reap 事实。已有 Windows QA 增加普通构建的单次 Kontakt 默认 class 扫描入口，明确只测空实例扫描/清理，不验证 NOIRE 音色或声音。原 source/renderer/stream 的证明保持独立。Source60 将新增测试模块限定到实际消费者所在的 Windows，避免 Linux 测试构建的未使用代码；生产 Windows/Linux 扫描不变。

Source60 实际通过默认及 all-feature 的 root `Clippy --all-targets -D warnings`、新增 6 项扫描证据回归、已有整曲/完整 source 的 34 项回归（3 项显式本机测试 ignored）、普通构建 native selector 编译/列举、root/独立 QA 格式及 diff 检查。依赖 SDK 的 4 项既有 Windows 未使用代码警告保留，不宣称整个 vendor 零警告。Source59 冻结记录未运行并保留；本次 8 项实际 terminal 收据在 `target/backend-reliability-20261014/source60-scan-*`。Source60 只编译/列举新入口，未启动 NI，不能以合成测试或 selector 列举替代实机。

Source61 在已推送 `1d374ce` 上实测普通 Kontakt 扫描：默认 helper 构建、默认测试 EXE 的真实 Cargo artifact、全部 284 输入及原作品/二进制 pins 通过；实际安装模块 FileVersionInfo 为 8.4.1。唯一测试 PID22476 实际 exit101，body 6.55 秒；helper PID25428 的 LoadPlugin 元数据已返回，但清理窗口内未自然退出，最终强制终止并实际回收，`kill_requested=true`、exit1、`reaped=true`，无 load/shutdown/kill/wait/pipe error 字段。生产正确返回 `UncleanRetirement`：`metadata returned but helper did not retire cleanly`，没有准入新索引；本次未操作 App 索引 UI，也未加载 NOIRE 状态、试听、编辑器、声卡、保存重开或导出。

Source61 外层的 `Root typed reap fields disagree with the actual bound receipt.` 是 PowerShell 单元素数组被解包成 11 字段 Hashtable 的校验误报；独立显式 object[] 核对三个布尔字段全部一致、校验错误数 0。实际扫描仍失败，不能将解析修正当作健康退出通过。原 JSON/log/terminal 字节全部保留，独立核对在 `source61-default-scan-independent-validation-01.json`；失败在 `windows-kontakt-default-scan-run-01/`。下一步保留原 5 秒加载及 250/2250ms 清理预算，观测 Shutdown/EOF、dispatcher、插件整体析构及 OLE 的实际边界；当前不能归因于 terminate 或 FreeLibrary。已有 SDK teardown 标记在 LoadInfo 回复后失去 active trace，需要补显式退出 trace 后再实测。

Source62 退出诊断初版复用原精确 `windows-load-response-timing-v1` opt-in，独立 reader/main trace、首个退出原因及每进程 128 条退出记录上限；未启用时不读 clock/TLS、不分配或写日志。记录命令消费、dispatcher 清理、整个插件 slot 清空、OLE 及 Windows run 返回，现有 6 个 SDK teardown 阶段归入退出 trace。SDK Drop/ExitDll body returned 不称为 FreeLibrary 完成；只有实际外层返回才记录相应 returned。6 项新增纯回归及原 39 项 helper 回归实际通过，default/all-feature 严格 Clippy、root/private fmt 和 diff 共 6 项门禁 exit0；未运行新诊断实机。无 vendor 或预算改动。

Source62 只读审查另外发现异常语义问题：拆出 dispatcher 字段使 Editor Rust unwind 时剩余局部变量改成 receiver→protocol，而原 aggregate dropglue 是 protocol→receiver，disabled 路径也受影响。该初版不能以门禁通过宣称顺序完全保留；Source63 已恢复完整 dispatcher 的聚合析构，仅括住原显式 editor.close，并补诊断开/关 × Editor 正常/unwind 四组合回归。复核确认原问题解决；修正版实际 46 项 helper 回归通过，default/all-feature 严格 Clippy、root/private fmt、diff 共 6 项门禁全 exit0，全部 284 pins 前后不变。原 Source62 冻结、45 项通过和 review 问题分别保留。Source63 门禁阶段未运行真实 Kontakt，不能据此宣布退出卡顿已修复；证据在 `source63-exit-*`，作品及初始三个受保护文件未改。

Source64 在提交 `9795f16` 上用同一入口实测退出诊断，默认 helper 和实际 Cargo 测试 EXE 构建通过，全部 284 输入前后不变。测试 PID18440 实际 exit101，外层 5.565 秒、校验错误数 0；helper PID25912 元数据返回后仍未自然退出，最终强制终止并实际回收（`kill_requested=true`、exit1、`reaped=true`）。实际错误仍为 `Plugin scan stopped: metadata returned but helper did not retire cleanly; previous index retained`，App 索引 UI 未测。

同一次日志确认 Shutdown 已消费、editor.close 和完整 dispatcher 析构已返回，组件 terminate 返回原始 SDK result0，插件 Drop 正文也已返回；随后 `windows_exit_dll edge=Before` 出现，但原清理期限内没有 Returned。整体 slot 清空、OLE 和 Windows run 也没有返回标记，范围缩小到 ExitDll 内部，不能称为 FreeLibrary 卡住或退出已修复。模块对象释放顺序复核未发现能解释本次失败的问题。复现仍使用精确 ignored selector、私有相邻 helper 及 `CITRUS_VST3_HELPER_LOAD_DIAGNOSTIC=windows-load-response-timing-v1`，加载和清理预算未改；证据在 `windows-kontakt-default-scan-run-02/` 和对应 `-stderr.log`、`-terminal.json`。未请求 NOIRE 状态、MIDI/DSP、编辑器、声卡、保存重开或导出。

Source65 仅给原退出日志补固定数字 PID 和 Windows 原生 TID；身份 API 在原 opt-in 与 128 条额度放行后才调用，未改 SDK、清理顺序、预算或权限。新增纯格式回归与原 46 项 helper 回归实际全部通过（47 passed），默认/all-feature root `Clippy --all-targets -D warnings`、root/private fmt、diff 共 6 项全 exit0，284 输入 pins 前后不变；依赖 SDK 原有 4 项未使用代码警告保留。最初本机门禁脚本误保留 Source63 manifest 路径，前置校验报 `Source65 manifest drift`，未启动 Cargo；新脚本 02 正确绑定后通过，原失败记录保留。证据在 `source65-exit-*-02`；新版身份日志尚未对真实 Kontakt 运行，等待链实验仅保留在 ignored target 中、未编译或实测。本次只提交该源码和两份文档，构建、脚本、收据、音频及作品留在 Git 外。

Source66 在已推送 `07e08de` 上实测身份日志：普通 helper/test artifact 构建通过，284 输入 pins 前后不变；唯一测试 PID15736 exit101，外层 5.499 秒、校验错误数 0。实际 helper PID8828、主退出日志 TID18888；组件 terminate result0 和 Drop 正文已返回，随后 ExitDll Before（563264800ns）仍无 Returned，生产再次强制终止并实际回收（exit1），普通扫描仍为 UncleanRetirement。没有 NOIRE 状态、MIDI/DSP、编辑器、声卡、保存重开或导出请求。

本机外部等待链实验的 SDK x64 结构实际编译检查通过；初版错误 PID/TID 的合成回调确实误标匹配，原失败保留。修正版03实际 Add-Type、托管 ABI、264 项检查/20 组用例通过，覆盖错误身份、错误/空回调、partial 结果及原进程/线程存活边界；严格原 emitter incarnation 仍未证明。最初检查脚本的数组拼接和反射 PSObject 错误已分别保留并在新脚本修正。这些纯检查不等于实际等待链成功。

实机采集被 `Direct child enumeration exceeded cap 128.` 拒绝，WCT session 未打开、Chains 为空；采集 owner 和 raw EOF 均完成后才退出同一 NoExit 宿主，fresh owned Cargo/helper/test 为 0。源码复核发现 do-while 中 continue 跳过 Process32NextW、重复同一候选直到上限；不能把 128 计数解释为 128 个不同子进程，也不能宣称已有死锁证据。实际候选的可执行路径没有记录，私有相邻 helper 的文件预检不替代实际 child image 绑定。新枚举修正及其回归仍在 target 准备、未编译/实测；原预算、权限和健康退出要求不改。证据在 `windows-kontakt-default-scan-run-03*`、`waitchain-observer-source-checks-01..05/`、`waitchain-sdk-layout-build-01/`；新采集仍需独立源码/二进制绑定和再次协调窗口。

Source67 在再次明确交接的独立窗口完成修正版04采集。相同 bounded 枚举核心的跳过/失败/重复/上限回归及原检查实际 279 项通过（20 组 callback/process 用例另含枚举检查），普通 helper/test artifact 构建通过，284 输入 pins 前后不变。唯一测试 PID25468 exit101，外层 5.547 秒、校验错误 0；helper PID15912 元数据返回、ExitDll Before（486396700ns）后未返回，仍强制终止并实际 reap exit1。采集成功不改变普通扫描失败结论。

本次原 helper 的实际 image path/creation 和同一 process/thread handle 存活核对通过；342 条进程记录包含 2 个 direct child，PID14120 因 path mismatch 排除，唯一匹配 PID15912。4889 条线程记录中匹配 19 个 helper 线程，各提交一次异步 WCT（false/997 为正常 Pending），19 个 callback status0、都只有一个 Thread 节点，无等待对象/拥有者。日志 TID12940 的节点为 Blocked，数字关联及观测区间身份成立；marker 不含线程 birth，严格原 emitter incarnation 仍未证明。IsCycle=false 不证明没有死锁，原始 WaitTime 数字不当作本次等待时长。实际 Close 返回、owner completed、raw EOF 后才退出同一 NoExit 宿主，fresh owned 进程为 0，再明确交回 CPU/native 窗口。证据在 `windows-kontakt-default-scan-run-04*`、`waitchain-observer-source-checks-07/` 和 `source67-default-*`。未改权限、预算或用户作品，也未请求 NOIRE 状态/声音或完整工程流程；下一步需调用栈证据定位 WCT 未给出的等待点。

Source68 已实际通过离线元数据 45 项纯回归、SDK x64 sizeof/offsetof/函数签名编译断言、C# Add-Type 与 25 项托管检查、C++ `/W4 /WX` 编译，以及普通 helper/test EXE 构建；全部 284 正式输入前后不变。最初采集尝试因仍打开的 VK3D PID8068 被原占用 guard 拒绝，未创建 private bins 或启动 NI；原拒绝及请求 inner exit2/外层实际 exit1 的独立校正保留。未弱化 guard、强关其他聊天的未保存界面或修改权限。

对方正常保存关闭并明确交接后，本轮唯一实际 NI 测试 PID26516 exit101（外层 7.693 秒、校验错误0）；helper PID21868、主日志 TID20900。组件 terminate result0、Drop 正文已返回，ExitDll Before（451214800ns）仍无 Returned；生产强制终止并实际 reap exit1，普通扫描仍为 UncleanRetirement。实际 WCT 20 个 callback 的 Close/owner/raw EOF 完成；这不证明健康退出。

外部单次 Normal|ThreadInfo 快照 API 实际返回 true，126110B、SHA `f31a3fa4f8b721af5a4059494cabbab89f6748fb0880116292d11a1b3795f483`；原写入 handle 与关闭后独立文件的 SHA/size 一致，fresh process/TID birth 绑定、实际 DLL 路径及所有读取 handle/DLL 释放完成后，才结束同一 NoExit 宿主（外层实际 exit101）。未延长原清理预算，诊断可能扰动时序。离线 reader01 实际 exit4：`duplicate_stream`；目录含四个 type0/size0/RVA0 的 SDK 空槽，已有 parser 将其重复误拒。原文件、失败和 45 项旧回归保留；reader02 仅兼容全零 UnusedStream，新的回归与实文件解析仍未测，C++ 实际读帧也未运行。

只读源码复核确认最终 Shutdown 先退出主消息循环并移除 TLS dispatcher，再清空 plugin slot；ExitDll 执行时 OLE 和 message-only command HWND 仍存活。普通 metadata 加载会探测并释放未挂载的 createView；宿主源码不证明 NI 自有 HWND/工作线程的等待关系。消息依赖仍是假设，不能以此盲改退出顺序或宣布原因已明。证据在 `source68-*`、`minidump-reader-pure-01/`、`minidump-sdk-layout-build-01/`、`minidump-capture-managed-checks-01/`、`minidump-frames-build-01/` 和 `windows-kontakt-default-scan-run-05*`，均在 Git 外；NOIRE 状态/声音和完整保存重开/导出仍未由本轮验证。

后续独立离线窗口：reader02 的 52 项回归全部通过、输入 SHA 无漂移；同一原 dump 实际重解析 exit2=`partial`，明确保留未知 streams、misc 扩展与 flags 的未解析边界。PID、线程 birth、进程 creation 秒级字段、SHA/size 及 AMD64 目标上下文均核对通过；四个全零 UnusedStream 不再误拒，其他重复/非法项的 guard 保留。既有 DbgEng 工具实际进程 PID24668 exit0（0.328s），20 个线程切换/读栈均 S_OK，无 cap、所有 pipe 完成、输入无漂移，NoExit 宿主实际 exit0 后 fresh owned0 并交回窗口。主 TID20900 得到 32 个引擎帧，经过 Kontakt、msvcp140、KERNELBASE、ntdll；返回的 symbol 文本只有 module 名，不能当作精确函数名或完整 unwind/等待原因证明。没有重编译、重采 NI、编辑器或声卡请求。证据为 `minidump-reader-pure-02/`、`source68-minidump-metadata-02.json` 和 `source68-offline-frames-01/`；原 reader01、拒绝结果、dump 和旧回归完整保留。

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

- 最新只读栈/PE 导出核对支持 Kontakt ExitDll 下方存在条件变量等待；信号来源与根因仍未证明。
  单次消息 pass 的新增 4 项纯回归已通过，Source69 实际运行 messages0 且退出仍失败；不把这个负结果扩大成所有消息/异步依赖均已排除。
- GitHub 已配置云端 Windows 质量检查和 preview 构建；当前聊天仍在本机，尚未建立云端开发环境。
  后台源码和无设备回归可移出桌面；Kontakt/NOIRE、用户作品与 iCON 监听留本机集中验收。

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
