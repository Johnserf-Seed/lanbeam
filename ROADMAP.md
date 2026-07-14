# LanBeam 后端路线图（M4 – M9）

> 基于 2026-07-12 对 `src-tauri` 的完整代码审计。UI v2 已就位，本路线图把界面外壳逐一接上真实 Rust 后端。
> 里程碑编号延续仓库惯例（M0 桥接自检 / M1 发现 / M2 加密信道 / M3 文件传输）。
>
> **两条贯穿性规则**
> 1. **协议兼容**：给现有 JSON 结构**加字段**（`#[serde(default)] Option<T>`）双向兼容；**加变体/kind 字节**会杀死旧对端 —— 任何新变体必须先落地 M4 的 `Hello` 版本协商。
> 2. **每个里程碑交付 = Rust 命令/事件 + 前端接线（拆掉对应 milestoneNote 提示）+ 回环集成测试**。
>
> **进度**（2026-07-12）：✅ M4 · ✅ M5 · ✅ M6 · ✅ M7 · ✅ **M8（axum 一次性分享服务：token/TTL/次数/停止即失效，仅局域网、按索引服务文件；ShareModal 接真；发现包广播分享端口）**。**五个核心后端里程碑全部交付**，界面所有外壳已接真实 Rust。测试：250 单元 + 13 集成（Rust）· 365（前端 Vitest，28 文件），全绿。三平台 CI（Windows / macOS / Linux）见 `.github/workflows/ci.yml`。
> - M4 Hello 协商 / 信任存储 / 安全加固 / 日志诊断
> - M5 本机 IP / 下载目录·端口 / 托盘 / 通知 / 自启 / Alt+Space 可开关快捷键·默认关 / 重置身份
> - M6 取消·暂停（有时限）/ SHA-256 校验 / 断点续传 / 冲突策略 / 自动整理 / 并发·限速 / 逐文件进度
> - M7 配对码·QR / IP 直连 / 快传文本 + 剪贴板（传输层 version 2 门控）
> - M8 浏览器接收（HTTP 回退）
>
> **M9 可选润色**：✅ **EXIF 抹除已交付**（`img-parts` 无重编码剥离 JPEG/PNG/WebP 的 EXIF/ICC/XMP + Extended-XMP；HEIC/TIFF/GIF/RAW 透传；剥离后 size+SHA-256 按流出字节重算、临时文件 RAII 全路径清理、Unix 0600/0700 权限；`stripExif` 开关真实化）。⬜ 检查更新（`tauri-plugin-updater`，**卡在发布服务器 + 签名密钥基础设施**，你提供前无法完成）· ~~⬜ 生效网络 SSID 检测~~ → **已放弃**：那个下拉框接的是空气（无后端字段、无命令、无读取方），而唯一诚实的实现是 Windows-only 的 WLAN FFI —— 在 macOS/Linux 上会静默失效，等于用一个新谎言换掉旧谎言。控件已从设置页移除；「可被发现」开关本来就在按需做这件事。
>
> **注**：暂停为「有时限」语义 —— 超过 50s 自动恢复（受帧层单向流 + Noise nonce 约束，无法安全加跨向 keepalive；已在代码中详述）。续传 offsets 用稀疏编码（仅非零、限帧预算），超预算文件回退到从头传。

---

## M4 · 基础加固与协议前置（其余一切的地基）· ✅ 已交付

