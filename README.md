# Yonder

[Herdr](https://github.com/herdrdev/herdr-app) 的 Android 客户端。通过 SSH 连上电脑（局域网，外网走 Tailscale）后，手机上看到的就是桌面上那一套：同样的 space、tab、分屏、终端和 agent 状态，两边实时同步。

- **电脑上什么都不用装**：只要开着 sshd、跑着 herdr-core。手机用密码或密钥登录，主机指纹第一次确认后就固定下来。
- **Agents**：跨 space 列出所有 agent。等你处理的 agent 直接显示它在问什么，点一下就能回答。
- **主 agent**：给一个 agent 单独开一页，像聊天一样和它说话。
- **终端**：快捷键栏补上 Esc、Ctrl、Tab 和方向键，可以双指缩放。
- **文件 / 改动 / worktree**：浏览文件、看 diff、提交。

只支持 Android（arm64，Android 9 及以上）。界面有中文和英文。

## 构建

需要 [NSH](https://crates.io/crates/nsh-core) 0.4、Rust、Android SDK/NDK 和 JDK 21。路径写在 `env.sh` 里，按自己的机器改。

```sh
. ./env.sh
sh scripts/check.sh            # clippy + 全部测试，不需要手机
sh app/apk.sh --debug          # 调试签名的 APK：app/.nsh/out/Yonder.apk
sh app/apk.sh --install        # 正式签名，并装到连着的手机上
```

不加 `--debug` 时用正式密钥签名：密钥在 `~/.config/herdc/release.jks`（可用 `HERDC_KEYSTORE` 换路径），密码从 macOS 钥匙串的 `herdc-release-keystore` 读。缺了任何一样就停下，不会退回调试签名。

## 目录

| 路径 | 内容 |
|---|---|
| `app/` | NSH 应用：界面 |
| `crates/herdc-engine` | 无头控制器：命令、对账、已读、提醒 |
| `crates/herdc-model` | 沿用 herdr-app 的模型代码（Apache-2.0，见 `app/NOTICE`） |
| `crates/herdc-wire` | herdr-core 的 JSON-RPC 与分帧协议 |
| `crates/herdc-ssh` | SSH 客户端 |
| `crates/herdc-term` | 终端仿真 |
| `e2e/` | 真机脚本和验收清单 |

设计和取舍见 [`docs/DESIGN.md`](docs/DESIGN.md)，改动记录见 [`CHANGELOG.md`](CHANGELOG.md)。
