# Yonder（仓库 herdc）：Herdr 的 Android 客户端

Yonder 是 [Herdr](https://github.com/herdrdev/herdr-app) 桌面版在手机上的同级客户端：连上 PC（局域网 SSH，
外网走 Tailscale）后，看到的是同一个世界——同样的 space、tab、分屏、终端和 agent 状态，改动双向实时同步。

## 关键事实（决定了架构）

- Herdr 桌面版本身不持有工作区：它是 herdr-core（`hc`）的客户端，经 `hc connect` 说 JSON-RPC 2.0，
  `hello {framed:true}` 后切到分帧模式（`[channel u32 LE][kind u8][len u32 LE][payload]`）。
- 共享结构（space → tab → 分屏树）存在 core 的 **`herdr-app` 文档**里（`doc.get` / `doc.set`，带版本 CAS）。
  每块屏幕的焦点、缩放、当前 tab、已读状态是**私有视图**，不写进文档。手机端读写同一份文档，
  就和桌面多屏完全同构。
- 终端都是 core 的终端；Herdr 用 `herdr.space` / `herdr.tab` 标签认领，`herdr.seen` 标签跨屏共享已读回合。
- core 没有 TCP 监听；远程访问的官方方式就是 `ssh host '…herdr-core connect'`（herdr 自己连远程机器也这么做）。
  本机 core 已经聚合了它添加的远程机器（`vps/t7` 这类地址），手机连上 PC 的 core 即可看到全部机器。

## 总体结构

```
┌──────────── Android (nsh, Rust) ──────────────┐        ┌──────── PC ────────┐
│ app/      UI：space/tab/分屏/终端/agents/面板  │        │ sshd               │
│   └ herdc-engine  无头控制器（移植自 window.rs）│  SSH   │  └ sh -c '…        │
│        ├ herdc-model  model/layout/mirror      │◄──────►│     herdr-core     │
│        ├ herdc-wire   JSON-RPC + 分帧多路复用   │  exec  │     connect'       │
│        ├ herdc-term   VT（alacritty_terminal）  │ channel│        │           │
│        └ herdc-ssh    russh：密码/密钥/主机指纹  │        │   herdr-core ◄─ herdr-app
└────────────────────────────────────────────────┘        └────────────────────┘
```

| crate | 职责 |
|---|---|
| `herdc-wire` | 与传输无关的 core 客户端：分帧编解码、调用/应答路由、attach 流与 credit 流控、`events.subscribe` 快照 + 事件应用 |
| `herdc-ssh` | russh 客户端：密码 / 私钥登录，主机指纹首次信任并固定（TOFU），exec 通道跑 Herdr 同款 pipe 命令行（支持 `HC_SESSION`） |
| `herdc-model` | **原样复用** herdr-app 的 `layout.rs`、`model.rs`、`mirror.rs`（Apache-2.0，见 NOTICE），文档读写与桌面逐字段一致 |
| `herdc-engine` | 无头控制器：把 window.rs 里的业务逻辑（命令、认领、对账、已读共享、提醒、文档 CAS 刷写）移植成可测试的逻辑 |
| `herdc-term` | 终端仿真：喂字节、出带样式的行快照、滚动回看、按键编码（应用光标键、bracketed paste）、查询应答走 AUTOMATIC_INPUT |
| `app` | nsh Rust 应用：手机/平板自适应 UI；终端用 canvas 画网格，隐藏输入框接软键盘，快捷键栏补 Esc/Ctrl/Tab/方向键 |

## 复刻范围

core 能力和内置 mods 的功能全部复刻，界面按手机重新设计；用户自定义的 JS mods 和 layout 不在手机上运行
（它们依赖 gpui-shell 运行时）。

| 桌面 | 手机 |
|---|---|
| spaces mod（侧栏、按机器分组、拖动排序） | 抽屉：按机器分组的 space 列表，状态点，长按重命名/关闭/移动 |
| tabs mod / terminals-bar | 顶部 tab 条，状态汇总，settle / unsettle |
| 分屏树（row/column/stack） | 平板：真实分屏；手机：一次一个 pane + pane 切换条，缩放即全屏 |
| agents mod（working/blocked/done/idle，子 agent） | Agents 页：跨 space 列表，排序同桌面，不止一组时按 置顶 / 等你处理 / 完成 / 工作中 / 空闲 加小标题；点进 peek。blocked 的 agent 直接显示它问的最后几行和菜单选项（Codex / Claude Code 的编号菜单），点选项即发对应按键；“回复…”先按 Esc 离开菜单再发文字（菜单上直接回车会选中高亮的“是”）。工作区里，别处的 agent 等你时弹出同样的问题卡片，一直留到它不再等（正看着它时不显示），可点 ✕ 先放下 |
| peek（无 pane 的 worker） | Agents 页里子 agent 挂在父 agent 下；无 pane 的点开是 Peek 页，可 keep 成 tab 或跳到所在 pane |
| toast / 声音提醒 | 应用内 toast（blocked / done）。系统通知要等 NSH 发布通知 API（0.4.0 没有），后台常驻需要前台服务（打包的 APK 不能加 Java） |
| palette（命令面板） | 命令面板：同一套命令名和参数 |
| new-menu | 新建终端：本机探测到的 agent 程序、新 space、新建/打开 worktree |
| machine-add | 抽屉底部「Add a machine…」：电脑 `~/.ssh/config` 里的 host（含 Include）或手填，经 `machine.add` 添加；需要回答时提示在终端里跑的命令 |
| files / changes / worktree | 对应页面，经 `machine.exec`（mods 原样的命令行）实现，远程目录每 10 s 刷新；打开的文件用 nsh-base 的只读 `Editor` 显示，按文件名高亮 |
| help（快捷键表） | 手机没有快捷键，不复刻；命令页列出全部命令 |
| settings | 手机端设置：字号（终端上也可双指缩放）、打字时适配、语言（跟随系统 / English / 中文）、本机密钥 |

窗口类命令（最小化、最大化、桌面 app 更新、遥测）不适用于手机，不复刻。herdc 只做 Android：没有桌面入口。

## 关键设计决定

- **用 herdr-app 自己的模型代码**而不是按文档重写：文档格式、分屏增删的权重分配、命名规则、
  attention 规则都与桌面逐行一致，并且直接带上它们的单元测试。
- **SSH 是唯一的鉴权边界**，PC 端零安装。主机指纹首次连接后固定，变化时拒绝并提示；
  密码加密保存在手机上，不往电脑上写任何东西（不自动配置公钥）。密码方式被拒就是密码错了，不再拿它试 keyboard-interactive：macOS 的 sshd 在密码失败后走 keyboard-interactive 会一直不回应（OpenSSH 客户端也一样）。只有电脑不接受 `password` 方式时才用 keyboard-interactive；登录最多等 30 s；连上电脑、交换密钥最多等 10 s（只接连接不说话的也算连不上）。保存的密码被拒（电脑改了密码）就不再重试（每次失败都会记到账户上），提示条上给“重新输入密码”。电脑上已经有手机公钥的（用户自己从设置页复制过去加的），就用钥匙登录。
  不支持 RSA 主机/用户密钥：russh 的 `rsa` 特性锁定的 `rsa` 预发布版与 NSH 依赖冲突，只支持 ed25519 和 ECDSA。
- **只 attach 可见的 pane**，离屏即 CLOSE，节省流量和电量；尺寸遵循 core 的“最后活跃的观看者定尺寸”，
  手机打开或切到一个终端就按手机的行列数接管（像 tmux 归最近用的客户端）：桌面的 145 列缩到手机上字小得没法看。代价是 Mac 上这个终端暂时变窄，桌面一打字就拿回去，手机改回旁观。设置里关掉“终端按手机尺寸排版”时才是旧的规矩：默认 observe，用户开始输入才接管尺寸。关掉时的两个例外：手机自己开的终端一开始就按手机的行列数；
  没人持有的终端（处在 core 的 headless 尺寸）也由手机接管，别的屏幕一打字就放手、改回旁观。
  手机开过哪些终端随“上次的界面”一起存下来，app 重启后这些终端仍按手机的行列数（以前重启后变成旁观，显示成桌面的大小）。
- **断线重连**：指数退避，重连后重新取快照、重新读文档、重新 attach（core 保证程序不受影响）。
  app 回到前台时先用 3 s 探一下线路，不通就立刻重拨（系统冻结进程后，断掉的线要等 keepalive 超时才会发现）。
  断线原因是网络时，app 每秒探一次电脑的 SSH 端口，一通就重拨，不等引擎的退避（最长 10 s）。
- **启动即回到上次的电脑**：退出前在哪台电脑就直接连哪台；在“更多 → 断开”离开后，下次从电脑列表开始。
- **界面语言**：文字以英文写在代码里（`t!("…")`），中文在 `app/src/i18n.rs` 查表；测试保证每个 `t!` 都有译文。
  引擎来的文字（提醒、确认框、命令说明、状态词）在显示时翻译；拼出来的（`"{name} needs you"`）按 `TEMPLATES` 里的模板认。
- **终端手势**：上下滑翻历史，左右滑（超过 80 pt，且横向为主）切到下一个 / 上一个 tab，双指缩放字号，长按选文字。Agents 页里看 agent 的终端不切 tab。
- **tab 名字**：没起名的 tab 显示它里面 agent 的标题，不再只是一个数字；起了名的照旧。
- **密钥存放**：保存的密码和手机的 ed25519 私钥用 Android Keystore 里的 AES-GCM 密钥加密后再放进 NSH 的 Storage（`app/src/vault.rs`，JNI 调 `javax.crypto`）。Keystore 的密钥不出手机，所以备份或拷走的数据打不开；打不开时再要一次密码。NSH 生成的 manifest 带 `allowBackup`，改不了，所以靠加密而不是关备份。
  Android 12 以前拿不到 JVM，改用 app 自己的 AES-256-GCM 密钥（`ring`），放在 `no_backup` 目录、权限 0600，备份和换机迁移都不带它（前缀 `aside1:`）。
- **打包**：`app/apk.sh`（`--install` 顺带装到手机）：只打 arm64-v8a，fat LTO，用 `~/.config/herdc/release.jks` 正式签名（密码在 macOS 钥匙串 `herdc-release-keystore`）；没有时用 debug 签名。名字 Yonder（包名 `dev.yonder.app`；以前叫 Herdr / `dev.herdc.app`，换包名不能覆盖升级）。图标是自己的（`app/icon.svg`：一条小路通进远处的终端窗口，窗口是张眨眼微笑的脸，`icon.png` 由它渲染）。仓库和 crate 仍叫 herdc，用户看不到。
  build 号 = 提交数（`git rev-list --count HEAD`），和提交号一起编进 app，设置 → 关于里显示；更新记录在 `CHANGELOG.md`。
  nsh-base 关掉默认的 `highlight`（syntect 和语法包），app 不做代码着色；LTO 本来就裁掉了它，APK 没变小，只是少编译一些。libnsh.so 里 24 MB 是 `.text`。
- **诊断信息**：设置 → 关于 → 复制诊断信息：版本、系统、偏好、密钥怎么存的、电脑地址和用户、线路和各机器的状态、数量。不含密码和密钥。
- **CI**：`.github/workflows/check.yml` 在 Ubuntu 上装指定版本的 herdr-core（`HC_VERSION`），跑 `scripts/check.sh`。
- **大量输出限速**：viewer 以 512 KiB/s 归还 credit（`Attach::pace`）。输出超过这个速度持续 1 s，就算“落后”，断开重新 attach，从当前屏幕开始，不再一个字节一个字节追 core 那边最多 4 MiB 的积压。省流量、省电，刷屏结束后 0.5 s 内看到最后的输出。
- **后台不收输出**：app 进后台时 viewer 放开终端（`Engine::set_shown(false)`），状态事件照收；回前台从当前屏幕重新 attach。以前后台刷屏 24 s 照样收 16 MB。
- **SSH 压缩**：优先 `zlib@openssh.com`，其次 `zlib`，电脑不支持就不压缩。
- **重画限频**：终端最多每 33 ms 画一次（约 30 fps）；刷屏时主要耗 CPU 的是绘制，不是解析。
- **外接键盘**：终端有键盘焦点时，Ctrl/Alt + 字母、方向键、Home/End/PgUp/PgDn、Esc、Tab、F1–F12 直接发给终端（NSH `on_key_with`，`When::Always`）；Ctrl+Q、F11 系统保留。
- **拖动分屏**：宽屏（≥ 720 pt）时，两个按权重分的窗格之间有一条可拖的分界线（`herdc_engine::seams`）。松手后两边按新位置重新分权重（`layout.resize`），每边至少一成；固定 px 的窗格不动。
- **先画上次的样子**：每台电脑连上并读完布局后，把它的 View 存起来（`seen.<id>`，有变化时最多 10 s 后存一次，进后台、断开时也存；超过 256 KiB 就不存 terminals）。
  再打开时先画这份，线路状态用实时的，读完新的就换成新的。电脑列表在每台电脑下列出它上次的 spaces，圆点是当时最需要关注的状态；点一个 space，连上后直接到那里；直接打开时回到上次看的 space（engine 不存 `view.json`，靠这份）。
- **多个 session**：添加电脑时可以填 `HC_SESSION`（只允许字母、数字和 `._-/`，因为要拼进远端 shell），同一台电脑的不同 session 是不同的条目（`id` 末尾加 `/session`）。
- **抽屉列表**：本机用电脑自己的名字，保留机器名大小写，本机在前、远程机器按名字排序。机器标题的小箭头可折叠整组空间，按电脑和机器存在 prefs，重启后保留；当前空间所在机器也允许折叠，选中空间不会自动展开。空间不显示标签页数量，只有非 main/master、不同于空间名及仓库目录名的分支才显示第二行，没内容就只画一行。
- **Agents 列表**：子 agent 已在别人的后代树中时不再占顶层行，置顶例外；已保留的后代优先取 AgentView 的状态，done 仍归 idle。主文字按桌面 Priority 视图显示空间加有意义的标签页或 agent 标题，数字自动名仅在重名时补上；空标题回退程序/app 名，远程行追加机器名。状态旁显示变化距今的 <1m / Nm / Nh / Nd，每 30 s 刷新。时间用 core 记录的 `status_ms`（电脑上状态变化的那一刻，不是手机连上的时刻），没有时才用引擎自己看到变化的时间；旧缓存缺失时默认为 0（暂不显示时间）。
- **抽屉排序**：每个 space 右边的 ≡ 可以上下拖，按行高算移动几格，松手执行 `space.move`（只在同一台机器的 spaces 里移动）。
- **按键栏可编辑**：设置 → 按键栏：栏上的键可以前移、后移、移除，下面是可以加上的键（`\` `` ` `` `:` 等符号、Del、Ins、F1–F12）。存在 prefs 里，⌄ 始终在最后。
- **焦点不共享**：手机上的焦点、放大、当前 tab 只记在本机引擎里，不发给 core，不会让桌面跟着跳。

## 测试

1. 单元测试：模型（随 herdr-app 测试一起迁移）、分帧编解码、事件应用、按键编码、VT 快照。
2. 集成测试：对本机隔离的 core 会话（`HC_SESSION=herdc-e2e`）跑真实协议，并经本机 sshd 走完整 SSH 路径。
   绝不连接用户正在使用的 core。
3. 端到端：
   - 在隔离会话里用 `codex -m gpt-6.1-sol` 跑真实 agent，验证状态流转、提醒、输入、已读共享；
   - 让 codex sol 通过 adb 截图/点击按脚本做 UI 验收（`e2e/acceptance/*.md`），真机测试。
