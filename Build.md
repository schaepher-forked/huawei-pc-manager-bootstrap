# 构建指南

本项目最终产物是三个 Windows PE 文件：

| 产物 | 架构 | 说明 |
| --- | --- | --- |
| `version.dll` | x86_64 | 劫持补丁：转发 `version.dll` 全部导出，并在 `DllMain` 中挂钩 `GetSystemFirmwareTable` |
| `huawei-pc-manager-bootstrap-core.dll` | i686 | 注入到安装包子进程的核心逻辑 |
| `huawei-pc-manager-bootstrap.exe` | i686 | GUI 安装器，`include_bytes!` 内嵌上面那份 `version.dll` |

## 一、Windows 原生构建（官方方式）

```cmd
.\build-release.bat
```

`build.sh` 是同一套流程的 sh 版本（仍面向 Windows 上的 MSVC 工具链）。

## 二、Linux 交叉编译（本仓库实际验证过的流程）

Linux 上没有 MSVC，使用 `cargo-xwin`（下载 MSVC CRT/SDK）+ `clang-cl` + `lld-link` 交叉编译。
以下环境准备只落在 `/tmp`、`~/.cache`、`~/.local/bin`，**不需要 root，不改仓库源码**。

### 1. Rust 工具链

```sh
# 不要用最新 nightly：std::intrinsics 已变成 deny(unstable_imports)，common 会编译失败
rustup toolchain install nightly-2025-01-15 --profile minimal
rustup target add --toolchain nightly-2025-01-15 \
  x86_64-pc-windows-msvc i686-pc-windows-msvc
```

注意：系统的 `/usr/bin/rustc`（发行版自建）看不到 rustup 的 target，**PATH 里必须让 `~/.cargo/bin` 优先**。

### 2. clang / lld（无 root 时解包 deb）

```sh
mkdir -p /tmp/llvm-debs && cd /tmp/llvm-debs
apt-get download clang-18 lld-18 llvm-18 libllvm18 libclang-cpp18 \
  libclang1-18 libclang-common-18-dev
for d in *.deb; do dpkg-deb -x "$d" /tmp/llvm; done

mkdir -p /tmp/llvm/bin
cd /tmp/llvm/bin
for t in clang lld-link llvm-lib llvm-rc llvm-ar; do
  ln -sf "/tmp/llvm/usr/bin/$t-18" "$t"
done
```

Ubuntu 的 clang 包里没有独立的 `clang-cl` 二进制，它由 clang 根据 argv[0] 切换驱动模式；
但 `cc-rs` 会给 `libudis86-sys` 传 `-includestring.h` 这种 GNU 风格参数，clang-cl 不认，
所以需要一个**普通文件**（不是符号链接）包装脚本：

```sh
cat > /tmp/llvm/bin/clang-cl <<'EOF'
#!/bin/bash
args=()
while [ $# -gt 0 ]; do
  case "$1" in
    -include|--include) [ -z "$2" ] && exit 1; args+=("/FI$2"); shift 2;;
    -include*) args+=("/FI${1#-include}"); shift;;
    *) args+=("$1"); shift;;
  esac
done
exec -a clang-cl /tmp/llvm/usr/bin/clang-18 "${args[@]}"
EOF
chmod +x /tmp/llvm/bin/clang-cl
```

> 坑：不要先 `ln -s <真实clang> clang-cl` 再 `cat >` 写脚本——`cat >` 会顺着符号链接
> 把真实 clang 二进制覆盖掉，之后每次编译都在 exec 自己，表现为**没有任何输出地永久卡住**。

### 3. cargo-xwin

```sh
curl -L -o /tmp/cxwin.tar.gz \
  https://github.com/rust-cross/cargo-xwin/releases/download/v0.23.1/cargo-xwin-v0.23.1.x86_64-unknown-linux-musl.tar.gz
tar xzf /tmp/cxwin.tar.gz -C /tmp
cp /tmp/cargo-xwin ~/.local/bin/
```

### 4. 准备编译期用的 version.dll 桩

`packages/version/build.rs` 会调用 `forward_dll("C:\\Windows\\system32\\version.dll")`，
在编译期读取该 DLL 的导出表。Linux 上这个路径不存在，需要先做一个**导出表一致**的桩 DLL，
再把它放到 `packages/version/` 下作为同名字面文件（Linux 允许文件名带反斜杠和冒号）：

桩 DLL 生成方式（x64，17 个导出，ordinal 取自仓库里的 `version.exports`）：

```sh
# stubs.c：17 个返回 0 的空函数，函数名与 version.exports 一致
clang-cl --target=x86_64-pc-windows-msvc -c stubs.c -o stubs.obj
lld-link /dll /noentry /nodefaultlib /def:stubs.def /out:version.dll stubs.obj /machine:x64
```

`stubs.def` 形如：

