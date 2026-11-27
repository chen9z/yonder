# Yonder 规格

本文是 Yonder 的唯一依据：范围、命令、架构以这里为准，改动时和代码同一次提交更新。
写法：已经在代码里的写「已实现」，还没做的写「计划（包名）」；材料和代码不一致时按代码写，并记进最后一节。
`docs/DESIGN.md` 记的是手机 app 的界面和交互细节，和本文不冲突时照它；冲突时以本文为准。

示例名：主 agent 叫 nova，常驻 bot 叫 trainer、ops，机器叫 gpu-box、homelab，路径用 `/tmp/example`。

## 1. 范围

Yonder 是跑在自己电脑上的个人 agent 底座和客户端。agent 本身跑在 herdr-core（`hc`）的终端里，人随时能在桌面 Herdr、手机或桌面 Yonder 里看、插话、接管。Yonder 负责 agent 之外的部分：

| 部件 | 是什么 | 状态 |
|---|---|---|
| `yonder` | 命令行：台账（任务、提醒）、`yonderd` 的状态；以后加 bot 管理和交接 | 已实现一部分 |
| `yonderd` | 常驻后台：提醒到点、跑带检查的提醒、把结果投递给主 agent 的终端 | 已实现 |
| 存储 | SQLite `ledger.db`（WAL） | 已实现 |
| App | 同一个 NSH app：Android 版和 macOS 版（`Yonder.app`） | 已实现（连电脑、看 agents 和终端） |
| 自定义 bot | 用户自己定义常驻或一次性的 bot，由 Yonder 起停、同步、交接 | 定义和查看已实现（B1）；其余计划（B2–B5、B4） |

不做：

- 不调大模型，不自己当 agent harness；agent 一律由 `hc` 跑。
- 不做记忆搜索、记忆自动整理（见「记忆」一节）。
- 暂停（排在 bot 之后，另行排期）：台账页、桌面管理页、IM（Discord、Telegram）、锁屏推送。

公开仓库：代码、注释、测试、文档里不出现个人信息（家目录绝对路径、真实机器名、Tailscale 主机、域名、真实 agent 名、会话 id）。个人的东西只放在仓库外的配置文件里。

## 2. 领域模型

类型在 `yonder-model`（只依赖 serde、serde_json，Android 也能编），存取和规则在 `yonder-core`。时间一律存 UTC `YYYY-MM-DDTHH:MM:SSZ`。

### 2.1 任务（已实现）

- id `T-0001` 起递增；`title`；`source`（可空，如 `hc:nova`）。
- 状态：`open` → `done` 或 `cancelled`，两个都是终态。
- 设计稿里更大的任务模型（领域、阻塞方、证据门禁、打回计数、受众）没有实现，也不在当前排期里。

### 2.2 提醒（已实现）

- id `R-0001`；`content` 最长 4096 字节，超长报 `bad_input`。
- 三种：一次性（`--in`、`--at`）、周期（`--every`：`5m`、`2h`、`1d`、`daily@09:00`、`weekly:1@09:00`）、带检查（`--every` 加 `--check`）。
- 状态：`active` → `fired`（一次性到点）、`cancelled`（取消）、`expired`（检查到期）；`fired`、`cancelled`、`expired` 都是终态。周期提醒到点后仍是 `active`，下次时间往后挪。
- 带检查的提醒：设置时当场跑一次命令，失败就不建；成功存输出（trim 后）的 SHA-256 作基线，不存原文。之后每次到点跑一次：哈希变了就投递 `check_changed`；连续失败 3 次投递一次 `check_failing`，成功后清零；`--once` 时第一次变化后停；检查期限默认 7 天，最长 90 天或 `forever`，到期投递 `check_expired` 并转 `expired`。
- `snooze` 把下次时间推到「现在 + 时长」。

### 2.3 投递（已实现）

`yonderd` 发给主 agent 的每一条触发记一行投递，和推进提醒写在同一个事务里，`(reminder_id, trigger_kind, due_at)` 唯一，所以同一次触发只记一次。状态机：