| # | 任务 | 关键锚点 |
|---|------|---------|
| 4.1 | `Settings` 全字段加 `#[serde(default)]`，解析失败不再整体回退默认（逐字段合并），为后续所有新设置铺路 | `settings.rs:11-45` |
| 4.2 | 启用握手后 `Hello{versions}` + `DeviceInfo{name,...}` 交换（双方版本能力协商 + 接收端拿到发送方友好名）；旧对端（无 Hello）按 v1 处理 | `protocol.rs:24-27`, `transfer.rs:291`, `run_send` 前置 |
| 4.3 | 修复 `connect_device`：改为 Hello→Bye 规范会话（当前发明文 `"hello"`，对端报协议错误；指纹重核对 UI 依赖它） | `commands.rs:40` |
| 4.4 | **信任存储后端化**：`trusted.json`（deviceId → {name, autoAccept, pairedAt}）+ `list/set/remove_trusted` 命令；`handle_incoming` 在提示前查表自动接收（代码注释已预留）；前端 localStorage 信任记录迁移导入 | `transfer.rs:305`, 新 `trust.rs` |
| 4.5 | 安全加固：clamp `file_count`/manifest 总量（防预信任内存 DoS）；`pending` 表拒绝重复 transfer_id（防挤掉他人确认框）；拨号 + 逐帧 I/O 超时；中断时清理正在写入的半成品文件；`transfer_error` 区分「对方拒绝」与真错误（加 `code` 字段） | `transfer.rs:172,307`, `transport/mod.rs:96` |
| 4.6 | 日志体系：`tracing` + `tauri-plugin-log`（级别读设置）替换全部 `eprintln!`；「导出诊断包」命令（打包日志 + 网络自检）；UDP 只发不收 / TCP 落到临时端口等**静默降级以事件上报 UI**；删掉 M0 遗留 `hello_tick` 线程；`completed` 表加淘汰 | `lib.rs:166`, 8 处 eprintln |

**验收**：新旧版本互传不回退；升级后设置不丢；重复 id / 超大 manifest 被拒的单测；两实例回环全绿。

## M5 · 系统集成（与协议无关，全部增量，见效最快）· ✅ 已交付

| # | 任务 | 关键锚点 |
|---|------|---------|
| 5.1 | `get_network_info` 命令（本机 IP/广播地址；`discovery::interfaces` 转 pub）→ 侧栏与设置页显示真实 IP | `discovery/interfaces.rs:11` |
| 5.2 | `set_download_dir`（系统目录选择器）+ 监听端口设置（`Settings.port`，重启生效；发现层自动广播实际端口，无协议改动） | `transport/mod.rs:52` |
| 5.3 | 系统托盘（Cargo `tray-icon` feature）+ 「关闭最小化到托盘」（`CloseRequested` → hide，读 trayClose 设置）；single-instance 聚焦时同时取消隐藏 | `lib.rs:107`, `Cargo.toml:21` |
| 5.4 | 系统通知 `tauri-plugin-notification`：收到请求 / 传输完成（读 notifSys 设置） | `transfer.rs:313,360` |
| 5.5 | 开机自启 `tauri-plugin-autostart`；全局快捷键 `tauri-plugin-global-shortcut`（唤起主窗口/快传文本） | `lib.rs:86` |
| 5.6 | 网络接口过滤设置（enumerate 处单点过滤，2s 内生效，无需重启） | `discovery/mod.rs:135-174` |
| 5.7 | 重置本机身份：墓碑广播 → 删 keychain 条目 → 清信任存储 → `app.restart()`（身份被启动时快照，热切换不现实） | `identity.rs:14`, `discovery/mod.rs:195` |
| 5.8 | 设置页「mDNS 发现」行改名「局域网发现」并与可见性统一（发现层是自研 UDP，非 mDNS；避免虚假宣称） | UI 文案 |

## M6 · 传输引擎强化 · ✅ 已交付

