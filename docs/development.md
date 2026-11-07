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
- 录屏（两端）：录屏快捷键、录制中再按快捷键停止；边框和控制条不出现在 GIF 里（区域贴屏幕边、全屏时也是），通过远程桌面（UU 远程、ToDesk 等）看也看得到；静止界面、滚动网页、播放视频三种画面各录 30 秒，记下实际帧率、CPU、录制中内存和文件大小；复制的 GIF 粘贴到微信、QQ、飞书、浏览器（GitHub Issue）、资源管理器/访达。macOS 的录屏截屏（`SCStream`）只在 Windows 上做过交叉编译检查，没有在真机上跑过；真机上要特别看：全屏录制时控制条压在 Dock 上看得到（状态栏层），点了马上开始时边框和控制条没录进 GIF。
- 吸附：浏览器、VS Code、资源管理器、设置、微信这类窗口里，默认高亮主要面板（不是按钮、列表项这种小块），滚轮能一层层换到窗口和更小的面板、卡片、输入框、表格；高分屏和深色主题下也认得出。调算法时用 `cargo test dump_screen -- --ignored` 存几张真实屏幕，`cargo test real_screens -- --ignored --nocapture` 打印每个点的层次并画出默认那层（`target/test-tmp/snap/`）。
- 录屏界面：倒计时、边框、控制条都不出现在 GIF 里；`cargo test record_flow -- --ignored` 截下真实屏幕存成 `target/test-tmp/record/scene-*.png`（它们没设不被截屏，截到的就是远程看到的样子），并检查 GIF 第一行不是边框颜色。框满整块屏幕的两种入口用发布版分别跑 `cargo test --release full_screen_flow -- --ignored --nocapture` 和 `cargo test --release full_screen_from_overlay -- --ignored --nocapture`（调试版编一帧 4K 要好几秒，2 秒只录得到一帧）：控制条压在任务栏上、截屏截得到；控制条底下垫的色块录到一半由绿变橙，GIF 第一帧那块是绿的、最后一帧是橙的（从遮罩来时比的是录完后的真实屏幕）。两个别放在同一个进程里连着跑：播放 4K 动图钉图很慢，第二个会卡到系统报「无响应」。
- 录屏资源复测（PLAN.md §10）：`cargo test --release measure_ -- --ignored --nocapture --test-threads 1`，静止、滚动、视频三种画面各录 30 秒 1280×720，再同时开 5 个动图钉图；结果打印出来，也存在 `target/test-tmp/measure/*.txt`。

资源目标：Windows 空闲私有内存 ≤ 15 MB、macOS 内存占用 ≤ 20 MB（待实测确认）；空闲 CPU 为 0；高分屏截图唤起 ≤ 100 ms、拖框 ≥ 60 fps；exe / app ≤ 3 MB。截图或取字结束后释放帧缓存，每张钉图内存不超过图片数据的两倍。

已知限制：Windows HDR 截图可能发灰，不支持独占全屏游戏和 UAC 安全桌面；一次只能框选单个显示器。录屏只录屏幕上那块区域（窗口挪走或被挡住就录到别的），GIF 每帧最多 256 色，渐变和照片会有色带；没有声音。