| 从 | 到 | 条件 |
|---|---|---|
| `pending` | `sending` | 找到主 agent 的终端，开始 `hc send` |
| `sending` | `sent` | `hc send` 成功 |
| `sending` | `pending` | hc 在打字前就拒绝（错误 kind 在白名单里，见 `yonderd` 一节），或进程没起来 |
| `sending` | `lost` | 其他任何失败、超时、被信号打断；可能已经送到，所以不再重发 |
| `pending` | `lost` | 单条就超过一批的上限（256 KiB） |
| `sending`（重启时遗留） | `lost` | 上次发到一半 yonderd 退出了 |

原则是「最多送一次」：可能送到过的不重发，记成 `lost`，`yonder ctl status` 显示 lost 的数量。找不到主 agent 的终端时投递留在 `pending`，下一轮再试。

### 2.4 信封 v1（已实现）

写进主 agent 终端的格式：一行头 `[yonder key=value ...]`，然后正文。例：

```text
[yonder kind=remind id=R-0003 trigger=check_changed due=2026-11-26T16:21:25Z]
文件变了
Check output now:
<payload untrusted="true" source="check_output">
b
</payload>
```

外部内容（检查输出）一律放进 `<payload untrusted="true">`；正文里行首的 `[yonder` 转义成 `\[yonder`，`</payload>` 转义成 `<\/payload>`，头里的值把空白和方括号换成 `_`，外部内容伪造不了信封头。

### 2.5 bot（定义已实现，B1）

bot 定义放在 `<home>/bots/<名字>/`，名字限 `^[a-z][a-z0-9-]{0,31}$`，必须和目录名一致。`bot.toml`：

```toml
name        = "trainer"           # 必填
description = "GPU 训练"          # 必填，一句话
kind        = "persistent"        # persistent | oneoff，必填
machine     = "local"             # local 或 hc 机器名，默认 local
cwd         = "~/bots/trainer"    # persistent 必填；oneoff 可空（启动时由任务指定）
harness     = "codex"             # codex | pi | claude | 其他字符串，必填
model       = ""                  # 可空
args        = []                  # 可空
memory_id   = "trainer"           # 可空，见「记忆」一节
env         = ["EXAMPLE_API_KEY"] # 可空，只写变量名
```

- `env` 的名字限 `^[A-Z_][A-Z0-9_]*$`，拒绝系统变量和 Yonder 自己的：`PATH HOME SHELL USER LOGNAME PWD TMPDIR TERM LANG TZ`，以及 `YONDER_`、`LC_`、`DYLD_`、`LD_` 开头的。同名写两次是警告。值只存在钥匙串里，B3 起 bot 时注入；不进 `bot.toml`、不进提示词、不进日志。
- 未知字段：`bot check` 报警告，不失败（以后加字段时旧版本还能读）。
- 校验（`crates/yonder-core/src/bot.rs`）：error 是 toml 语法错（带行号）、没有 `bot.toml`、必填字段缺了或为空、字段类型不对、名字格式不对、`name` 和目录名不一致、`kind` 不是 `persistent` / `oneoff`、persistent 没有 `cwd`、`env` 名字不合法；有 error 的定义算 `invalid`，不能用。warning 是未知字段、`env` 重复、`description` 还是 `bot new` 写的占位（`TODO: ` 开头）。
- 读定义永远不会整体失败：一个坏定义只让它自己 `invalid`，不挡别的。`bots/` 下不是目录的、`.` 开头的都不算 bot。
- bot 目录和 `bot.toml` 不能是符号链接：是的话（或 `bot.toml` 不是普通文件）这个 bot 标 `invalid` 并写明原因，不读链接指向的东西，`bot ls` 照样列出它、不挡别的。`bots/` 本身是符号链接时 `bot ls/show/check/new` 都报 `symlink`。`bot new` 的三个文件都是排他创建，那里已经有东西（文件或链接）就报错，不覆盖。
- `bot new` 只生成文件，之后 Yonder 不改 bot 目录里的任何东西。生成的 `bot.toml` 字段都在顶层，没有表，所以在文件末尾追加一行（如 `cwd = ...`）就是加一个字段；`cwd` 等可选字段以注释给出。不带 `--harness` 时 `harness` 为空，`bot check` 会报 error。
- 同目录可选文件：`template.md`（第一条消息的正文）、`AGENTS.md`（bot 的角色和规则）、`MEMORY.md`（从 bot 工作目录拉回来的记忆）。
- bot 运行状态（会话 id、是否在跑、上次停在哪）记进台账（B3）；每个非终态都要有出路，迟到的结果不覆盖新状态。

