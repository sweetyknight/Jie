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

`cargo test` 默认跳过标为 `ignored` 的交互、性能和在线测试。交互测试会盖住屏幕一会儿，按模块一起跑，一次一个（例如 `cargo test overlay:: -- --ignored --test-threads 1`），样张存在 `target/test-tmp/` 下。在线测试（`llm_samples` 等）会调用设置里的服务、按量计费，不要直接批量运行全部忽略项。

Windows 测试程序若仅在受限沙箱内报 `0xc0000142 / STATUS_DLL_INIT_FAILED`，先在普通终端（或经批准的沙箱外环境）重跑同一命令。沙箱外能正常运行时，不需要更换 DLL 或调整 CRT 链接配置；后续测试沿用该获准环境，不在同一受限沙箱内反复启动失败的程序。

- 吸附：`cargo test dump_screen -- --ignored` 存几张真实屏幕，`cargo test real_screens -- --ignored --nocapture` 打印每个点的层次并画出默认那层（`target/test-tmp/snap/`）。
- 录屏界面：`cargo test record_flow -- --ignored` 截下真实屏幕存成 `target/test-tmp/record/scene-*.png`，并检查 GIF 画面第一行不是边框颜色（编进去的那圈动图边框除外）。框满整块屏幕的两种入口要用发布版（调试版编一帧 4K 要好几秒）：`cargo test --release full_screen -- --ignored --nocapture --test-threads 1`，控制条压在任务栏上、截屏截得到，它底下的色块录到一半由绿变橙，GIF 首帧那块是绿的、末帧是橙的。
- 长截图：`cargo test longshot::tests` 覆盖快速往返、越过起点补接上方、失配后回滚恢复、固定栏和边缘空白，逐行比对完整内容。`cargo test longshot_flow -- --ignored --test-threads 1` 在屏幕上开一个能滚的测试窗口，从页面中间开始，下滚后一次回滚两屏多到起点上方，再回页首、滚到底，逐行比对整页，并检查原位置出了结果钉图；操作栏截图存在 `target/test-tmp/longshot/scene.png`。对真实浏览器（Edge 本地长网页，带固定顶栏）实测过一次：200 次 ↓ 每帧都对上，拼出的 1170 × 14316 长图没有重复和断层；要从项目目录外运行测试程序，否则低完整性的 Edge 起不来窗口。
- 输出失败：`cargo test failure_keeps_result -- --ignored --test-threads 1` 模拟写入失败和剪贴板被占用，确认失败后结果仍可重试；不改剪贴板内容。
- 录屏资源：`cargo test --release measure_ -- --ignored --nocapture --test-threads 1`（`measure_4_pins` 要在 `measure_2_scroll` 之后），结果另存在 `target/test-tmp/measure/*.txt`。

## 脚本

| 路径 | 用途 |
|---|---|
| `scripts/macos/package.sh` | 构建、生成图标、签名并打包 macOS 应用 |
| `scripts/macos/setup-signing.sh` | 首次准备或重新信任本机签名证书 |
| `scripts/macos/icon.swift` | 生成应用图标，由打包脚本调用 |
| `scripts/macos/test.sh` | 运行独立的 macOS 界面回归应用 |
| `scripts/windows/icon.ps1` | 生成 `assets\jie.ico`（改图标后手动跑，结果提交） |
| `scripts/windows/package.ps1` | 构建 Windows 便携包 |

构建产物统一写入 `target/` 和 `dist/`，不提交到仓库。

## 设计约定

**资源是第一约束**：Windows 空闲私有内存 ≤ 15 MB、macOS 内存占用 ≤ 20 MB（待实测确认）；空闲 CPU 为 0；高分屏截图唤起 ≤ 100 ms、拖框 ≥ 60 fps；exe / app ≤ 3 MB。截图或取字结束后释放帧缓存，每张钉图内存不超过图片数据的两倍。录屏不常驻，1280×720 区域录制中多占 ≤ 20 MB、录多久都不涨，动图钉图 ≤ 2 帧图片数据。新窗口（托盘面板、钉图悬停条）用时才建、关了就销毁。

**模块单向依赖**：`app` → `overlay` → `record` → `pin` → `anim`；`app`、`overlay` → `longshot` → `record`（边框和控制条的摆法）、`pin`；`overlay`、`pin` → `toolbar`；`overlay` → `snap`（纯计算）；`app`、`settings_ui` → `hotkeys`。平台层不依赖 `settings`，`pin/bar.rs` 不认识钉图（按钮回调由钉图建窗口时给）。设置文件不做旧版兼容。

