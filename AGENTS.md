# AGENTS.md

## 项目

华为电脑管家安装包启动器（Rust workspace），让非华为电脑可以安装并正常使用 PC Manager：

- `packages/huawei-pc-manager-bootstrap`：i686 GUI 安装器，`include_bytes!` 内嵌补丁 DLL
- `packages/huawei-pc-manager-bootstrap-core`：i686 注入 DLL
- `packages/version`：x86_64 劫持 `version.dll`（转发全部导出 + 挂钩 `GetSystemFirmwareTable`）
- `packages/common`：SMBIOS 伪造、进程注入等公共逻辑

## 构建

**完整构建流程、Linux 交叉编译环境、踩坑记录与产物验证见 [Build.md](./Build.md)，改构建相关代码前必须先读。**

速记：

- 目标三元组：`version` → `x86_64-pc-windows-msvc`；core / exe → `i686-pc-windows-msvc`
- 编译顺序不能变：先编 `version` → 复制到 `target/release/version.dll` → 再编 core / exe
  （bootstrap 的 build.rs 只在 `target/release` 找 DLL，供 `include_bytes!` 内嵌）
- 产物在 `dist/`：`version.dll`、`huawei_pc_manager_bootstrap_core.dll`、`huawei-pc-manager-bootstrap.exe`

## 关键约束

- 跨平台编译使用 `nightly-2025-01-15`；更新的 nightly 会把 `std::intrinsics` 当作硬错误。
- `forward-dll` 的 build.rs 在编译期读取 `C:\Windows\system32\version.dll`；
  Linux 下需在 `packages/version/` 放同名桩 DLL，编译完删除，不要提交。
- Linux 交叉编译用 `cargo-xwin` + `clang-cl` / `lld-link`；`XWIN_ARCH` 决定缓存架构，
  x86 SDK 首次解包约 20 分钟，中途不要杀进程。
- 补丁 DLL 必须匹配目标程序架构：`*_x64.exe` 装出来的管家是 x64，配 x64 的 `version.dll`。
- 注入的 core DLL 必须在 `enable_hook` 开头调用 `common::common::pin_module`（`GetModuleHandleExW` + `PIN`）：
  `injectors` 的 `InjectionGuard` 在注入后立即 `FreeLibrary`，而 detour 代码就在该 DLL 里；
  不 pin 会在新版安装包首次调用 `GetSystemFirmwareTable` 时跳进已释放内存，崩溃 `0xc0000005`（上游 PR #54）。
- 不要提交安装包 exe、`dist/`、`target/`。

## 验证

- `file dist/*` 检查架构。
- `llvm-readobj --coff-exports dist/version.dll` 应有 17 条 `ForwardedTo`。
- 用 Python 校验 `dist/version.dll` 的字节确实内嵌在 `dist/huawei-pc-manager-bootstrap.exe` 中。
- 交叉编译产物无法在 Linux 上运行，最终功能仍需 Windows 实机安装验证（查看 hijacking 日志）。