## 3. 命令

错误一律在 stderr 打一行「码: 原因; 下一步」，退出非 0。`--help` 是英文；列表命令有 `--json`。

| 子命令 | 状态 | 说明 |
|---|---|---|
| `yonder task add` | 已实现 | 加一条 `open` 任务；`--source` 记来源 |
| `yonder task ls` | 已实现 | 列出 `open` 任务；`--all` 含终态，`--json` |
| `yonder task done` | 已实现 | `open` → `done` |
| `yonder task cancel` | 已实现 | `open` → `cancelled` |
| `yonder remind set` | 已实现 | `--in` / `--at` / `--every`，可加 `--check`、`--for`、`--once`、`--task` |
| `yonder remind ls` | 已实现 | 列出 `active` 提醒，最近的在前；`--all`、`--json` |
| `yonder remind snooze` | 已实现 | `--by <时长>`，从现在算起 |
| `yonder remind cancel` | 已实现 | → `cancelled` |
| `yonder remind due` | 已实现 | 列出现在到点的提醒，不改任何东西 |
| `yonder ctl status` | 已实现 | yonderd 是否在跑、上一轮时间、pending 和 lost 投递数 |
| `yonder ctl service-plist` | 已实现 | 打印跑 yonderd 的 launchd plist（`KeepAlive`，固定短 PATH，不读 shell 配置）；安装由用户自己做 |
| `yonder bot new` | 已实现 | `--kind persistent` 或 `oneoff`（默认 persistent）、`--harness`；生成 bot 目录、带注释的 `bot.toml`、空的 `template.md` 和 `AGENTS.md`；已存在报 `bot_exists`，名字不合法报 `bad_name` |
| `yonder bot ls` | 已实现 | 每个 bot 一行：名字、kind、machine、harness、有哪些文件；坏定义也列出来，标 `invalid` 和第一个 error；`--json` |
| `yonder bot show` | 已实现 | 完整字段、目录、文件路径和问题；`--json` 时字段在顶层，另有 `valid`、`dir`、`files`、`problems`；没有这个 bot 报 `not_found` |
| `yonder bot check` | 已实现 | 校验一个或全部定义，逐条一行 `error: <名字>: …` / `warning: <名字>: …`；有 error 退出 1。B3 加 `--live`：harness 装了没、登录没、cwd 在不在、机器连得上吗 |
| `yonder bot sync` | 计划（B2） | 把 `AGENTS.md`（末尾拼上 `yonder manual`）推到 bot 工作目录，把 `MEMORY.md` 拉回 bot 定义目录 |
| `yonder manual` | 计划（B2） | 给 agent 看的 Yonder 用法，`bot sync` 拼进 `AGENTS.md` 用的就是它 |
| `yonder bot env set` | 计划（B3） | 把一个 env 的值写进钥匙串，从 stdin 读，只写不读 |
| `yonder bot start` | 计划（B3） | 先跑启动检查，再由 yonder 直接 `hc run --bg` 起 bot，等就绪后发第一条消息；env 从钥匙串注入 |
| `yonder bot stop` | 计划（B3） | 只中断这一轮，不关会话 |
| `yonder bot pause` | 计划（B3） | 暂停：不再给它投递 |
| `yonder bot resume` | 计划（B3） | 恢复投递 |
| `yonder bot reset` | 计划（B3） | 丢掉会话，用近况包（含 `MEMORY.md` 开头）开新会话 |
| `yonder bot rm` | 计划（B3） | 删除 bot |
| `yonder bot status` | 计划（B3） | 运行状态、会话 id、上次停在哪 |
| `yonder bot diagnostics` | 计划（B3） | 一份可以直接贴出去排查的诊断信息，不含密钥 |
| `yonder log` | 计划（B3） | 看 yonderd 和 bot 的日志 |
| `yonder handoff` | 计划（B5） | 把别处对话的进展、决定、偏好交给一个 bot：排队、只投一次、有大小上限；bot 自己整理进 `MEMORY.md` |
| `yonder handoff prompt` | 计划（B5） | 打印一段提示词，贴给别处的对话，让它写出交接内容 |
| `yonder ui snapshot` | 计划（台账页，暂停） | 给 App 台账页的一次性快照，`--json` |
| `yonder ui act` | 计划（台账页，暂停） | App 上的延后、完成、取消、关任务，直接改库并留备注 |

