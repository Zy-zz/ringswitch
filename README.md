# RingSwitch

一个 Rust 编写的 macOS 菜单栏工具，只保留两个功能：**切换具体窗口**、**按住鼠标侧键或自定义键，通过圆盘排列当前窗口**。

界面采用 AltTab 风格的窗口缩略图卡片和 Loop 风格的小型环形指示器。使用原生 [AppKit](https://developer.apple.com/documentation/appkit)、[Accessibility](https://developer.apple.com/documentation/applicationservices/axuielement) 和 [ScreenCaptureKit](https://developer.apple.com/documentation/screencapturekit)，没有 WebView、后台服务、网络请求或遥测。需要 **macOS 14 或更新版本**。Apple Silicon 本机实测；Intel 可从源码构建，尚未实测。

## 灵感来源

- [AltTab — lwouis/alt-tab-macos](https://github.com/lwouis/alt-tab-macos)：窗口级切换、应用图标与缩略图卡片、按住修饰键选择并松开激活。
- [Loop — MrKai77/Loop](https://github.com/MrKai77/Loop)：按住触发、移动选方向、松开排列窗口的小型环形交互，以及缩放目标预览。
- [REA — morluto/rea](https://github.com/morluto/rea)：从现有应用的可观察行为与实现线索理解功能，再实现所需功能的分析思路。本项目直接研究了公开源码和本机界面，未运行 REA 或反编译器。

本项目是独立 Rust 实现，没有复制上述项目的源码或图形资源。上述项目及其名称、许可属于各自作者；RingSwitch 与其没有隶属关系。

## 使用

- 默认 **Option + 反引号**：按住 Option，按反引号逐个选择窗口，松开 Option 激活。**Shift** 反向；方向键可选；**Esc** 取消。按物理按键识别。
- 默认 **鼠标后退侧键**：按住侧键，移动选择，松开执行。轻点不调整窗口；移回圆心最大化，或在圆心按住超过 0.5 秒后松开最大化；**Esc** 取消。
- 四个正方向对应四个半屏，四个斜方向对应四个角，中央对应最大化。缩放前显示目标区域，使用触发圆盘所在显示器的可用区域，避开菜单栏和 Dock。
- 切换器显示同一应用的多个具体窗口，分页展示并高亮当前选择。

## 自定义按键

点击菜单栏 **RS**：

| 菜单项 | 操作 |
| --- | --- |
| **窗口切换：… · 更改…** | 按下新组合，例如 Control + Tab 或 Option + Tab。至少含 Control、Option 或 Command；松开任一组合修饰键激活。Shift 留作反向切换。 |
| **圆盘键盘：… · 更改…** | 按下一个键或组合，例如 F6、Control + Space。也可单独按下后松开右 Control、右 Option 等修饰键。以后按住该键并移动鼠标，松开执行。 |
| **圆盘仅使用鼠标侧键** | 关闭圆盘键盘触发；这是默认设置。 |
| **侧键 4 / 5** | 常见后退、前进键；内部是 CoreGraphics 从零起的编号 3 / 4。 |
| **学习鼠标侧键…** | 按一次实际侧键，适配不同鼠标的按钮编号。 |
| **恢复默认按键** | 恢复 Option + 反引号、后退侧键、关闭圆盘键盘触发。 |

录制时按 Esc 取消。冲突或不支持的组合会显示原因，保留原设置。单独圆盘修饰键不能同时成为窗口切换组合的修饰键。录制需要先完成辅助功能和输入监控授权。

设置保存在 `~/Library/Application Support/RingSwitch/bindings`，原版 `mouse-button` 设置自动迁移。修改立即生效，重启保留。

## 构建与安装

安装 [Rust](https://rustup.rs/) 和 Xcode Command Line Tools 后：

```sh
git clone https://github.com/Zy-zz/ringswitch.git
cd ringswitch
cargo test --locked
./scripts/install.sh
```

安装到 `/Applications/RingSwitch.app` 并打开。更新前先从菜单栏退出 RingSwitch。仅构建可运行 `./scripts/build.sh`，产物为 `dist/RingSwitch.app`。

首次启动，在 **系统设置 → 隐私与安全性** 中允许：

1. **辅助功能**：读取、激活、恢复和调整窗口。
2. **输入监控**：监听快捷键和鼠标侧键。
3. **录屏与系统录音 → 屏幕录制**：可选，用于真实窗口缩略图；不采集音频。未授权仍显示图标与标题卡片，窗口切换和圆盘可用。

菜单里的权限项可打开设置页。`RS !` 表示监听尚未启动，`RS` 表示已启动；授予权限后自动重试。若系统要求重启应用，退出后重新打开。

本机产物使用 **ad-hoc 签名**，没有 Apple 开发者签名或公证；重新构建可能改变签名，macOS 可能要求在权限列表中重新允许该应用。

若权限列表显示已开启而菜单仍为 `RS !`，请在辅助功能和输入监控列表中移除旧的 RingSwitch 条目，再用 **添加** 重新选择 `/Applications/RingSwitch.app` 并允许，最后重启应用。只切换旧条目的开关可能仍保留失效的签名记录。

测试时退出 AltTab，或关闭其相同快捷键绑定；关闭 Loop 的重叠触发方式，避免同时响应。RingSwitch 不修改这些应用的配置。

## 验证

本机已通过：

- 12 项测试：布局、死区、跨屏坐标、循环切换、自定义快捷键、左右修饰键释放、录制、冲突校验和重叠窗口的预览匹配。
- 格式、严格 Clippy、release 构建和签名验证。
- 实际启动检查新版卡片网格和小型环形界面。
- 两个临时原生窗口：9 种布局经真实 Accessibility 设置并回读坐标和尺寸；分别激活并回读焦点；最小化窗口恢复。
- 两个临时窗口经 ScreenCaptureKit 捕获，确认有效 CGImage 缩略图返回。

CI 在 macOS 上运行格式、测试、Clippy 和应用构建。真实硬件侧键长按仍需鼠标试验；终端测试进程的授权不等于桌面应用已获授权。

```sh
/Applications/RingSwitch.app/Contents/MacOS/RingSwitch --doctor
open -n dist/RingSwitch.app --args --demo-switch
open -n dist/RingSwitch.app --args --demo-radial
```

原生检查只操作临时的 RingSwitch QA A/B 测试窗口：

```sh
open -n dist/RingSwitch.app --args --qa-windows
dist/RingSwitch.app/Contents/MacOS/RingSwitch --native-check <QA进程PID>
dist/RingSwitch.app/Contents/MacOS/RingSwitch --capture-check <QA进程PID>
```

检查后退出临时进程再打开正常应用。预览只存在内存中，不保存、不上传；每次切换仅捕获当前页一次，避免持续录制。

## 范围与限制

- 列出正常应用向 Accessibility 公开的窗口，包括最小化窗口，按最近激活顺序排列。未使用私有 Space API，不能保证列出所有桌面的全部窗口。
- macOS 真全屏窗口先退出全屏才能缩放；不可调整或有最小尺寸限制的应用会显示原因。
- 若鼠标驱动把侧键转换成键盘快捷键，系统收不到原始按钮事件；请恢复普通按钮，或把转换后的键录制为圆盘触发键。
- 无自动更新、主题管理、开机自启或其它窗口管理功能。

## 仓库写入权限

仓库公开，由 **Zy-zz** 维护，不向其他用户授予协作者写入权限。其他人可查看、下载、fork 或提交建议，但不能直接 push、合并或修改此原仓库。公开不等于开放写入权限，MIT 许可也不赋予原仓库的 GitHub 写入权限。

仓库自动化使用只读 contents 权限。源码采用 [MIT License](LICENSE)。