```
LIBRARY version
EXPORTS
  GetFileVersionInfoA @1
  GetFileVersionInfoByHandle @2
  ...
  VerQueryValueW @17
```

放置桩文件：

```sh
cd <仓库根目录>
cp /tmp/stub/version.dll 'packages/version/C:\Windows\system32\version.dll'
```

编译完成后删除桩文件，避免污染工作区。

### 5. 编译

```sh
export PATH=$HOME/.cargo/bin:$HOME/.local/bin:/tmp/llvm/bin:$PATH
export LD_LIBRARY_PATH=/tmp/llvm/usr/lib/x86_64-linux-gnu
export RUSTUP_TOOLCHAIN=nightly-2025-01-15

# 5.1 先编 x64 补丁 DLL
cargo xwin build --release -p version --target x86_64-pc-windows-msvc

# 5.2 bootstrap 的 build.rs 只在 target/release 下找 version.dll，需手动放一份
mkdir -p target/release
cp target/x86_64-pc-windows-msvc/release/version.dll target/release/

# 5.3 再编 i686 的 core + GUI
XWIN_ARCH=x86 cargo xwin build --release \
  -p huawei-pc-manager-bootstrap-core -p huawei-pc-manager-bootstrap \
  --target=i686-pc-windows-msvc

# 5.4 清理桩文件
rm 'packages/version/C:\Windows\system32\version.dll'
```

关于 `XWIN_ARCH`：

- `cargo-xwin` 的缓存按架构放（`~/.cache/cargo-xwin/xwin/{crt,sdk}/lib/{x86,x86_64}`）。
- 第一次编 x64 只会下载/解包 x86_64，编 i686 前必须让缓存里存在 x86 的 CRT/SDK。
  用 `XWIN_ARCH=x86 cargo xwin cache xwin`（或直接给构建命令加 `XWIN_ARCH=x86`）补齐。
- **首次解包 x86 SDK 很慢（实测约 20 分钟，8 线程持续 CPU）**，中途不要杀进程；
  中断后重跑会从头解包，缓存里出现半套 `sdk/lib/um/x86` 时不要手动拷贝拼装。

### 6. 组装 dist

```sh
mkdir -p dist
cp target/x86_64-pc-windows-msvc/release/version.dll dist/
cp target/i686-pc-windows-msvc/release/huawei_pc_manager_bootstrap_core.dll dist/
cp target/i686-pc-windows-msvc/release/huawei-pc-manager-bootstrap.exe dist/
```

`target/` 和 `dist/` 都在 `.gitignore` 中。

## 三、产物验证

```sh
file dist/*
# version.dll: PE32+ x86-64 / 另外两个: PE32 i386

LD_LIBRARY_PATH=/tmp/llvm/usr/lib/x86_64-linux-gnu \
  /tmp/llvm/usr/bin/llvm-readobj-18 --coff-exports dist/version.dll
# 应看到 17 条 ForwardedTo: C:\Windows\system32\version.<函数名>

# 确认 GUI 内嵌的正是这份补丁 DLL
python3 - <<'EOF'
dll = open('dist/version.dll','rb').read()
exe = open('dist/huawei-pc-manager-bootstrap.exe','rb').read()
print(dll in exe)
EOF
```

说明：导出序号会被 rustc 自动导出的 `DllMain` 占掉第 1 位，实际转发函数是 2–18。
官方发布产物同样转发到带路径的 `C:\Windows\system32\version.*`，属预期行为（按名字导入不受影响）。

## 四、常见运行期问题

### 安装器退出码 0xc0000005（STATUS_ACCESS_VIOLATION）

现象：注入成功后日志出现 `Free library successfully`，几秒后安装器以 `0xc0000005` 退出，且崩溃前没有任何 `GetSystemFirmwareTable` 回调日志。

根因：`injectors` 的 `InjectionGuard` 在 `enable_hook` 返回后会 `FreeLibrary` 注入的 core DLL，
而 `retour` 安装的 inline detour 代码就在这个 DLL 内。模块被卸载后，安装包首次调用
`GetSystemFirmwareTable` 会跳进已释放内存（新版 PC Manager 触发）。

修复：在 core 的 `enable_hook` 开头用 `GetModuleHandleExW` + `GET_MODULE_HANDLE_EX_FLAG_PIN`
把自身模块钉住（`common::common::pin_module`），日志中应出现 `Pinned module: 0x...`。
对应上游 issue #53 / PR #54。

## 五、已知限制

- 补丁 DLL 必须与目标程序架构一致：`PCManager_Setup_*_x64.exe` 装出来的是 x64 管家，
  因此配 `dist/version.dll`（x64）。
- 交叉编译产物未在真实 Windows 上做运行验证；功能验证仍需在 Windows 上安装一次，
  并查看 `ProjectDirs` 缓存目录（通常在 `%LOCALAPPDATA%\hamflx\huawei_pc_manager_bootstrap\cache`）
  下的 `hijacking-*.log`。