| # | 任务 | 协议影响 |
|---|------|---------|
| 6.1 | **取消**：`AppState` 加 sessionId→CancellationToken 表，收发 chunk 循环 `select!`，`cancel_transfer` 命令；对端见连接断开走现有错误路径。带内优雅 Cancel 变体待 Hello 门控后加 | 无（本地）/ 变体需门控 |
| 6.2 | **暂停/继续**（会话内）：token + `Notify`，发送侧停读即天然背压 | 无 |
| 6.3 | **SHA-256 校验**：`FileMeta` 加 `#[serde(default)] sha256: Option<String>`（sha2 crate，读边流式哈希），接收端核对后再 ack；verifyHash 开关生效；detail 抽屉逐文件「已校验」变真 | 加字段，双向兼容 |
| 6.4 | **断点续传**：`FileSendReply` 加 `offsets: Option<Vec<ResumeOffset>>`（稀疏，`{index, offset}`）；`resolve_and_open` 加"续写模式"（保留全部包含性检查）；错误路径**有条件**保留半成品：中断保留、校验失败只删损坏的那一个（同批已通过校验的兄弟文件保留）；持久化断点状态 + 设置页可见可清理；发送端 seek 续传；UI 「立即重试 · 从断点继续」变真 | 加字段，双向兼容 |
| 6.5 | **重名冲突策略**：策略枚举贯穿 `run_receive`（rename=现状 / overwrite=防符号链接替换 / ask=收前检测冲突 → 新事件 + 前端补 ConflictModal，`pending` oneshot 升级为决策结构体） | 无 |
| 6.6 | **自动整理**：按设备（用 M4 DeviceInfo 的友好名）/ 按日期子文件夹 | 无 |
| 6.7 | **并发上限**（accept 循环 + send_files 各一个 Semaphore，超限礼貌拒绝）与**速率限制**（chunk 循环令牌桶） | 无 |
| 6.8 | 逐文件进度事件（`file_index`）→ detail 抽屉真实逐文件进度条 | 无 |

**验收**：取消/续传/哈希不匹配/冲突三策略的回环集成测试；新旧版本互传矩阵。

## M7 · 配对与快传文本 · ✅ 已交付

| # | 任务 |
|---|------|
| 7.1 | 配对：配对码（6 位，10 分钟 TTL）+ QR payload 命令（deviceId+name+ip:port）；兑换 = 带内 PairRequest/PairConfirm（Hello 门控）→ **双方各自显示同一个 SAS，用户确认一致后前端调 `set_trusted` 写入信任存储（握手本身不授予任何信任）**；PairModal 接真 |
| 7.2 | IP 直连：`connect_by_addr(ip:port)` 命令（拨号 + Hello + 入表），设备页「IP 直连」接真 |
| 7.3 | 快传文本：`send_text` 命令 + `handle_incoming` 首帧分派（TextSend → 信任/策略门控 + 按源限流 → `text_received` 事件 + **带 `delivered`/`reason` 的 Ack**：被丢弃时发送方 `send_text` 真实失败，不再假报成功）；`tauri-plugin-clipboard-manager` 写剪贴板（读 clipShare 设置）；收件箱文本条目 + 全局快捷键唤起快传弹窗 |

## M8 · 浏览器接收（HTTP 回退）· ✅ 已交付

| # | 任务 |
|---|------|
| 8.1 | axum 一次性分享服务：token URL + TTL + **每文件**下载次数上限 + 停止即失效；**绑 0.0.0.0，「仅限局域网」由每请求中间件把关**；**任何分享（含单文件）都出品牌落地页**，文件按索引单独下载（无 zip 流） |
| 8.2 | ShareModal 接真（开始/停止/复制链接/二维码/有效期/次数） |
| 8.3 | （可选）DiscoveryPacket 加 `http: Option<u16>` 字段广播分享端口（加字段兼容，注意 2048 字节收包上限） |

## M9 · 媒体与润色（可选项）

- EXIF 抹除（`img-parts`/`little_exif`，发送端临时副本，保证 size/hash 一致）→ stripExif 设置 + 发送前单次开关
- 检查更新（`tauri-plugin-updater`）

---

### 顺序依据

M4 是硬前置（settings 地雷 + 版本协商 + 信任表被 M6/M7 依赖）；M5 全部增量、见效最快；M6 动传输主路径，测试成本最高；M7/M8 是独立新子系统，可与 M6 并行。