`yonderd` 自己的参数（已实现）：`--once` 跑一轮就退出，`--now <UTC>` 把这一轮当成指定时间（测试和验收用）。

排查 bot 的顺序（B3 起）：叫停（`bot stop`）→ 重置（`bot reset`）→ 诊断（`bot diagnostics`）。

## 4. yonderd

已实现（`crates/yonder-core/src/{daemon,firing,check_exec,hc}.rs`，`crates/yonder-cli/src/bin/yonderd.rs`）：

- **一轮**：每 `daemon.tick_secs` 秒（默认 15）一轮：把到点的提醒推进一步并写投递，开始到点的检查，把 `pending` 投递合成一条消息发给主 agent。错过多次的周期提醒（比如电脑睡过去）只触发一次，下次时间从现在往后算。
- **检查**：同时最多 2 个，在轮次旁边跑，不卡住轮次。用 `/bin/sh -c`，独立进程组，只带白名单环境变量（`PATH HOME USER SHELL LANG TZ TMPDIR`），60 秒超时整组杀掉，输出上限 1 MiB；投递里回显的输出最多 1500 字，先脱敏再截断。
- **找主 agent**：`hc ls --json --label <lead.label>` 里第一个活着的终端（`lead.label` 默认 `yonder.role=lead`），用 `hc send` 写进去。一条消息最多 256 KiB 的信封，用空行隔开。每次调用 hc 都有时限（`ls` 15 秒，`send` 120 秒）。
- **hc 的错误怎么算**：hc 退出非 0 时从 stderr 读它的错误 JSON。kind 在 `REFUSED_BEFORE_TYPING`（`not_found`、`machine_unknown`、`invalid_params`、`status_mismatch`）里的算没发出去，留着下轮再发；其他 kind、没有 kind、没有 JSON、被信号杀掉，都算可能送到了，记 `lost`。
- **本机通知**：`notify.local`（默认开）时到点也用 macOS 通知提示一下。
- **单实例**：状态目录里的 `yonderd.lock`（flock，内容是 pid）；第二个实例报 `already_running` 退出 1。`--once` 也拿这把锁。
- **停止**：SIGTERM / SIGINT 后不再开始新的触发、检查和发送，2 秒内退出，退出码 0；发到一半的投递下次启动时记 `lost`。
- **日志**：状态目录下 `logs/yonderd.log`。
- **服务**：launchd label `dev.yonder.daemon`（`service.label` 可改），plist 由 `yonder ctl service-plist` 打印。

计划：

- B3：起停 bot、监视 bot 的终端状态、自动拉起；bot 的运行状态进台账，收拢成显式状态机。
- 暂停：锁屏推送、主 agent 离线自动拉起和冷启动近况包、本机 API、IM 网关。

## 5. 目录

### 5.1 Yonder 的 home

所有东西在一个 home 下：默认 `~/.yonder`，`YONDER_HOME` 覆盖（`crates/yonder-core/src/home.rs`）。没有旧位置，也不读旧的环境变量。

- home 必须是绝对路径：`YONDER_HOME` 是相对路径（或 `HOME` 是相对路径）时，`yonder` 和 `yonderd` 都报 `bad_home` 并退出非 0，不在当前目录下建任何东西。所以 `ctl service-plist` 写进去的 home 一定是绝对路径，launchd 起的 yonderd 和 yonder 用的是同一个。
- 不存在的 home 由第一个用到它的命令建（台账或 `bot new` 都一样），权限 0700，连同建出来的上级目录；已经存在的 home 不改权限。

