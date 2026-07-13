# LanBeam

[English](README.md) · **简体中文**

> 快速、私密、点对点的局域网文件传输 —— 不上云、不注册、不中转。

**LanBeam** 在同一局域网内的设备之间**直接**传输文件(和快传文本)。每次传输都走端到端加密信道,全程不碰公网。对于没装 LanBeam 的设备,它还能起一个一次性的、基于链接的 HTTP 下载 —— 手机浏览器不装任何 App 就能取走文件。

基于 **Tauri 2**(Rust 内核 + WebView 界面)。发布产物是单个自包含可执行文件(约 8 MB);Windows 上的网页运行时用系统自带的 WebView2,不额外内嵌任何东西。

- **状态:** `v0.1.0` —— 功能完整,发版前。
- **平台:** Windows(主要目标);macOS / Linux 由 Tauri 技术栈支持。
- **语言:** 英文 & 简体中文(应用内可切换)。

---

## 功能

### 传输
- **局域网内直接 P2P** —— 设备通过轻量的自研 UDP 发现协议(**非** mDNS)互相找到并直连,中间没有服务器。
- **端到端加密** —— 每次会话都是一次 `Noise_XX_25519_ChaChaPoly_BLAKE2s` 握手(基于 [`snow`](https://crates.io/crates/snow) crate)。设备以静态 X25519 密钥标识;一段简短的 **SAS(校验码)** 让你可以带外核对对端。
- **完整性校验** —— 文件流式过 SHA-256,接收端在落地前重新哈希核对(可开关)。
- **续传、暂停与取消** —— 中断的传输从持久化的字节偏移继续;暂停施加背压(有时限,会自动恢复);取消立即释放并发槽。
- **重名冲突策略** —— `rename`(去重)、`overwrite`(崩溃安全:先写临时文件、整批成功后才原子替换)、或 `ask`。
- **自动整理** —— 按发送设备或按日期归档收到的文件。
- **并发上限与限速**,以及逐文件进度。

### 配对与触达
- **配对** —— 6 位配对码(10 分钟有效)、可扫二维码,或 `lanbeam://pair` 深链接。配对会在双方互存指纹,之后同网自动识别。
- **IP 直连** —— 用于自动发现看不到的对端(不同子网)。
- **快传文本** —— 通过加密信道发一段文字/链接,可选择直接落到对方剪贴板。
- **浏览器接收** —— 把指定的一组文件通过一次性 HTTP 分享出去:不可猜的 token 链接 + 有效期 + 下载次数上限 + 一键停止 —— 仅限局域网,文件按索引寻址(无路径/穿越面)。

### 隐私与系统集成
- **元数据抹除** —— 发送时剥离 JPEG/PNG/WebP 的 EXIF / ICC / XMP,采用容器级手术(不重编码,像素逐字节不变)。
- **信任存储** —— 记住的对端(`deviceId → 名称、自动接收、配对时间`),并在指纹变化时告警。
- 系统托盘 + 关闭最小化到托盘、系统通知、开机自启、可选的全局快捷键、网络接口过滤,以及一键重置身份。

---

## 安全模型(速览)

- 设备的 X25519 私钥存在 **操作系统钥匙串**(`keyring`)里,绝不明文落盘。
- 所有对端流量端到端加密(Noise);应用不打开任何公网连接。
- 不可信输入按敌对处理:manifest 的文件名/大小/数量都有上限,接收路径针对目录穿越和 Windows 保留名/ADS 花招做净化,配对是 TOFU + 用户确认 + 带外 SAS 核对。`lanbeam://` 深链接**只会预填**配对表单 —— 绝不自行授予信任。
- 浏览器分享服务器绑定局域网,只按索引服务那组明确的文件,每次请求都用 token + 有效期 + 下载次数重新把关。

依赖漏洞用 `cargo audit` 跟踪;已接受的传递依赖告警记录在 [`src-tauri/.cargo/audit.toml`](src-tauri/.cargo/audit.toml)。

---

## 技术栈

| 层 | 选型 |
|---|---|
| 外壳 | Tauri 2(Rust)+ WebView2 |
| 后端 | Rust、`tokio`、`snow`(Noise)、自研 UDP 发现、`axum`(分享服务)、`img-parts`(EXIF)、`keyring`、`sha2` |
| 前端 | React 19、TypeScript(strict)、Vite、`zustand`、`react-i18next` |
| 工具 | pnpm、Biome(lint/format)、Vitest、`cargo clippy` / `rustfmt` / `cargo-llvm-cov` |

---

## 快速开始

### 前置依赖
- [Rust](https://www.rust-lang.org/tools/install) —— stable 工具链(≥ 1.85)
- [Node.js](https://nodejs.org) ≥ 20.19 和 [pnpm](https://pnpm.io)(`npm i -g pnpm`)
- 对应操作系统的 [Tauri 2 系统前置依赖](https://tauri.app/start/prerequisites/)(Windows 上:MSVC 构建工具 + WebView2 运行时,后者 Windows 10/11 自带)。

### 安装
```bash
pnpm install
```

### 开发
```bash
pnpm tauri dev      # 带热重载运行桌面应用
# 或只在浏览器里跑 Web 界面(后端调用回退到 demo 桩):
pnpm dev
```

### 构建
```bash
pnpm tauri build    # 产出应用二进制 + NSIS 安装包,位于
                    # src-tauri/target/release/(以及 .../bundle/nsis/)
```

---

## 项目结构

```
.
├── src/                  # React + TypeScript 前端
│   ├── bridge/api.ts     #   Tauri 命令/事件的类型化封装(+ 浏览器桩)
│   ├── lib/store.ts      #   zustand 状态仓库(需要时持久化)
│   ├── components/       #   弹窗、外壳、共享 UI 原语
│   ├── pages/            #   设备 / 传输 / 收件箱 / 已信任 / 设置
│   └── i18n/             #   en + zh 语言包
├── src-tauri/            # Rust 后端(应用内核)
│   └── src/
│       ├── discovery/    #   UDP 局域网发现 + 接口枚举
│       ├── transport/    #   Noise 握手 + 帧
│       ├── transfer.rs   #   收发状态机、续传、完整性、冲突
│       ├── share.rs      #   axum 一次性浏览器分享服务
│       ├── sanitize.rs   #   接收路径安全(唯一的写入choke point)
│       ├── trust.rs      #   信任存储  ·  exif.rs —— 元数据抹除
│       └── commands.rs   #   Tauri 命令面
├── ROADMAP.md            # 后端里程碑 M4–M9(全部已交付)
└── vitest.config.ts
```

---

## 测试

```bash
# 前端(Vitest + Testing Library)
pnpm test
pnpm test:coverage

# 后端(在 src-tauri/ 下)
cargo test
cargo clippy --all-targets
cargo fmt --check
cargo llvm-cov --summary-only -- --test-threads=1   # 覆盖率
```

> **Windows 注意:** 后端测试串行跑(`-- --test-threads=1`)。MockRuntime 集成测试在并行时可能偶发原生层崩溃(`0xc0000005`),那是环境抖动、不是逻辑错误,重跑即可。

---

## 打包与分发

- `pnpm tauri build` 产出单个 **NSIS** 安装包。裸的 `src-tauri/target/release/lanbeam.exe` 也能独立运行(即"绿色"便携版)。
- **不内嵌 WebView2** —— 应用使用系统 Evergreen 运行时,Windows 10/11 自带。
- `lanbeam://` 协议由安装器注册;便携版首次运行时会自行注册该协议(按用户,写 HKCU)。

---

## 许可证

以 [MIT 许可证](LICENSE) 发布。第三方组件及其许可证在应用内 **设置 → 关于 → 开源许可** 中列出。
