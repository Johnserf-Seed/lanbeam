## Downloads · 下载

Pick the file for your system from **Assets** below · 在下方 **Assets** 中选择对应系统的文件:

| System · 系统 | File · 文件 |
| --- | --- |
| **Windows** 10 / 11 (x64) — installer · 安装版 | `LanBeam_${VERSION}_x64-setup.exe` |
| Windows — MSI (managed deployment · 批量部署) | `LanBeam_${VERSION}_x64_en-US.msi` |
| Windows — portable, no install · 便携免安装版 | `LanBeam_${VERSION}_x64_portable.exe` |
| **macOS** 10.15+ (Apple Silicon & Intel) | `LanBeam_${VERSION}_universal.dmg` |
| **Linux** — Debian / Ubuntu | `LanBeam_${VERSION}_amd64.deb` |
| Linux — Fedora / openSUSE | `LanBeam-${VERSION}-1.x86_64.rpm` |
| Linux — any distro · 通用 | `LanBeam_${VERSION}_amd64.AppImage` |

Install LanBeam on **every** device you want to transfer between; for a phone or a machine without it, use *Browser receive* (a one-time download link). · 需要互传的**每台**设备都装上 LanBeam;没装的设备(比如手机)用「浏览器接收」的一次性下载链接。

### First launch · 首次运行

These builds are **not code-signed** yet, so the OS warns once · 安装包**尚未做代码签名**,首次打开系统会拦一次:

- **Windows** — SmartScreen: *More info → Run anyway*. Allow LanBeam on **private** networks when the firewall asks. · SmartScreen 点「更多信息 → 仍要运行」;防火墙弹窗时允许其访问**专用网络**。
- **macOS** — open it once, then *System Settings → Privacy & Security → Open Anyway*; allow **Local Network** access when asked. · 先打开一次,再到「系统设置 → 隐私与安全性 → 仍要打开」;询问时允许**本地网络**访问。
- **Linux** — `chmod +x LanBeam_*.AppImage`, or install the package: `sudo apt install ./LanBeam_*.deb` / `sudo dnf install ./LanBeam-*.rpm`.

### Verify · 校验

```bash
sha256sum -c SHA256SUMS.txt --ignore-missing
gh attestation verify <file> --repo ${GH_REPO}
```

The second command proves the file was built by this repository's release workflow from the tagged commit. · 第二条命令可证明文件确实由本仓库的发布工作流、从该 tag 的提交构建而来。