| 内容 | 位置 |
|---|---|
| 配置 | `<home>/config.toml`，没有就全用默认值；示例见 `crates/yonder-core/config.example.toml` |
| 台账 | `<home>/ledger.db` |
| yonderd 的锁和日志 | `<home>/yonderd.lock`、`<home>/logs/yonderd.log` |
| bot 定义 | `<home>/bots/<名字>/` |
| hc | 配置的 `hc.bin`，没有就从 PATH 找；`YONDER_HC` 优先 |

`yonder ctl service-plist` 生成的 plist 把 `HOME`，以及设了的 `YONDER_HOME`、`YONDER_HC` 原样带给 yonderd，所以 yonderd 和打印它的 `yonder` 用同一个 home。

```text
~/.yonder/
├── config.toml
├── ledger.db
├── yonderd.lock
├── logs/
└── bots/
    └── trainer/
        ├── bot.toml
        ├── template.md
        ├── AGENTS.md
        └── MEMORY.md      # bot sync 从 bot 工作目录拉回，留 git 历史
```

bot 的工作目录是 `bot.toml` 里的 `cwd`（可以在别的机器上）：Yonder 只往里推 `AGENTS.md`，只从里面拉 `MEMORY.md`，不写别的。

### 5.2 仓库

```text
Cargo.toml          # 根 workspace，members = crates/*，不含 app/
crates/
  herdc-*           # 手机 app 用的连接、终端、引擎、测试工具
  yonder-model      # 共享类型和信封，只依赖 serde、serde_json
  yonder-core       # 存储、配置、提醒、检查、投递、hc 调用
  yonder-cli        # 两个二进制：yonder、yonderd
app/                # NSH app（独立 workspace），Android 和 macOS
docs/spec.md        # 本文
docs/DESIGN.md      # 手机 app 的界面和交互
scripts/check.sh    # clippy 和全部测试
```

`app/` 和 `crates/herdc-*` 只能依赖 `yonder-model`，不能依赖 `yonder-core`、`yonder-cli`。

## 6. App

同一个 NSH 0.4 app（`app/`，纯 Rust，`nsh.json` 的 `root` 是 `App`），出两个包：

| | Android | macOS |
|---|---|---|
| 打包 | `app/apk.sh`（不带 `--debug` 用正式签名，签名不可用就停；`--debug` 用调试签名） | `app/macos.sh`：`nsh package --target macos`，ad hoc 签名，最后一行打印 `.app` 的绝对路径；不做正式签名和公证 |
| 密钥 | Android Keystore；Android 12 以前用 app 自己 `no_backup` 目录里的密钥 | 登录钥匙串，service `dev.yonder.app.vault`，经 `/usr/bin/security`（密钥走 stdin，钥匙串按 `security login-keychain` 给的路径显式指定，路径里有控制字符、引号、反斜杠就报错） |
| 时区 | `localtime_r` | `localtime_r` |
| 按键栏 | 默认显示 | 默认隐藏，设置 → 按键栏里可以打开 |

共同规则：

- 加密失败就报错，任何构建、任何平台都不退回明文。headless 测试用 `test-vault` feature：密钥是测试进程内存里的随机数，存的照样是密文；release 构建带这个 feature 编译不过。
- 打包时 `--remap-path-prefix` 去掉家目录路径。
- Android 不加 Java（打包出来的 APK 不能带自定义 Java）。
- 桌面交互（已实现）：菜单栏「设置 ⌘,」（连着电脑时可用）、「关闭 ⌘W」、「退出 ⌘Q」，编辑和窗口菜单；Agents 行右键弹出「置顶 / 停止」，停止仍要确认。
- 桌面没做：行尾悬停图标、⌘K、⌘1..9、⌘+ / ⌘−、关窗后留在 Dock。NSH 0.4 的 macOS 后端没有托盘、系统通知、登录自启、防睡眠，默认 runner 也不暴露关窗不退出。
- 桌面连电脑和手机一样走 SSH 到 herdr-core，不做本机免 SSH 直连。

计划：

- B4：bot 页：列表、状态、起停。
- 暂停：台账页（手机单列、桌面分栏）、桌面管理页（IM 接入、凭据只写、后台状态）。

## 7. 记忆

