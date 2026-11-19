# 永久修复补丁：给 bootstrap 的 `version.dll` 加 WlanQueryInterface hook

## 已验证的根因

`HiConnectivityService.exe` 调用 `WlanQueryInterface` 时传的是**未初始化的 GUID**
（下面是脱敏后的观测样本，后 8 字节已用 `xx` 代替）：

```
7dc3a671 f77f 0000 xxxxxxxxxxxxxxxx
7dc3a671 f77f 0000 xxxxxxxxxxxxxxxx
7dc3a671 f77f 0000 xxxxxxxxxxxxxxxx
```

前 8 字节固定、后 8 字节每次不同 —— 不是网卡 GUID，是堆上的垃圾数据。
WlanSvc 返回 `1168 (ERROR_NOT_FOUND)` → PCManager 兜底 `wifi switch state = 2` →
`OpenWifi::CheckOrOpenWifi` 判定「WiFi 未开」→ `DoLaunchProc = -27` → 协同超时。

**替换成物理 WLAN GUID 后 `rc=0`，协同当场成功。**

---

## 需要改 3 个文件

### 1. 新增 `packages/common/src/wlan_fix.rs`

源码已集成到 `packages/common/src/wlan_fix.rs`（本文档随仓库保存，便于后续排查）。

### 2. `packages/common/src/lib.rs`

```rust
pub mod common;
pub mod communication;
pub mod config;
pub mod wlan_fix;      // <-- 新增
```

### 3. `packages/common/src/common.rs`

在 `pub fn enable_hook(...)` 的 `Ok(())` 之前加一行：

```rust
    info!("wlan_fix: installing WlanQueryInterface hook");
    crate::wlan_fix::install_async();
}

Ok(())
```

`Cargo.toml` **不用改**：`retour`、`widestring`、`windows-sys`
（含 `Win32_System_LibraryLoader` / `Win32_Foundation`）都已在依赖里。

---

## 编译

```cmd
cd <仓库目录>
build-release.bat
```

产物在 `dist\` 和 `target\x86_64-pc-windows-msvc\release\`。
**PCManager 是 64 位，用 `x86_64` 那份 `version.dll`。**

---

## 部署

```powershell
# 管理员
Copy-Item '.\target\x86_64-pc-windows-msvc\release\version.dll' `
          'C:\Program Files\Huawei\PCManager\version.dll' -Force
Restart-Service HiConnectivityService -Force
```

> 也可以直接重跑 `huawei-pc-manager-bootstrap.exe` 重装，它会自己投放新的 `version.dll`。

---

## 验证

重启服务后，看服务侧的劫持日志：

```powershell
Get-ChildItem 'C:\Windows\System32\config\systemprofile\AppData\Local\hamflx\huawei_pc_manager_bootstrap\cache' `
  -Filter 'hijacking-HiConnectivityService*.log' |
  Sort-Object LastWriteTime -Descending | Select-Object -First 1 |
  ForEach-Object { Get-Content $_.FullName -Tail 20 }
```

应该出现：

```
wlan_fix: physical WLAN GUID = [xx, xx, xx, xx, ...]
wlan_fix: WlanQueryInterface hooked (attempt N)
```

然后点一次多屏协同，服务日志里应变成：

```
cur wifi switch state:1
```

并且 `HiViewSynergy.log` 里不再出现 `DoLaunchProc return code =-27`。

---

## 实现要点

- **只在 `HiConnectivityService.exe` 里安装**（`is_target_process()`），避免把 `wlanapi.dll`
  强加载进所有 PCManager 进程
- hook 安装放在**独立线程**里做，避免 `DllMain` 的 loader lock 死锁
- 物理 GUID **运行时**用 `WlanEnumInterfaces` 取，不写死（换网卡/换驱动仍然有效）
- 对**所有 opcode** 都替换 GUID（Frida 验证时就是这么做的，`op=4` 是协同必需的那个）

---

## 回滚

```powershell
# 用 bootstrap 重新投放原始 version.dll，或从备份恢复
Copy-Item '<备份>\version.dll' 'C:\Program Files\Huawei\PCManager\version.dll' -Force
Restart-Service HiConnectivityService -Force
```

---

## 附：不需要重新编译的过渡方案

如果暂时不想动 Rust 工程，可以用计划任务在开机时以 SYSTEM 身份跑 Frida 脚本
（Frida 脚本，未随仓库分发），缺点是需要常驻、服务重启后要重新附加。
建议还是走 DLL 方案。
