# LanBeam 宣传视频

用 [Remotion](https://www.remotion.dev) 做的 LanBeam v0.2.0 宣传片：1920×1080、30 fps、53.4 秒，H.264，无声，默认中文文案。

整片没有硬切：一束光（LanBeam 的「beam」）贯穿全片，每段的面板最后都会收成一个光点，飞到下一段再展开。

## 运行

依赖只在这个目录里，不碰根目录的 `package.json`。仓库根目录是 pnpm 工作区，所以安装时要加 `--ignore-workspace`：

```bash
cd video
pnpm install --ignore-workspace
pnpm studio      # 在浏览器里逐帧预览、拖时间轴
pnpm render      # 导出 out/lanbeam-promo.mp4（out/ 不进仓库）
```

快速预览可以用 `pnpm render --scale=0.5`，出图快很多。

## 分镜

| # | 时间（秒） | 内容 | 文件 |
| --- | --- | --- | --- |
| — | 0 – 4.8 | 光点落下变成图标，展开「LanBeam · 一束即达 · 无需云端」 | `scenes/Intro.tsx` |
| 01 | 4.2 – 10.6 | 雷达扫过，设备逐个出现，把文件拖到「客厅 · Mac mini」上 | `scenes/Discover.tsx` |
| 02 | 10.2 – 14.6 | 两台设备的屏幕同时显示校验码 483 · 921，核对一致后点「接收」 | `scenes/Send.tsx` |
| 03 | 14.6 – 20.6 | 传输进度：暂停、继续，传完 | `scenes/Send.tsx` |
| 04 | 20.0 – 25.8 | 快传文本：输入、发送，对方「已入剪贴板」 | `scenes/QuickText.tsx` |
| 05 | 25.2 – 31.8 | 浏览器接收：链接和二维码，手机扫码打开落地页下载 | `scenes/Share.tsx` |
| 06 | 31.2 – 37.6 | 跨网段配对：本机地址 + 6 位配对码，翻到对方屏幕粘贴配对链接、加入、核对校验码 | `scenes/Pair.tsx` |
| 07 | 37.0 – 42.0 | 信任圈：把「小敏的手机」拖进圈内，变成「已信任 · 自动接收」 | `scenes/Trust.tsx` |
| 08 | 41.4 – 48.0 | 真实 App 窗口：浅色 → 深色 → English，三平台 | `scenes/Look.tsx` |
| — | 47.4 – 53.4 | 结尾：图标、口号、v0.2.0、仓库地址 | `scenes/Outro.tsx` |

各段时间、交接点和面板位置都在 `src/lib/timeline.ts`，改节奏只动这一个文件。颜色在 `src/lib/theme.ts`，就是 App 深色主题的变量。

## 素材从哪来

- **面板是照真实界面还原的**，不是示意图。每个组件文件开头都写了对应的源码（比如 `ui/Radar.tsx` 对应 `src/pages/DevicesPage.tsx`），尺寸、颜色和文案都取自 `src/styles.css`、组件代码和 `src/i18n/locales/zh.json`。设备名、文件名和校验码用的是演示模式（`?demo=1`）的示例数据。手机上显示的是 `src-tauri/src/share.rs` 生成的落地页。
- **第 08 段是真截图**：`public/ui/*.webp`，由 `scripts/capture-ui.mjs` 从演示模式截取。界面改了以后，在仓库根目录 `pnpm install` 之后运行：

  ```bash
  node video/scripts/capture-ui.mjs
  ```

- 「✓ 校验码一致」标签、⌘V 键帽、设备名标签和每段的大标题是视频里加的说明，不是 App 的界面。

## 字体

Space Grotesk 和 IBM Plex Mono，和 App 用的是同一套（都是 OFL 许可，通过 `@fontsource` 打包）。中文跟 App 一样走系统字体，在 macOS 上就是苹方。如果在 Linux 上渲染，先装 Noto Sans SC，否则中文会退回到别的字体。

## 许可

Remotion 对个人和 3 人以内的公司免费；更大的公司需要购买公司许可，详见 <https://www.remotion.dev/license>。