- 每个 bot 的记忆是它工作目录里的 `MEMORY.md`，由 bot 自己写；Yonder 不替它写。`bot sync` 把它拉回 `~/.yonder/bots/<名字>/`，留 git 历史（B2）。
- 一次性 bot（`kind = "oneoff"`）不写记忆；它在回报里提「经验候选」，由主 agent 决定写到哪。
- 正在做的事进台账，不进记忆。
- 冷启动和 `bot reset` 的近况包带上 `MEMORY.md` 的开头（B3）。`yonder handoff` 交来的内容由 bot 自己整理进 `MEMORY.md`（B5）。
- 不做：记忆体检、每天自动整理、记忆搜索（没有 `yonder memory search`）。
- Nowledge 只作为可选身份：`bot.toml` 有 `memory_id` 时，起 bot 时带上 `NMEM_AGENT_ID=<memory_id>`，别的不接。Yonder 不依赖 Nowledge，没有它一切照常。
- `bot sync` 不生成 hc 模板：B3 由 yonder 直接 `hc run` 起 bot，模板不再需要；B3 完成后删掉 hc 模板目录里原有的软链。

## 8. 里程碑

| 包 | 内容 | 状态 |
|---|---|---|
| M1a | `yonder task`、`yonder remind`（含带检查的提醒）、`ledger.db` | 已完成 |
| M1b | `yonderd`：到点投递、检查、投递状态机、`ctl status`、`ctl service-plist` | 已完成 |
| D1 | 桌面版：`Yonder.app`、钥匙串存密钥、本地时区、桌面默认值和菜单、右键菜单 | 已完成 |
| S0 | 本文和仓库根 `AGENTS.md` | 已完成 |
| B1 | bot 定义格式、`bot new/ls/show/check`、`~/.yonder` 目录 | 已完成 |
| B2 | `bot sync`、`yonder manual` | 计划 |
| B3 | bot 生命周期、`yonder log`、env 从钥匙串注入、启动检查 | 计划 |
| B5 | `yonder handoff` 和提示词 | 计划 |
| B4 | App 里的 bot 页 | 计划 |

之后暂停：台账页、桌面管理页、IM。

## 9. 已实测与待验证

### 9.1 已实测

- 任务和提醒的增删改查、`--json`、错误一行：`crates/yonder-cli/tests/cli.rs` 的 `task_list`、`reminder_due_snooze_cancel`、`bad_input_is_one_line`。
- 检查失败不建、检查看不到密钥类环境变量、基线先脱敏再截断：`failing_check_is_not_stored`、`check_does_not_see_secrets`、`baseline_is_masked_before_it_is_cut`。
- 一次性提醒只投递一次、周期提醒往后挪、检查变化和第三次失败各投递一次：`yonderd_fires_a_one_off_once`、`yonderd_moves_repeats_on`、`yonderd_tells_a_changed_check`、`yonderd_tells_the_third_failure_once`。
- 找不到主 agent 时投递留着、之后只补发一次：`yonderd_keeps_what_it_cannot_deliver`，以及 M1b 验收第 6 条。
- hc 打字后报错的不重发、记 `lost`；打字前拒绝的会重发：`hc_errors_after_typing_are_never_sent_again`、`hc_refusals_are_sent_again`。
- `REFUSED_BEFORE_TYPING` 里的 `not_found`、`machine_unknown`、`status_mismatch`：在隔离的 hc core（0.1.22）上手工复现过，`status_mismatch` 的提示写着没有打字。
- 单实例、SIGTERM 后 2 秒内退出（包括 `--once` 正在等检查时）：`yonderd_runs_alone_and_stops_on_sigterm`、`yonderd_stops_in_time_with_a_check_running`、`yonderd_once_stops_in_time_with_a_check_running`。
- plist 合法、含 `KeepAlive` 和 `dev.yonder.daemon`、不读 shell 配置、带上 `YONDER_HOME`：`service_plist_is_valid`，以及 M1b 验收第 8 条（`plutil -lint`）。
- 台账、锁、日志都在 `YONDER_HOME` 下，没设时在 `~/.yonder`，不再碰旧位置：`everything_is_in_the_yonder_home`，`home::tests::finds_the_home`。
- 相对的 home 被拒（`bad_home`）、新 home 是 0700、已有的不改：`a_relative_home_is_refused`、`bot_new_in_a_new_home_keeps_it_the_owners`、`home::tests::a_new_home_is_the_owners_only`、`bot::tests::a_new_bot_in_a_new_home_keeps_it_the_owners`。
- 符号链接不跟随、新文件不覆盖：`bot::tests::links_are_never_followed`、`new_files_never_replace_what_is_there`。
- bot 定义的生成、不覆盖、列出（坏定义不挡别的）、校验（行号、名字和目录不一致、persistent 缺 cwd、未知字段只警告）、`show --json`：`bots_are_made_listed_and_checked`，`crates/yonder-core/src/bot.rs` 的单元测试，以及 B1 验收第 2–6 条。
- yonderd 用真实 hc 投递：M1b 验收时手工冒烟过一次。
- 钥匙串存取、不产出明文、test-vault 下也加密、钥匙串路径校验：`app/src/vault.rs` 的 `keychain_round_trip`、`never_seals_to_plaintext`、`test_vault_seals_too`、`keychain_path_is_one_line_of_its_own`（测试只用 `.test` 结尾的 service，跑完删掉）。
- 时区跟随本机：`app/src/chat.rs` 的 `utc_offset_follows_local_zone`（上海、纽约冬夏、UTC 各跑一次）。
- `Yonder.app` 能打包、签名能过 `codesign --verify --deep --strict`、二进制里没有 `/User` 开头的家目录路径：D1 验收第 2–3 条。D1 验收第 4 条只验证到进程起来（`pgrep` 找到）、让它退出后进程没了；没有截图。
- Android 没坏：D1 验收第 6 条（`app/apk.sh --debug` 退出 0）和 `app/tests/app.rs` 的 headless 测试。

