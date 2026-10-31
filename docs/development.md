# 开发说明

需要 Rust stable（支持 edition 2024）。在对应平台上构建，以下命令均从仓库根目录运行。

## macOS

要求 macOS 14+、Apple 芯片，以及 Xcode Command Line Tools（可用 `xcode-select --install` 安装）。

```sh
# 首次构建时准备本机签名证书
sh scripts/macos/setup-signing.sh
sh scripts/macos/package.sh
open dist/Jie.app
```

产物为 `dist/Jie.app` 和 `dist/Jie-<版本>-mac-arm64.zip`。日常使用可将应用放到「应用程序」文件夹。

默认使用本机的 `Jie Local Development` 证书，保持应用身份稳定，减少更新后重复授权。不需要 Apple 开发者证书或公证。仅需临时构建时可以跳过证书准备：

```sh
JIE_SIGN_IDENTITY=- sh scripts/macos/package.sh
```

临时签名不访问签名证书，但更新后可能需要重新授予屏幕录制或钥匙串访问权限。

## Windows

需要 Visual Studio Build Tools 的 C++ / MSVC 工具链和 Windows SDK。

```powershell
cargo build --release
powershell -ExecutionPolicy Bypass -File scripts\windows\package.ps1
```

可执行文件是 `target\release\jie.exe`；打包命令会重新构建，输出 `dist\Jie-<版本>-portable.zip`。

## 测试

```sh
cargo test
```

macOS 原生界面回归：

```sh
sh scripts/macos/test.sh
```

界面回归会短暂打开独立测试应用，检查设置、登录项、保存对话框、PNG/JPEG、钉图、剪贴板、快捷键和输入法。配置写入 `target/test-tmp/mac-smoke`，不读取个人 API Key，检查后恢复剪贴板和测试登录项；采用临时签名，不访问签名证书，也不需要屏幕录制权限。

`cargo test` 默认跳过标为 `ignored` 的交互、性能和在线测试。按需运行指定测试；在线测试会调用配置的服务，不要直接批量运行全部忽略项。

## 脚本

| 路径 | 用途 |
|---|---|
| `scripts/macos/package.sh` | 构建、生成图标、签名并打包 macOS 应用 |
| `scripts/macos/setup-signing.sh` | 首次准备或重新信任本机签名证书 |
| `scripts/macos/icon.swift` | 生成应用图标，由打包脚本调用 |
| `scripts/macos/test.sh` | 运行独立的 macOS 界面回归应用 |
| `scripts/windows/package.ps1` | 构建 Windows 便携包 |

构建产物统一写入 `target/` 和 `dist/`，不提交到仓库。

## 待验收

UI 和两端功能已实现，以下人工验收仍需保留：

- macOS：屏幕录制授权、Retina 与不同缩放的多屏、全屏应用上的截图、菜单栏、中文输入法，以及更新签名包后的授权和钥匙串访问。
- Windows：在全新 Windows 10 22H2 / Windows 11 上运行便携包，回归托盘、快捷键、单实例、开机自启和保存对话框。
- 两端：截图、标注、钉图、取字和翻译完整流程，粘贴到常用软件，以及资源占用复测。

资源目标：Windows 空闲私有内存 ≤ 15 MB、macOS 内存占用 ≤ 20 MB（待实测确认）；空闲 CPU 为 0；高分屏截图唤起 ≤ 100 ms、拖框 ≥ 60 fps；exe / app ≤ 3 MB。截图或取字结束后释放帧缓存，每张钉图内存不超过图片数据的两倍。

已知限制：Windows HDR 截图可能发灰，不支持独占全屏游戏和 UAC 安全桌面；一次只能框选单个显示器。
