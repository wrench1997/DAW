# Windows x64 构建与发布

当前源码候选：Citrus Studio 0.5.0-alpha.1；下文固定发布流程/示例继承自 0.4.0
目标系统：Windows 10/11 x64  
目标三元组：x86_64-pc-windows-gnullvm

本流程锁定 Cargo.lock、Rust 工具链、LLVM-MinGW 版本、feature 集与发布文件白名单。它使发布输入和步骤可追溯；除非已在独立干净环境中比较结果，否则不要宣称不同机器上的输出逐字节完全相同。最终发布身份以签名后的 SHA-256 为准。

## 当前验证状态（2026-10-09）

本页记录固定 gnullvm 发布流程，不能当作当前 prerelease 的发布验收。最新终态 `b92c394` 的 [MSVC preview](https://github.com/wrench1997/DAW/actions/runs/37950671153) 完整通过：231 Python cases（227 passed、四项 Unix-only skip）、1165 app +15 helper +5 editor protocol +2 transport protocol Rust tests、optimized static-CRT、MIT/vendor provenance、PE/ZIP/hash 与解压 helper smoke；上传关闭、artifacts 为0。[Quality](https://github.com/wrench1997/DAW/actions/runs/37950671033) 源码与独立 native state/interaction/lifecycle 通过，paint 前后失败、repaint comparison 跳过，整个 quality 仍失败。

上一轮 native-edit 合并源码 `87ceb06` 已通过Linux1170 app +22 helper +5+2 protocol、1166 core、215 Python、304 available vendor cases（一个明确fixture排除）、26 doctests、fmt/两套严格root Clippy/build/helper smoke和两种MSVC source checks。新helper default2048四项/fresh-state回归已通过，对应e487a50的Windows结果见上，不能沿用更旧helper的优化测量。它只更换原生编辑交付/捕获的内部通道；App/timing/wire 未变，helper 仍单线程。旧4fdf/244优化结果为 quiet12/16、load12/16，512/2048全部delivery组合通过，但loaded2048仍有raw-interval overrun，不能称实时/设备认证。默认2048、128/256/512 Experimental保持不变。0.5.0-alpha.1加载v10/v11、保存v12；旧项目MIDI ports默认Off，先保留备份。

下文 `$Version` 和目录名的 0.4.0 是历史示例；构建当前候选时必须使用 `Cargo.toml` 的实际版本 `0.5.0-alpha.1`，不得把新 v12 构建标成旧 0.4.0。Rust/LLVM-MinGW 固定工具链及三文件发布契约不变。

## 开发 CI（独立分支验证）

`.github/workflows/ci.yml` 增加 Windows MSVC stable 开发门禁：fmt、锁定依赖的 all-feature/all-target tests、Clippy `-D warnings`、all-bin build、独立的 helper harness 回归 / 实际 helper 协议 smoke 和 no-default-features check。独立分支 `ci/windows-reliability-20261009` 的 push 触发验证；含 helper 自动 smoke 的运行已在 Rust/Cargo 1.99.0、Python 3.12.10 上全部通过。后续源码候选分别在相同门禁验证，确切提交/结果见 GitHub Actions 和 WORK_LOG。开发 smoke 不替代下述历史固定 gnullvm 发布流程、Release helper smoke、干净 Windows 实机验收与打包校验。

## 发布契约

一个可运行的 Windows 发布包必须在同一 ZIP 根目录中携带以下三个二进制文件：

1. citrus-studio.exe
2. vst3-host-helper.exe
3. libunwind.dll

vst3-host-helper.exe 不是可选示例程序。主程序启用 VST3 process isolation 后会显式从自身目录加载它；缺失时 VST3 加载会返回明确错误。两个 EXE 都导入 libunwind.dll，因此 DLL 也必须同目录分发。

ZIP 还必须携带仓库根目录的 LICENSE、THIRD_PARTY_NOTICES.md、README.md、保持原相对路径的 docs/FL_STUDIO_PARITY.md 与 docs/BUILD_AND_RELEASE.md，以及发布时生成的 SHA256SUMS.txt。保留 docs 子目录可保证 README 中的仓库内链接在解压后继续有效。

## 历史 Windows 构建基线（本次待复验）

| 组件 | 固定版本 |
| --- | --- |
| Rust | rustc 1.97.1，commit 8bab26f4f68e0e26f0bb7960be334d5b520ea452c |
| Cargo | 1.97.1，commit c980f4866141969fab6254a680546a277789d6f0 |
| Rust target | x86_64-pc-windows-gnullvm |
| LLVM-MinGW | 20260616 UCRT x86_64 |
| Clang/LLD | 22.1.8 |
| Cargo features | all-features，即 vst2 与 vst3 |
| 项目依赖 | 仓库 Cargo.lock，所有命令使用 --locked |

LLVM-MinGW 20260616 的官方发布页是 [llvm-mingw 20260616 with LLVM 22.1.8](https://github.com/mstorsjo/llvm-mingw/releases/tag/20260616)。

## 1. 准备工具链

在 PowerShell 中安装精确 Rust 工具链，而不是随时间移动的 stable 别名：

    rustup toolchain install 1.97.1-x86_64-pc-windows-gnullvm --profile minimal --component rustfmt --component clippy

下载并解压 llvm-mingw 20260616 的 ucrt-x86_64 包。以下路径仅为示例，按实际安装位置修改：

    $ReleaseToolchain = "1.97.1-x86_64-pc-windows-gnullvm"
    $LlvmMingwRoot = "C:\toolchains\llvm-mingw-20260616-ucrt-x86_64"
    $env:Path = "$LlvmMingwRoot\bin;$env:Path"

确认工具链和目标完全匹配：

    rustc "+$ReleaseToolchain" --version --verbose
    cargo "+$ReleaseToolchain" --version --verbose
    clang --version
    ld.lld --version

预期 rustc host 为 x86_64-pc-windows-gnullvm，Clang 和 LLD 为 22.1.8。不要混用 MSVC、windows-gnu 或其他 LLVM-MinGW 版本生成同一个发布候选。

## 2. 固定输入

从干净的发布提交或只读源归档构建。进入仓库根目录后设置：

    $RepoRoot = (Resolve-Path ".").Path
    $ReleaseOut = Join-Path $RepoRoot "target\release"
    $env:CARGO_INCREMENTAL = "0"

如果发布系统记录了源提交时间，可在每次构建中使用同一 SOURCE_DATE_EPOCH：

    $env:SOURCE_DATE_EPOCH = "<recorded-commit-unix-time>"

记录 Cargo.lock、工具链和源提交的哈希。--locked 会在 manifest 与 lock 不一致时立即失败，不允许在发布任务中临时执行 cargo update。

## 3. 质量门禁与 Release 构建

按顺序执行，任何一步失败都停止发布：

    cargo "+$ReleaseToolchain" fmt --all -- --check
    cargo "+$ReleaseToolchain" test --locked --all-features
    cargo "+$ReleaseToolchain" clippy --locked --all-features --all-targets -- -D warnings
    cargo "+$ReleaseToolchain" build --locked --release --all-features --bins

all-features Release 会同时构建主程序和受 vst3 feature 控制的 helper bin target。Cargo.toml 的 default-run 仍是 citrus-studio，因此新增 helper 不会使普通 cargo run 产生目标二义。

Cargo 不保证把 gnullvm 运行时 DLL 复制到输出目录。必须显式复制与本次链接器相同 LLVM-MinGW 包中的 libunwind.dll：

    Copy-Item -LiteralPath (Join-Path $LlvmMingwRoot "bin\libunwind.dll") -Destination (Join-Path $ReleaseOut "libunwind.dll") -Force

只允许使用这一 LLVM-MinGW 20260616 副本；不要混入 Rust sysroot 或其他 LLVM 版本中同名但不同内容的 DLL。

检查三个必要文件：

    $RequiredBinaries = @(
        (Join-Path $ReleaseOut "citrus-studio.exe"),
        (Join-Path $ReleaseOut "vst3-host-helper.exe"),
        (Join-Path $ReleaseOut "libunwind.dll")
    )
    foreach ($RequiredBinary in $RequiredBinaries) {
        if (-not (Test-Path -LiteralPath $RequiredBinary -PathType Leaf)) {
            throw "Missing release binary: $RequiredBinary"
        }
    }

## 4. PE 架构和依赖检查

用当前 LLVM-MinGW 的工具检查两个 EXE：

    llvm-readobj --file-headers (Join-Path $ReleaseOut "citrus-studio.exe")
    llvm-readobj --file-headers (Join-Path $ReleaseOut "vst3-host-helper.exe")
    llvm-objdump -p (Join-Path $ReleaseOut "citrus-studio.exe")
    llvm-objdump -p (Join-Path $ReleaseOut "vst3-host-helper.exe")

两个 EXE 都必须报告：

- Format: COFF-x86-64
- Arch: x86_64
- Machine: IMAGE_FILE_MACHINE_AMD64
- Subsystem: IMAGE_SUBSYSTEM_WINDOWS_GUI
- 导入表包含 libunwind.dll

在没有 Rust、Cargo、LLVM-MinGW 和 Visual Studio 的干净 Windows 10/11 x64 虚拟机上再启动一次发布目录中的 citrus-studio.exe。这一步用于发现开发机 PATH 隐式提供的 DLL；不能只在构建机上验收。

## 5. VST3 helper 协议 smoke test

复用的 `scripts/smoke_vst3_helper.py` 不加载任何第三方插件，也不启动 DAW 主程序。它只需要 Python 3.8+ 标准库：Python 是开发 / CI 验收工具，**不是应用运行或发布包依赖**，不加入第 7 节的发布白名单。Windows hosted runner [官方软件清单](https://github.com/actions/runner-images/blob/main/images/windows/Windows2025-Readme.md) 包含 Python；CI 记录实际解释器版本并运行 harness 自测。

在仓库根目录运行（先完成上面的 Release 构建及运行时 DLL 复制）：

    python -B -m unittest discover -s scripts -p "test_smoke_vst3_helper.py" -v
    python -B scripts/smoke_vst3_helper.py (Join-Path $ReleaseOut "vst3-host-helper.exe") --timeout 5

开发 CI 对 `target/debug/vst3-host-helper.exe` 执行相同脚本。第 4 节 PE 检查仍负责确认 Release helper 的 Windows GUI subsystem；脚本在 Windows 使用 CREATE_NO_WINDOW，并以 helper 所在目录为工作目录。

协议顺序与断言：

1. 空行后发送 `"GetAllParameters"`，必须收到 JSON `{"Error":{"message":"No plugin loaded"}}`，不能出现额外 stdout。
2. 发送无效 JSON，必须收到 `Invalid command:` 错误；再次发送 `"GetAllParameters"`，验证解析错误不会破坏后续响应。
3. 发送 `"Shutdown"`，**保持 stdin 打开**，确认进程因命令而非 EOF 正常退出，退出码为 0；Shutdown 不应额外回复。

启动、三次回复及正常退出共享五秒总超时。stdout / stderr 并行读取，保留的诊断和协议队列有上限；失败时终止 helper，并用独立的有界等待回收子进程及管道。CI 步骤另有 1 分钟外层超时。脚本自测使用临时 Python 子进程覆盖正常协议、stderr 大量输出、无响应 / 退出超时、无效回复、早退、非零退出、协议恢复失败和清理，不加载插件。

这证明被测二进制的 0.9.0 wire protocol 可启动、通信、恢复和关闭，不证明插件加载、DSP、设备或 GUI 可用；MSVC debug smoke 也不证明 gnullvm Release 的 DLL 和打包正确。

如组织拥有可用于测试的合法 VST3 插件，再执行一次主程序端加载、音频处理、保存状态和卸载测试，但不要把测试插件复制进 Citrus Studio 发布包。

## 6. 签名顺序

商业发布若使用 Authenticode，应先签名本项目构建的 citrus-studio.exe 和 vst3-host-helper.exe，再执行最终 smoke test、SHA-256 和 ZIP。libunwind.dll 是第三方原始二进制，默认保持与 LLVM-MinGW 20260616 副本逐字节一致；除非组织的签名与开源合规策略明确批准，不要用本项目证书重签它。任何签名、时间戳或二进制后处理都会改变哈希；哈希生成后不得再修改包内文件。

## 7. 按白名单暂存

禁止直接复制整个 target/release。只暂存明确允许的文件：

    $Version = "0.4.0"
    $DistBase = Join-Path $RepoRoot "dist"
    $PackageName = "Citrus-Studio-$Version-Windows-x64"
    $StageRoot = Join-Path $DistBase $PackageName
    $ZipPath = Join-Path $DistBase "$PackageName.zip"
    if (Test-Path -LiteralPath $StageRoot) {
        throw "Staging directory already exists: $StageRoot"
    }
    if (Test-Path -LiteralPath $ZipPath) {
        throw "Release ZIP already exists: $ZipPath"
    }
    New-Item -ItemType Directory -Path $StageRoot | Out-Null
    $StageDocs = Join-Path $StageRoot "docs"
    New-Item -ItemType Directory -Path $StageDocs | Out-Null
    Copy-Item -LiteralPath (Join-Path $ReleaseOut "citrus-studio.exe") -Destination $StageRoot
    Copy-Item -LiteralPath (Join-Path $ReleaseOut "vst3-host-helper.exe") -Destination $StageRoot
    Copy-Item -LiteralPath (Join-Path $ReleaseOut "libunwind.dll") -Destination $StageRoot
    Copy-Item -LiteralPath (Join-Path $RepoRoot "LICENSE") -Destination $StageRoot
    Copy-Item -LiteralPath (Join-Path $RepoRoot "THIRD_PARTY_NOTICES.md") -Destination $StageRoot
    Copy-Item -LiteralPath (Join-Path $RepoRoot "README.md") -Destination $StageRoot
    Copy-Item -LiteralPath (Join-Path $RepoRoot "docs\FL_STUDIO_PARITY.md") -Destination $StageDocs
    Copy-Item -LiteralPath (Join-Path $RepoRoot "docs\BUILD_AND_RELEASE.md") -Destination $StageDocs

此白名单天然排除 PDB、incremental 文件、测试程序、源代码、用户数据和未知 DLL。

## 8. SHA-256

签名和最终 smoke test 完成后，对包内固定文件生成 SHA256SUMS.txt：

    $HashNames = @(
        "citrus-studio.exe",
        "vst3-host-helper.exe",
        "libunwind.dll",
        "LICENSE",
        "THIRD_PARTY_NOTICES.md",
        "README.md",
        "docs/FL_STUDIO_PARITY.md",
        "docs/BUILD_AND_RELEASE.md"
    )
    $HashLines = foreach ($HashName in $HashNames) {
        $Hash = (Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $StageRoot $HashName)).Hash.ToLowerInvariant()
        "$Hash  $HashName"
    }
    $Utf8NoBom = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllLines((Join-Path $StageRoot "SHA256SUMS.txt"), $HashLines, $Utf8NoBom)

不在仓库文档中硬编码候选二进制的大小或哈希。源码、工具链、链接环境、签名或时间戳都可能改变结果；每个发布候选必须在签名完成后重新生成 SHA256SUMS.txt，并把最终 ZIP 的 SHA-256 记录到发布系统。

## 9. 生成和检查 ZIP

使用目录本身作为 Compress-Archive 输入，以保留单一顶层目录：

    Compress-Archive -LiteralPath $StageRoot -DestinationPath $ZipPath -CompressionLevel Optimal
    Get-FileHash -Algorithm SHA256 -LiteralPath $ZipPath

ZIP 根目录必须严格为：

    Citrus-Studio-0.4.0-Windows-x64/
        citrus-studio.exe
        vst3-host-helper.exe
        libunwind.dll
        LICENSE
        THIRD_PARTY_NOTICES.md
        README.md
        SHA256SUMS.txt
        docs/
            FL_STUDIO_PARITY.md
            BUILD_AND_RELEASE.md

检查归档内容：

    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $Archive = [System.IO.Compression.ZipFile]::OpenRead($ZipPath)
    try {
        $Archive.Entries | ForEach-Object FullName
    } finally {
        $Archive.Dispose()
    }

归档中不得出现第二个顶层目录、绝对路径或隐藏的开发文件。

## 10. 绝不能打包的用户数据

Citrus Studio 在 Windows 上使用 directories crate 的本地数据目录。当前路径位于：

    %LOCALAPPDATA%\Citrus\Citrus Studio\data

其中至少包括：

- Autosave.citrus
- plugins.json
- Recorded 目录中的 24-bit WAV 录音

这些文件包含用户作品、录音内容和本机插件扫描结果，绝不能进入发布暂存目录、ZIP、安装器、符号包、崩溃附件或公开 CI artifact。也不要打包用户另存的 .citrus/.wav 文件、VST2/VST3 插件目录、插件 state 或预设。

发布脚本应始终使用第 7 节的文件白名单；不要从用户 profile、LOCALAPPDATA、APPDATA、临时目录或整个 target 目录递归收集文件。

## 最终发布检查表

- fmt、当前完整测试集、Clippy -D warnings、all-features --bins Release 全部通过，并记录实际执行数、结果及对应提交；不能用历史固定测试数代替门禁。
- 两个 EXE 均为 AMD64 Windows GUI subsystem。
- helper 协议 smoke 通过且没有残留进程。
- libunwind.dll 与本次 LLVM-MinGW 20260616 副本哈希一致。
- 两个自建 EXE 已完成组织要求的签名；三个二进制均已完成恶意软件扫描，第三方 libunwind.dll 的来源哈希已核对。
- 在无开发工具链的干净 Windows 10/11 x64 环境完成启动测试。
- ZIP 只有一个顶层目录，内容与白名单完全一致。
- SHA256SUMS.txt 与最终签名后的包内文件一致，并另行发布 ZIP 的 SHA-256。
- LICENSE、THIRD_PARTY_NOTICES.md、README.md 与保持相对路径的 docs 文档已随包。
- 未包含任何用户缓存、录音、项目或第三方插件。


## Bounded parameter-storage follow-on

Reviewedbf573a4 integrates atfb7b91a; helper/vendor source is exact and App/timing/wire
is unchanged. Input8192/output4096 parameter policies, checked admission and sticky
output-fault capture/Process fences are source-scoped. The new copied helper must
matcha29e4942b5fe027e9891a28e5d7f7611d2c5b2f12ef5f283599d15c4278aee64.
Fresh combined gates pass1170 app +22 helper +5+2 protocol,1166 core,339 available
vendor cases/one explicit fixture exclusion,26 doctests,231 Python, fmt/two strict
root Clippy modes/build/helper smoke and both MSVC source checks. New-helper bounded
default2048 four-case/fresh-state acceptance passes, both exits0, with13/14 changing
debug core overruns retained. Its exactb92c394 Windows source/preview passed, with
the known native paint failure retained;
prior source and optimized measurements retain their actual helper identities.
No fixed gnullvm Release, installer or physical-device qualification follows.


Parameter comparative evidence is carried as a separate immutable source-only
appendix, with [concise pinned records](../qa/parameter_storage_cost/README.md) in
the preview source-document whitelist. The full appendix is not a preview executable
payload. Source ZIP verification separately counts tracked Git files, original-main
patch and appendices; every original negative/source/profile identity is retained.


## Checked-event admission follow-on

Reviewed7d8a416 integrates at7d7294b. Only bounded event rejection and note bookkeeping
change; App/reset/timing/wire stay atb92c394. Exact owner/helper/native checks pass
with helper59b6bcbdb7a90b08fe5c8ebb2da2ba3ffe368c1089d70a6c7d4e7ab28b90d086.
Fresh combined1170app+22helper+5+2protocol,1166core,362availablevendor/one explicit
fixture exclusion,26doctests,239Python,fmt/two strict rootClippy modes/build/helper
smoke andbothMSVCsource checks pass. New exact Windows CI remains pending. Old parameter costs and
optimized244f622 timing remain historically attributed; event admission carries no
new payload-allocation, threading, realtime or native-resize qualification.