### 9.2 待验证

- 材料写的是 `yonder remind add`，代码里是 `yonder remind set`；本文按代码写。
- 睡眠唤醒：两轮间隔超过 5 个 `tick_secs`（默认 15 秒，即 75 秒）时 yonderd 记一行 `woke after Ns`；错过多次的周期提醒只触发一次，靠的是「下次时间从现在往后算」（`yonderd_moves_repeats_on`）。真实睡眠唤醒没有手工验证过。（S0 版本误写成代码没有睡眠检测，已改正。）
- 窗口出现过一次（D1 时用 CGWindowList 看到过一个 1200×832 的 Yonder 窗口），但不在验收里，也没有截图；桌面版的画面待人看。
- 检查的环境白名单：设计稿含 `LOGNAME`、`LC_*`，代码只有 `PATH HOME USER SHELL LANG TZ TMPDIR`。
- hc 的 `blocked`、`finished` 等错误是不是一定发生在打字之前：没有依据，现在按可能送到处理（记 `lost`）；主 agent 卡在权限对话框时那批提醒会丢。
- launchd 实际安装和崩溃后拉起：只验证了 plist 合法，没装过。
- Linux CI：`rusqlite` 要系统的 `libsqlite3`，CI 是否需要补 `libsqlite3-dev` 没跑过。
- release 体积预算（两个二进制合计小于 15 MiB）：没跑过 release 构建，没量。
- 桌面菜单（⌘, ⌘W ⌘Q）和 Agents 右键菜单：没有实际点过，headless 测试里 NSH 的菜单 API 什么都不做。
- 真实钥匙串条目 `dev.yonder.app.vault` 的创建和桌面版真连一台电脑：没做（要输真实的 SSH 密码）。
- 打包后的路径残留：NSH 的 `asset!()` 把每个资源的源文件绝对路径写进二进制，`--remap-path-prefix` 管不到。`app/macos.sh` 现在把二进制里的家目录等长改写后重新签名，这个做法是临时的，待换成正经修法；Android 的 `libnsh.so` 里也有同样的残留，`app/apk.sh` 还没处理。
- B3 用 `hc run --bg` 起 bot 后怎么判断「就绪」再发第一条：没实测。
- bot 的 env 注入方式（hc 起终端时怎么只给这个终端带上变量而不进日志）：没实测。
