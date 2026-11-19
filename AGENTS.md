# AGENTS.md

## 项目

华为电脑管家安装包启动器（Rust workspace），让非华为电脑可以安装并正常使用 PC Manager：

- `packages/huawei-pc-manager-bootstrap`：i686 GUI 安装器，`include_bytes!` 内嵌补丁 DLL；
  `src/nfc.rs` 生成「碰一碰」NFC 标签载荷与二维码
- `packages/huawei-pc-manager-bootstrap-core`：i686 注入 DLL
- `packages/version`：x86_64 劫持 `version.dll`（转发全部导出 + 挂钩 `GetSystemFirmwareTable`）
- `packages/common`：SMBIOS 伪造、进程注入、`wlan_fix.rs`（WlanQueryInterface 修复）等公共逻辑

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

## Wi-Fi 协同补丁（wlan_fix）

- `common::enable_hook` 末尾调用 `wlan_fix::install_async()`；只在 `HiConnectivityService.exe`
  里安装 hook，避免把 `wlanapi.dll` 强加载进所有 PCManager 进程。
- hook 安装必须在独立线程里做：`version.dll` 的安装逻辑跑在 `DllMain`，直接 `LoadLibrary`
  会因 loader lock 死锁。
- `WlanQueryInterface` 的 GUID 参数用 `WlanEnumInterfaces` 运行时取得的第一块物理网卡 GUID
  全量替换（坏 GUID 是未初始化内存，不是虚拟网卡；`op=4` radio_state 是协同必需）。
- 细节、部署与验证步骤见 [docs/wlan-fix.md](./docs/wlan-fix.md)。

## 一碰传 NFC 标签

- 载荷格式固定为 `SN=<16>|MAC=<12>|MODELID=00000506`，格式规则与
  [Thirdparty-huawei-Share-OneHop](https://github.com/Masterchiefm/Thirdparty-huawei-Share-OneHop)
  / [NFCSHARE-Honor-Edition](https://github.com/difaandailisi/NFCSHARE-Honor-Edition) 一致。
- SN 必须使用补丁配置里的 SN（`Config::sn`），不能用真实 BIOS SN，否则电脑管家匹配不上。
- 蓝牙 MAC 用 `BluetoothGetRadioInfo` 读取（`Win32_Devices_Bluetooth` feature），
  结果按 `{:012X}` 输出，不要反转字节。
- 二维码用 iced 的 `qr_code` feature（底层 `qrcode` crate），只出图不写 NFC。
- `Cargo.lock` 固定了 `web-sys 0.3.72` + `wasm-bindgen-futures 0.4.45`：
  新版 web-sys 删了 `GpuImageCopyBuffer` feature，会让 wgpu 0.16 依赖解析失败（见 Build.md）。

## CI / Release

- `.github/workflows/rust.yml`：windows-latest，固定 `nightly-2025-01-15`，
  push main 滚动更新 `latest` release，push `v*` 发正式 release，PR / 手动只构建。
- CI 构建顺序与本地一致：version x64 → 复制到 `target/release` → core / exe i686；
  额外跑 i686 单测、校验 PE 架构和 `version.dll` 是否内嵌。

## 验证

- `file dist/*` 检查架构。
- `llvm-readobj --coff-exports dist/version.dll` 应有 17 条 `ForwardedTo`。
- 用 Python 校验 `dist/version.dll` 的字节确实内嵌在 `dist/huawei-pc-manager-bootstrap.exe` 中。
- 交叉编译产物无法在 Linux 上运行，最终功能仍需 Windows 实机安装验证（查看 hijacking 日志）。