**界面：新野兽派**（[设计稿](https://claude.ai/artifact/Q3iKzYeopupsZP8jBp6hT1)）。颜色、描边、实心阴影、圆角、尺寸都在 `src/ui.rs`，两端共用一份绘制代码，平台层不加新画法。界面里不出现蓝色，主题紫 `#9C44EC`，选区颜色可自定义（录屏边框、截图和动图边框、录制圆点也用它）。边框粗细只有一份设置（细、中、粗 = 1、2、3 DIP，默认 2，同选区边框），录制时区域外的框、长截图的框、截图和动图的框都用 `Settings::frame_px`。截图和动图的框是导出内容的一部分：动图的框录制时就编进 GIF（`anim::Writer::framed`），截图的框钉图上现画、复制保存时加上（`Image::framed`），取字和翻译用的仍是不带框的原图。栏上的按钮只放图标，鼠标移上去显示名字和快捷键；对话框里的按钮写字。平台差别只有：快捷键写法、系统字体、设置窗口标题栏和底部按钮顺序（Windows「确定 取消」，mac「取消 确定」）、托盘面板从任务栏往上弹还是从菜单栏往下弹。两处例外：设置窗口的边框和标题栏是系统的（Win11 用 DWM 刷成奶油色和墨色边框，Win10 保持白色）；钉图右键菜单在 mac 上是系统菜单。

**图标「取景框」**（[预览](https://claude.ai/artifact/PWwgR63LbH73esTYzEUD8t) 的 A）：紫底、白色取景框、右下角柠檬黄方块，墨色粗描边和错位实心阴影。三处画的是同一份 100 × 100 网格坐标，改造型要一起改：Windows 的 `scripts/windows/icon.ps1`（exe、托盘、窗口图标都取自 `assets\jie.ico`）；mac 的 `scripts/macos/icon.swift`（紫底铺满整个图标，因为 macOS 26 起不是满版圆角方块的图标会被套进灰底方块）；mac 菜单栏的单色模板图（`src/plat/mac/app.rs` 的 `tray_icon`）。24 像素及以下白框加粗，不然发灰。

**截图和录屏是同一个遮罩**，顶上「截图 | 录屏」模式条，`Tab` 切换（模式条挡着选区、读数或操作栏时挪到屏幕底下，上下都挡就不出）；两个快捷键只决定打开时的模式。有标注时不能切到录屏（标注录不进去）。

**长截图**（`longshot.rs`）沿用录屏的做法：关掉遮罩，选区外画边框，操作栏（取消、完成）按录屏控制条的规则放在区域外面，用平台层的区域截屏（不画鼠标）每秒 20 帧截活的屏幕。拼接是纯计算：每行算哈希（右边 24 DIP 不算，滚动条的滑块跟着动）；第一次真滚动时前后两帧顶上、底下相同的行当成不动的（固定顶栏、底栏，或者恰好是空白），中间那段每帧在已拼好的内容里从上一帧位置往两边找最近的对得上的位置，搜索覆盖整张长图及两端仍有足够重叠的位置，不限制在上一帧附近一屏；伸出去的行接上去（往上滚过开头也接）。对得上要求重叠够长、重叠里有足够多「有内容」的行（一大片纯色中间的行不算，否则空白处哪儿都对得上），对不上的行不超过 1/8（鼠标停的地方变色）。长图最多 30000 行、6000 万像素。期间临时注册不带修饰键的 `↑` `↓` `Esc` `Enter` 全局热键（编号 3–6，`plat::grab_keys`），`↑` `↓` 往区域中间的窗口发滚轮（`plat::scroll`：Windows 直接投递 `WM_MOUSEWHEEL`，不用前台、不挪鼠标；mac 合成滚轮事件，要辅助功能权限，没有就请求并提示一次）。完成后交给钉图的「结果」状态（同录屏结果条），太高先缩小到放得进工作区。

**录屏只出 GIF**，两端共用纯 Rust 的 `gif` crate：和上一帧比较只编码变化区域、不超过 256 色用精确调色板、超过用 NeuQuant；帧延迟按时间戳算，画面不变只加长上一帧。边录边写临时文件（Windows `%TEMP%\Jie\`，mac `NSTemporaryDirectory()/Jie/`），丢弃时删、启动时清空。复制到剪贴板的是文件（`CF_HDROP` / 文件 URL），因为没有通用的动图剪贴板格式。截屏：Windows 录制线程 `BitBlt`（不带 `CAPTUREBLT`，带了鼠标可能闪），鼠标自己画；mac 用 `SCStream`。

**录屏界面不用「不被截屏」属性**（`WDA_EXCLUDEFROMCAPTURE` 会让窗口从所有截屏里消失，远程桌面、会议共享都看不到）：边框画在区域外面；控制条依次试区域下、上、右、左，都放不下才放进区域里，这时 Windows 每帧用放大镜 API 排除控制条、截它底下那一小块实时画面贴回去。mac 用 ScreenCaptureKit 只对本程序这一路排除；录屏界面放在状态栏层（25），不被 Dock 挡住；开始时可截窗口列表里还没有边框和控制条就每 50 ms 再要一次，最多 0.5 秒。

**取字和翻译**都交给 OpenAI 兼容的模型服务（默认 DeepSeek），本机不识别。大图横着切成几条整宽的分开发（服务会把整张缩到约 1300×1300 像素）。翻译目标只提供中、英，原文语言也只粗判中、英，用来在原文已经是目标语言时换一个目标、在卡片上标出语言；别的语言照设置的目标语言译。

**暂不做**：MP4（GIF 滚动网页 30 秒约 23 MB、视频约 131 MB，要发长录屏时再做：Windows Media Foundation、mac AVAssetWriter，两端各约 M）、WebP/APNG、录制暂停、帧率和「录鼠标」设置、按 1 倍尺寸录、剪辑首尾、录制中标注、显示按键、声音、跟随窗口录制、吸附窗口里的控件。

## 实测记录

Windows 11 26200，3840×2160、150% 缩放，release 构建，15 fps，录屏幕中间 1280×720。CPU 是占一个核的比例。

| 画面 | 实际帧率 | 30 秒文件 | 每帧编码 | 录屏 CPU | 录制中多占内存 |
|---|---|---|---|---|---|
| 静止界面 | 15.0 | 0.59 MB | 1.0 ms | 4% | 11.8 MB |
| 滚动网页 | 15.0 | 22.9 MB | 18.5 ms | 32% | 15.5 MB |
| 播放视频 | 14.9 | 130.8 MB | 39.5 ms | 66% | 21.9 MB（录屏本身约 19 MB） |

5 个动图钉图：每个多占 4.9 MB（1.4 帧），一起播 CPU 33%。exe：2,004,992 字节（2026-11-09，加长截图后）。NeuQuant 在 4096 色渐变上平均误差 8.8、最大 30。

## 待验收

- 空闲内存、CPU：界面重设计后没复测。要用发布版、关掉平时开着的 Jie；项目目录里的 exe 以低完整性运行、加不上托盘，先拷到别处再跑。
- Windows：在全新 Windows 10 22H2 / Windows 11 上运行便携包，回归托盘、快捷键、单实例、开机自启和保存对话框。
- 托盘面板（两端）：右键托盘弹出、点外面和 `Esc` 关、方向键和 `Enter`；录制中显示「停止录屏」和时长；mac 上从菜单栏往下弹、能拿到键盘。各项没有单独的无障碍名字，读屏软件只读得出窗口名「Jie 菜单」。
- 吸附：浏览器、VS Code、资源管理器、设置、微信这类窗口里，默认高亮主要面板，滚轮能一层层换；高分屏和深色主题下也认得出。
- 长截图：在常用软件（Chrome、Edge、VS Code、资源管理器、微信、Word）里实测滚轮和 ↑↓；高分屏小数缩放下文字逐行是否还完全一样（不一样会频繁「没接上」）；Windows 关掉「悬停时滚动非活动窗口」时滚轮落在操作栏上的转发。
- macOS（只在 Windows 上交叉编译检查过，没有真机跑过）：长截图全流程，特别是不带修饰键的 Carbon 热键能不能注册上、合成滚轮事件按位置落到区域里的窗口（不在鼠标下面时）、辅助功能权限的请求和提示；屏幕录制授权、Retina 与不同缩放的多屏、全屏应用上的截图、菜单栏、中文输入法、更新签名包后的授权和钥匙串访问；新图标（`icon.swift` 没在 mac 上跑过）在程序坞、访达和菜单栏里的样子；截图、标注、钉图、取字和翻译完整流程；录屏全流程，特别是全屏录制时控制条压在 Dock 上看得到、点了马上开始时边框和控制条没录进 GIF，复制的 GIF 粘贴到常用软件。

## 已知限制

Windows HDR 截图可能发灰，不支持独占全屏游戏和 UAC 安全桌面；一次只能框选单个显示器。长截图只认竖直滚动；右边滚动条那一截不参与比对，长图里会留下滑块的碎片；内容里有超过一屏高的纯色空白时，滚过那段对不出挪了多少，空白会变短；滚动中鼠标下面变了色的行可能被接进长图。录屏只录屏幕上那块区域（窗口挪走或被挡住就录到别的），GIF 每帧最多 256 色，渐变和照片会有色带；没有声音。
