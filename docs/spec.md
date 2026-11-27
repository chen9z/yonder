# Yonder 规格

本文是 Yonder 的唯一依据：范围、命令、架构以这里为准，改动时和代码同一次提交更新。
写法：已经在代码里的写「已实现」，还没做的写「计划（包名）」；材料和代码不一致时按代码写，并记进最后一节。
`docs/DESIGN.md` 记的是手机 app 的界面和交互细节，和本文不冲突时照它；冲突时以本文为准。

示例名：主 agent 叫 nova，常驻 bot 叫 trainer、ops，机器叫 gpu-box、homelab，路径用 `/tmp/example`。

## 1. 范围

Yonder 是跑在自己电脑上的个人 agent 底座和客户端。agent 本身跑在 herdr-core（`hc`）的终端里，人随时能在桌面 Herdr、手机或桌面 Yonder 里看、插话、接管。Yonder 负责 agent 之外的部分：

| 部件 | 是什么 | 状态 |
|---|---|---|
| `yonder` | 命令行：台账（任务、提醒）、`yonderd` 的状态、bot 的定义、同步、生命周期、日志和交接 | 已实现一部分 |
| `yonderd` | 常驻后台：提醒到点、跑带检查的提醒、把结果投递给主 agent 的终端 | 已实现 |
| 存储 | SQLite `ledger.db`（WAL） | 已实现 |
| App | 同一个 NSH app：Android 版和 macOS 版（`Yonder.app`） | 已实现（连电脑、看 agents 和终端） |
| 自定义 bot | 用户自己定义常驻或一次性的 bot，由 Yonder 起停、同步、交接 | 定义和查看（B1）、同步（B2）、起停和状态（B3a）、暂停恢复重置删除诊断和日志（B3b）、交接（B5）已实现；App 的 bot 页计划（B4） |

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

- `env` 的名字限 `^[A-Z_][A-Z0-9_]*$`，拒绝系统变量和 Yonder 自己的：`PATH HOME SHELL USER LOGNAME PWD TMPDIR TERM LANG TZ`，以及 `YONDER_`、`LC_`、`DYLD_`、`LD_` 开头的。同名写两次是警告。值只存在钥匙串里，起 bot 时注入（见 2.7）；不进 `bot.toml`、不进提示词、不进日志。
- 未知字段：`bot check` 报警告，不失败（以后加字段时旧版本还能读）。
- 校验（`crates/yonder-core/src/bot.rs`）：error 是 toml 语法错（带行号）、没有 `bot.toml`、必填字段缺了或为空、字段类型不对、名字格式不对、`name` 和目录名不一致、`kind` 不是 `persistent` / `oneoff`、persistent 没有 `cwd`、`env` 名字不合法；有 error 的定义算 `invalid`，不能用。warning 是未知字段、`env` 重复、`description` 还是 `bot new` 写的占位（`TODO: ` 开头）。
- 读定义永远不会整体失败：一个坏定义只让它自己 `invalid`，不挡别的。`bots/` 下不是目录的、`.` 开头的都不算 bot。
- bot 目录和 `bot.toml` 不能是符号链接：是的话（或 `bot.toml` 不是普通文件）这个 bot 标 `invalid` 并写明原因，不读链接指向的东西，`bot ls` 照样列出它、不挡别的。`bots/` 本身是符号链接时 `bot ls/show/check/new` 都报 `symlink`。`bot new` 的三个文件都是排他创建，那里已经有东西（文件或链接）就报错，不覆盖。
- `cwd` 必须是绝对路径或以 `~/` 开头，否则是 error（`~` 本身、`~user/...`、`work/x` 都不行）。
- `bot new` 只生成文件，之后 Yonder 不改 bot 目录里的任何东西，只有 `bot sync` 会替换其中的 `MEMORY.md`（见 2.6）。生成的 `bot.toml` 字段都在顶层，没有表，所以在文件末尾追加一行（如 `cwd = ...`）就是加一个字段；`cwd` 等可选字段以注释给出。不带 `--harness` 时 `harness` 为空，`bot check` 会报 error。
- 同目录可选文件：`template.md`（第一条消息的正文）、`AGENTS.md`（bot 的角色和规则）、`MEMORY.md`（从 bot 工作目录拉回来的记忆）。
- bot 运行状态（会话 id、是否在跑、上次停在哪）记进台账（见 2.7）；每个非终态都要有出路，迟到的结果不覆盖新状态。

### 2.6 bot 同步（已实现，B2）

`yonder bot sync [<名字>...] [--force] [--dry-run]`，代码在 `crates/yonder-core/src/sync.rs`。不给名字就同步全部 bot，每个 bot 一行，一个 bot 出问题不挡别的：

- 只同步合法的 persistent bot。oneoff 输出 `<bot>: skipped, oneoff`；不合法的（B1 的校验）和找不到的输出 `error: <bot>: <第一个原因>`。`bots/` 本身是符号链接时整条命令报 `symlink`。
- 推送：目标是 `<cwd>/AGENTS.md`，内容是 `bots/<名字>/AGENTS.md` 原文（没有结尾换行就补一个）、一个空行、标记行 `<!-- yonder manual: generated by yonder bot sync; edits below are overwritten -->`、一个空行、`yonder manual` 的输出（作为文件末尾）。`bots/<名字>/AGENTS.md` 不存在就不推，记 `skipped`。
- 防覆盖：台账的 `bot_pushes` 表（迁移第 3 步）按 bot 和目标（`<machine>:<cwd>`，照 `bot.toml` 原样）记上次推出去的内容的 SHA-256，只存哈希。目标不存在、或等于上次推的，就写；等于这次要推的，记 `unchanged`（没有记录时补记）；都不是（包括第一次同步前就有的文件），不写，记 `drifted`。`--force` 时照写。改了 `cwd` 就是新目标，旧记录不用于它。
- 拉回：读 `<cwd>/MEMORY.md`，和 `bots/<名字>/MEMORY.md` 不同就覆盖本地：同目录下先写 `.MEMORY.md.<pid>.yonder-new`，再 rename；失败时删掉这个自己建的临时文件。目标没有就不动本地，记 `missing`。
- 输出：`<bot>: AGENTS.md <pushed|unchanged|drifted|skipped|failed>; MEMORY.md <pulled|unchanged|missing|failed>`，失败时后面加 ` — <原因>`（两边都失败用 `; ` 隔开）。有任何 `drifted`、`failed`、不合法或找不到的 bot，最后 stderr 一行 `sync_incomplete: <bot>: <怎么了>; …; <下一步>`，退出 1；否则退出 0。
- `--dry-run`：照样读、照样输出将会怎样，什么都不写。台账只用只读方式打开（没有 `ledger.db` 就当没有记录，也不建它；旧版本的台账不迁移，当作没有记录）。
- 不删除任何东西，不往 `cwd` 写 `AGENTS.md` 以外的文件。
- 本机（`machine = "local"`）：直接读写，`cwd` 的 `~/` 按 `$HOME` 展开（没有 `HOME` 就记 `failed`）。不跟随符号链接（`O_NOFOLLOW`），目标是链接或不是普通文件就记 `failed`；`bots/<名字>/AGENTS.md`、`MEMORY.md` 是链接也一样。`cwd` 不存在时记 `failed`，不替它建目录。
- 别的机器：只经 `crates/yonder-core/src/hc.rs` 的 `HcFiles`：`hc file read --json --on <machine> <路径>` 取 JSON 的 `text`，`hc file write --on <machine> <路径>` 内容走 stdin，每次调用限时 30 秒。`cwd` 的 `~/` 去掉，传相对路径（hc 带 `--on` 时从那台机器的家目录算起）；绝对路径原样传；以 `-` 开头的相对路径前面加 `./`。hc 报 `not_found` 算文件不存在，别的错误记 `failed`，原因只带 hc 的 `kind` 和 `message`（不带 `hint`，hint 里有机器列表）。`hc file write` 会建缺的目录，所以远端的 `cwd` 不存在时照样推得上去，这一点和本机不同。
- `hc file read` 一次最多给 8 MB，超过的部分截掉却仍退出 0（`last` 小于 `lines`）；Yonder 把这种情况当 `failed`，不拉回半个文件。不是 UTF-8 的文件 hc 报 `not_text`，记 `failed`。

### 2.7 bot 运行（已实现，B3a、B3b）

代码在 `crates/yonder-core/src/botrun.rs`（流程）、`store.rs`（台账）、`hc.rs` 的 `HcBots`（hc 调用）、`keychain.rs`（钥匙串）。Yonder 自己经 hc 起 bot，不生成、不读 hc 模板。

**运行记录**：台账 `bot_runs` 表（迁移第 4 步），每个 bot 一行当前记录：`bot`、`address`（`t7`，远端 `machine/t7`）、`state`、`session`（这次起的时候续的会话）、`started_at`、`updated_at`、`reason`。会话 id 另存 `bot_sessions` 表，run 结束不丢。

```text
(没有记录 / failed / gone) --bot start、hc run 成功--> starting
starting --hc get 说 idle 或 done--> ready
starting --终端结束 / 已知对话框或 blocked / 超时--> failed（reason：exited: code N、needs_attention: …、start_timeout）
ready --终端结束或不在了（status、start、stop 看到时）--> gone
starting、ready --bot pause / bot reset 杀掉终端--> gone（reason：paused、reset）（B3b）
```

- 每次转换都是 `UPDATE bot_runs … WHERE bot = ? AND address = ? AND state = <来源状态>`（`Store::move_run`），只允许上面这几步；地址不对（更早那次 run 迟到的结果）或已经被别人推进过，就什么都不改。新一次 start 只能覆盖 `failed`、`gone`（`Store::begin_run`），同一行复用。
- 不会卡在 `starting`：`start` 自己等到出结论；它中途被打断时，下次 `status` / `start` / `stop` 看到 `starting` 超过启动时限再加 10 秒就记 `failed`（`start_timeout`），终端已经结束或不在了也记 `failed`。hc 调不通时不改台账。
- 只有 `bot pause` 和 `bot reset` 杀终端（`hc kill`，不 `hc rm`），别的情况终端都不杀：失败的留给人去 `hc attach` 看。

**`yonder bot start <名字> [--task <文字>] [--cwd <目录>]`**：

1. 定义要合法。暂停着的报 `bot_paused`（`<名字> is paused; yonder bot resume <名字> starts it`），退出 1，什么 hc 调用都不做（B3b）。oneoff 必须给 `--cwd` 和 `--task`，persistent 不接受 `--cwd`（用 `bot.toml` 的 `cwd`），否则 `bad_input`。远端 bot 有 `env` 就报 `env_remote_unsupported`，不起。
2. 启动检查（同 `bot check --live`）有 error 就报 `start_check`，不起。
3. 先按 hc 的实际情况推进旧记录；`hc ls --json --label yonder.bot=<名字>` 有活着的终端，或记录还是 `starting` / `ready`，报 `already_running`，不起。hc 的 `--name` 在活着的终端里唯一，两个 start 同时跑时第二个的 `hc run` 会失败。
4. `hc run --bg --json --name <名字> --label yonder.bot=<名字> --outlive-parent --keep [--on <machine>] --cwd <cwd> -- <argv>`，记 `starting`。cwd：本机是绝对路径（`~/` 按 `$HOME` 展开，oneoff 的相对 `--cwd` 从当前目录算）；远端去掉 `~/` 传相对路径（hc 带 `--on` 时从那台机器的家目录算起，实测过；hc 不展开 `~`，目录不存在时 `hc run` 直接报 `spawn_failed`），绝对路径原样传，其他相对写法报 `bad_input`。
5. argv：
   - harness 一律经目标机器的登录 shell 起（B3a.1）：`/bin/sh -c 'exec "${SHELL:-/bin/sh}" -lic '\''exec "$@"'\'' yonder-bot "$@"' yonder-bot <harness> <会话参数> <model 参数> <args>`（`botrun::LOGIN_EXEC` / `login_exec`，两条路径共用）。原因：hc 终端和 `hc exec` 的环境来自 hc core，PATH 只有系统目录，找不到 nvm、`~/.local/bin` 这类用户装的 harness（B3a 时真实冒烟起 `pi` 报 `env: pi: No such file or directory`，退出 127）；登录交互 shell 会读用户的 profile 和 rc。`$SHELL` 是终端那边的（远端就是远端的），没有就 `/bin/sh`；`-l -i` 都带。脚本对所有 bot 都一样，harness 和每个参数都是独立的 argv，不拼进脚本。`$SHELL` 是 fish 这类不接受 `-c '...' 名字 参数` 写法的 shell 时不能用。登录 shell 的 rc 里有什么（启动 tmux、打印东西、改环境）都会跟着发生。
   - `env` 为空：`env [NMEM_AGENT_ID=<memory_id>] <上面的包装> <harness> …`，本机远端一样。
   - `env` 非空（只限本机）：`env YONDER_HOME=<绝对路径> [YONDER_KEYCHAIN_SERVICE=<值>，只在设了时] <yonder 的绝对路径> bot exec <名字>`。hc 终端的环境来自 hc core，不是调用者（实测：终端里没有 `YONDER_HOME`），所以 home 要显式带上。
   - 会话参数：记了 session 时，`codex` → `resume <id>`，`pi` → `--session <id>`，`claude` → `--resume <id>`（按 harness 的文件名认）；其他 harness 在 stderr 打一行 `warning:`，不加。
   - model 参数（B3a.1）：`bot.toml` 的 `model` 非空时，`codex` → `-m <model>`，`pi` → `--model <model>`，`claude` → `--model <model>`，放在会话参数之后、`args` 之前；其他 harness 打一行 `warning:`，不传。
6. 等就绪：每秒一次 `hc get --json <地址>`，加 `hc read --json <地址>` 取屏幕文字；时限默认 90 秒，`config.toml` 的 `[bot] start_timeout_secs` 可改（1–3600）。按顺序判断：
   - 终端 `finished` → `failed`（reason 带 exit code 或 signal），报 `start_failed`；`hc get` 说没有这个终端也算结束。
   - 屏幕文字（或 `hc get` 的 `screen` 字段）里有已知对话框文字 → 立即 `failed`，报 `needs_attention: <命中的文字>; attach with hc attach <地址>`，不按任何键。列表在 `botrun::DIALOGS`：`Hooks need review`、`open in another app`。`status.state` 是 `blocked`（hc 的权限、提问、登录）也一样处理。对话框优先于 idle。
   - `status.state` 是 `idle` 或 `done` → `ready`。
   - 超时 → `failed`，报 `start_timeout`。
7. 就绪后发第一条（`hc send`）：`template.md` 去掉开头 `---` … `---` frontmatter 后的正文，有 `--task` 时接一个空行再接任务；都为空就不发。发失败报 `send_failed`，记录仍是 `ready`。
8. 成功输出一行 `<名字>: ready at <地址>`，退出 0。

**env 注入**：`yonder bot env set <名字> <变量>` 从 stdin 读值（去掉末尾一个换行；stdin 是终端时提示在 stderr、不回显），写进登录钥匙串：service 默认 `dev.yonder.bots`（`YONDER_KEYCHAIN_SERVICE` 可改，测试和验收用 `dev.yonder.bots.test`），account `<名字>/<变量>`，已存在就覆盖（`-U`）。

- 变量必须在 bot 的 `env` 里（否则 `env_not_declared`），名字规则同 2.5（保留名报 `bad_env`）。
- 写法：`security -i` 的 stdin 上一行 `add-generic-password -U -s <service> -a <account> -X <值的 hex> "<登录钥匙串路径>"`。值用 hex（`-X`）编码，所以不会断行、不会跳出引号；值不进任何进程的 argv、不进日志、不回显，`security` 失败时的输出也不转述（可能回显那一行）。service、account、钥匙串路径不能含空白、引号、反斜杠、控制字符。
- 值只能是可打印 ASCII（空格到 `~`），不能为空，否则 `bad_value`：实测 `find-generic-password -w` 在值全是可打印 ASCII 时原样打印，否则打印 hex，限定之后读回来没有歧义。
- 只写不读：没有打印值的命令。
- `yonder bot exec <名字>`（内部，`--help` 里标明）：读定义，按 `env` 逐个从钥匙串取值（`find-generic-password -w`，值在 stdout，不在 argv），缺一个就报 `env_missing: <变量>` 退出 1、不起 harness；加上 `NMEM_AGENT_ID=<memory_id>`（有的话），persistent 切到 `cwd`（oneoff 留在 hc 给的目录），然后经同一个登录 shell 包装 `exec` harness（argv = 包装 + harness + 会话参数 + model 参数 + args，会话从台账只读取）。env 值和 `NMEM_AGENT_ID` 在包装之前就设进环境。值不打印、不写日志。

**`yonder bot status [<名字>] [--json]`**：每个 bot 一行：名字、state（没有记录是 `stopped`）、地址、会话、上次原因；顺手按 hc 的实际情况推进记录。hc 调不通时这一行显示 `unknown` 和台账里记的状态，不改台账。`--json` 每个 bot 一个对象：`name`、`state`、`recorded_state`、`address`、`session`、`reason`、`started_at`、`updated_at`、`hc_error`。

**`yonder bot stop <名字>`**：只叫停当前这一轮：`hc interrupt <地址>`（按一次 ctrl+c），不关终端、不改会话，输出 hc 看到的结果（实测多半是 `unknown`）。没有 `ready` 的记录、或终端已经结束，报 `not_running`（顺便记 `gone`）。

**暂停**（B3b）：是 owner 设的状态，和 run 分开存：台账 `bot_pauses` 表（迁移第 5 步），一个 bot 一行 `bot`、`paused_at`。只有 `bot pause` 设它，只有 `bot resume`、`bot reset`、`bot rm` 清它；run 的状态变化、`status` 的观察、重启都不碰它。暂停着时 `bot status` 的 state 显示 `paused`（run 是 `starting`、`ready` 时仍显示 run 的状态，正常不会出现），`--json` 多 `paused`、`paused_at`。

**`yonder bot pause <名字>`**（B3b）：已经暂停：输出 `<名字>: already paused`，退出 0，不调 hc。否则把它的终端都结束：当前 run 不是终态时它的地址，加上 `hc ls --label yonder.bot=<名字>` 里活着的（最多 8 个），每个先 `hc interrupt`，还活着就 `hc kill`（终端留着，可以 `hc read`，不 `hc rm`）；当前 run 记 `gone`、reason `paused`；会话 id 保留；记成已暂停。没在跑也能暂停，只记标记。hc 出错时报 `hc`，不记暂停。

**`yonder bot resume <名字>`**（B3b）：没暂停报 `not_paused`，退出 1。否则清掉暂停标记，然后和 `bot start` 一样起（带记着的会话）。起失败时暂停标记仍然清掉（run 是 `failed`，能再 `start`），错误信息末尾写明 `no longer paused`。

**`yonder bot reset <名字> --yes`**（B3b）：丢掉会话重新开始。没有 `--yes` 报 `confirm_needed`，退出 1，什么都不改、不调 hc。oneoff 报 `bad_input`（没有会话可丢）。有 `--yes`：像 `pause` 那样结束终端（reason `reset`），清掉会话 id 和暂停标记，再像 `bot start` 那样起（不带会话参数），第一条消息是 `template.md` 正文 + 空行 + 近况包：

```text
[yonder kind=recap bot=<名字>]
Your session was reset; you start fresh. The start of your MEMORY.md:
<payload untrusted="true" source="memory">
<bots/<名字>/MEMORY.md 的前 4096 字节，按 UTF-8 边界截断（截在半个字符上就丢掉那半个），去掉末尾空白；没有这个文件就写 (none)>
</payload>
```

头和正文照 2.4 转义（`botrun::recap`）。`MEMORY.md` 用 `bot sync` 上次拉回的那份（符号链接不跟随），输出里提示先 `bot sync` 能拿到最新的。起失败时会话已经清了，错误信息里写明。

**`yonder bot rm <名字> --yes`**（B3b）：先按 hc 推进记录；run 还是 `starting` / `ready`，或 hc 上有活着的带它 label 的终端，报 `bot_running`（提示先 `pause`），退出 1（没有 `--yes` 时也先查这个）。没有 `--yes` 报 `confirm_needed`。有 `--yes`：

- 把 `bots/<名字>/` 整个改名到 `bots/.trash/<名字>-<UTC 时间，如 20261127T012841Z>/`（重名时末尾加 `-2`、`-3`…），不删，手工挪回来就能找回定义；`.trash` 以 `.` 开头，`bot ls` 本来就跳过；没有就建（0700），是符号链接或不是目录时报 `home`、不挪。
- 台账删掉它的 run、会话、暂停标记、同步记录（`Store::forget_bot`，一个事务）。
- 删掉钥匙串里它 `env` 每个变量的条目（`delete-generic-password`，不存在的跳过）；定义读不出来时不知道有哪些变量，不删，输出写明。
- 不碰它的工作目录，不动 hc 终端。输出写明挪到了哪。

**`yonder bot diagnostics <名字> [--json]`**（B3b）：一段可以直接贴出去的纯文本，每行 `键 值`：yonder 版本、系统（`uname -sr`）、hc 版本（`hc --version` 的第一行，拿不到写 `unknown`）、bot 的 name/kind/machine/cwd/harness/model/memory_id、是否暂停和时间、run 的 state/address/started_at/reason（台账里记的，不去问 hc）、会话 id、`env` 每个变量 `set` / `not set`（`find-generic-password` 不带 `-w`，输出丢掉，只看退出码；钥匙串出错写 `unknown`）、`MEMORY.md` 的字节数、有哪些定义文件、上次同步（`bot_pushes` 里最近的推送时间）、`bot check` 的 problems。不含任何 env 值和文件内容。台账不存在时不建。`--json` 是同样内容的一个对象。

**bot 日志**（B3b）：`<home>/logs/bots.log`，bot 的每一步一行：`<UTC> <名字> <事件> [address=…] [reason=…]`，事件是 `start`（`hc run` 成功后）、`ready`、`failed`（reason 是错误码）、`stop`、`pause`、`resume`、`reset`、`rm`；`pause`、`reset` 的 address 是结束的终端（逗号隔开，没有就不写）。值里的空白换成 `_`，最多 200 字。不写 env 值、不写消息正文。文件 0600，超过 1 MiB 时改名成 `bots.log.1`。写不进去不让命令失败（`botrun::log_event`）。

**`yonder log [--bot <名字>] [--lines <N>]`**（B3b）：打印 `logs/yonderd.log` 和 `logs/bots.log` 末尾各 N 行（默认 50），每段一行标题 `== yonderd.log (last N lines) ==`；文件不存在写 `(none)`。`--bot X`：两个文件都只留按整词提到 X 的行（前后不是字母、数字、`-`、`_`，所以 `trainer` 不匹配 `trainer-2`），再附上它活着的终端（`hc ls --label`）屏幕最后 N 行（`hc read`），段标题写地址；没有活着的终端就不附，hc 出错写一行 `(hc: …)`。

**`yonder bot check --live [<名字>]`**：B1 的校验之后再查：机器连得上、`cwd` 存在（oneoff 的 cwd 每次 start 才给，这里不查）、harness 是终端那边的登录 shell 找得到的程序。本机远端都是一次 `hc exec [--on <machine>] -- /bin/sh -c 'cd -- "$1" || exit 10; "${SHELL:-/bin/sh}" -lic '\''command -v "$1" > /dev/null 2>&1'\'' yonder-bot "$2" || exit 11' yonder-bot <cwd> <harness>`（B3a.1 起；路径和 harness 作为参数传，不拼进脚本），环境和 `bot start` 起 harness 时一样；本机的 cwd 仍直接查（传给 hc exec 的是 `.`）。按退出码和 hc 的错误把原因归到机器、cwd 或 harness。B3a 时本机用的是 `yonder` 进程自己的 PATH，查得到而终端里找不到，这个漏洞 B3a.1 修了。harness 登录了没有不查。只有这类问题时报 `start_check`，定义本身有错仍报 `bot_invalid`。

### 2.8 交接（已实现，B5）

别处的对话（另一个 Claude Code、Codex、网页聊天）把进展、决定、偏好交给一个常驻 bot，由 bot 自己整理进 `MEMORY.md`。交接不来自聊天，没人等回复。代码在 `crates/yonder-core/src/handoff.rs`。

**台账**：`handoffs` 表（迁移第 6 步）：`id`（`H-0001` 起）、`bot`、`body`、`via`（`command` | `paste`）、`state`、`created_at`、`delivered_at`、`address`（投到的终端）、`sender`（正在发它的进程的 pid）、`reason`（为什么还在等，或为什么 lost）。不提供删除和撤回。

```text
waiting --认领（记下 pid）--> sending --hc send 成功--> delivered
                               |--hc 在打字前拒绝（kind 在 REFUSED_BEFORE_TYPING 里）--> waiting
                               \--其他任何失败、超时（可能送到了）--> lost
sending，发它的进程已经不在了（发到一半退出）--> lost
sending，认领超过 10 分钟仍没结果（pid 可能已被别的进程复用）--> lost
waiting，bot 被 bot rm 删了 --> lost（reason 写明，不会投给以后同名的 bot）
```

- 规则照 2.3 的投递：最多送一次，可能送到过的不重发。每次转换都是 `UPDATE handoffs … WHERE id = ? AND state = <来源>`，所以 `yonder handoff` 和 yonderd 同时投也只有一个认领得到。`delivered`、`lost` 是终态，`bot reset` 之后也不重投。
- 遗留的 `sending`：yonderd 每一轮查一遍，`sender` 那个 pid 已经不在了（`kill(pid, 0)`）就记 `lost`（2.3 是重启时；这里 `yonder handoff` 也会发，所以按发送进程在不在算）。另外认领时记下 `claimed_at`（迁移第 7 步，`store.rs` 里内联的 `ALTER TABLE`），认领超过 10 分钟（`handoff::STUCK_SECS`）仍是 `sending` 的不管 pid 在不在都记 `lost`，原因 `still sending 10 minutes after it was claimed`（B4）；第 7 步之前认领、没有 `claimed_at` 的也算超时。

**`yonder handoff <名字> [--via command|paste]`**：正文从 stdin 读，最多读 200 KiB + 1 字节。

- 去掉首尾空白后为空：`handoff_empty`；超过 200 KiB（204800 字节）：`handoff_too_large`（提示把细节写成文件、交接里只给路径）；不是 UTF-8：`bad_input`；都退出 1，不存。bot 不存在或定义不合法照 B1 报错；oneoff bot 报 `handoff_oneoff`。
- 存的正文去掉开头的空行和末尾空白，像密钥的串按 `redact` 换成 `[REDACTED]`（和提醒一样）。
- 存成 `waiting`，马上对这个 bot 试投一次（见下）。输出 `H-0001: delivered to <名字> at <地址>`、`H-0001: waiting — <原因>`（`paused`、`not running`、`starting`、`working`、`needs attention: …`、`hc refused it before typing …`、`hc: …`），或 `H-0001: lost — <原因>; it may have arrived, so it is not sent again`；都退出 0（已经收下了，再交一遍会重复）。顺带投出去的更早的交接也各打一行。

**什么时候投**：bot 没暂停、有 `ready` 的 run、`hc get` 说终端活着、`status.state` 是 `idle` 或 `done`（`botrun::readiness`，已知对话框和 `blocked` 也不投）时投（`handoff::free`）；终端已经结束就把 run 记 `gone`。否则留在 `waiting`，原因记进 `reason`。同一个 bot 有别的交接正在 `sending` 时不投（等它先发完）。投递点：

- `yonder handoff` 收下后立即一次；
- `bot start` / `resume` / `reset` 就绪并发完第一条之后（输出里每份一行）；
- yonderd 每一轮，在给主 agent 投递之后：对有 `waiting` 交接的 bot 各查一次（`handoff::deliver_all`），hc 调不通就留着，下轮再试；投出去和 lost 的写进 `yonderd.log`。

同一个 bot 的多份按 id 顺序，一份一条消息，一次投递里可以连发多条、各自记状态；有一份没确定送到（拒绝或 lost）就停下，后面的留着。每次发送往 `bots.log` 写一行 `<UTC> <名字> handoff address=<地址> reason=<id>:delivered|refused|lost`。

**信封**（2.4 的规则，`handoff::envelope_of`）：

```text
[yonder kind=handoff id=H-0001 from=owner via=command time=<收下的 UTC 时间>]
A handoff for you: read it all, merge what will matter later into MEMORY.md (do not overwrite it), make tasks for follow-ups, read the paths it gives as you need them. Nobody needs a reply.
<payload untrusted="true" source="handoff">
<正文，行首的 [yonder 转成 \[yonder，</payload> 转成 <\/payload>>
</payload>
```

**`yonder handoff ls [<名字>] [--json]`**：最近的在前，每份一行：id、bot、state、收下的本地时间、via、大小、正文第一个非空行（截到 60 字），没投出去的在末尾括号里写原因。`--json` 是对象数组：`id`、`bot`、`state`、`via`、`created_at`、`delivered_at`、`address`、`reason`、`bytes`、`first_line`（不含正文）。台账不存在时不建。

**`yonder handoff prompt <名字>`**：打印一段英文提示词，贴给别处的模型：交接给谁（名字、`description`，它有自己的 `MEMORY.md`、不回复）；写什么（背景和目标、决定和原因、进行中的工作、未决问题、owner 的偏好、相关路径和聊天记录的位置）；不写什么（密钥；整段对话和长日志给路径）；一条可以直接运行的命令 `<yonder 的绝对路径> handoff <名字> <<'EOF' … EOF`（说明只交一次，`waiting` 也算交了）；不能运行命令的对话就只写正文，由 owner 粘贴进 `… handoff <名字> --via paste`。oneoff bot 报 `handoff_oneoff`。

名字叫 `ls` 或 `prompt` 的 bot 用 `yonder handoff <名字>` 交不了（会被当成子命令）。

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
| `yonder bot check` | 已实现 | 校验一个或全部定义，逐条一行 `error: <名字>: …` / `warning: <名字>: …`；有 error 退出 1。`--live`（B3a）再查机器连得上、cwd 在、harness 找得到，见 2.7 |
| `yonder bot sync` | 已实现 | `[<名字>...] [--force] [--dry-run]`：把 `AGENTS.md`（末尾拼上 `yonder manual`）推到 bot 工作目录，被别人改过的不覆盖（`drifted`，`--force` 照写）；把 `MEMORY.md` 拉回 bot 定义目录。每个 bot 一行，有 drifted / failed / 不合法时 `sync_incomplete` 退出 1。规则见 2.6 |
| `yonder manual` | 已实现 | 给 agent 看的 Yonder 用法（英文）：信封和 untrusted payload、`task` 和 `remind` 的例子、记忆和台账怎么分。唯一来源是 `crates/yonder-cli/src/manual.md`（编进二进制），`bot sync` 拼进 `AGENTS.md` 用的就是它 |
| `yonder bot env set` | 已实现 | 把一个声明过的 env 的值写进登录钥匙串（`-X` hex），从 stdin 读，不回显，只写不读；见 2.7 |
| `yonder bot start` | 已实现 | `[--task] [--cwd]`：启动检查，`hc run --bg` 起 bot，等就绪（已知对话框立即失败、不按键），发第一条；env 经 `bot exec` 从钥匙串注入；见 2.7 |
| `yonder bot stop` | 已实现 | `hc interrupt` 只中断这一轮，不关终端、不改会话；没在跑报 `not_running` |
| `yonder bot pause` | 已实现 | 结束它的终端（interrupt + kill，终端留着）、记 `gone`，会话保留，标记暂停；暂停着时 `start` 报 `bot_paused`；已暂停再 pause 输出 `already paused`；见 2.7 |
| `yonder bot resume` | 已实现 | 清掉暂停标记，像 `start` 一样起（续记着的会话）；没暂停报 `not_paused` |
| `yonder bot reset` | 已实现 | `--yes`：结束终端、清会话和暂停，开新会话，第一条是模板加近况包（`MEMORY.md` 前 4096 字节，untrusted）；没有 `--yes` 报 `confirm_needed` |
| `yonder bot rm` | 已实现 | `--yes`：目录挪进 `bots/.trash/<名字>-<UTC>/`，台账和钥匙串条目删掉，工作目录和终端不动；在跑报 `bot_running`，没有 `--yes` 报 `confirm_needed` |
| `yonder bot status` | 已实现 | 每个 bot 一行：state（`stopped`、`starting`、`ready`、`failed`、`gone`，暂停着 `paused`，hc 调不通时 `unknown`）、地址、会话、上次原因；顺手推进记录；`--json` |
| `yonder bot session` | 已实现 | `<名字> [<id>] [--clear]`：记下、打印（没有就打印 `none`）或清掉 bot 下次 start 时续的会话 |
| `yonder bot exec` | 已实现（内部） | hc 终端里跑的：从钥匙串取 env，`exec` harness；缺值报 `env_missing`，不起 |
| `yonder bot diagnostics` | 已实现 | 一份可以直接贴出去排查的纯文本：版本、系统、字段、暂停和 run、会话、env 有没有设（绝不取值）、文件、上次同步、problems；`--json` |
| `yonder log` | 已实现 | `[--bot <名字>] [--lines N]`：`yonderd.log` 和 `bots.log` 末尾各 N 行（默认 50）；`--bot` 只留提到它的行，再附它活着的终端屏幕 |
| `yonder handoff` | 已实现 | `<名字> [--via command\|paste]`，正文从 stdin：存成 `waiting`，bot 空闲时投一条 `kind=handoff` 的信封（正文 untrusted），最多送一次；上限 200 KiB；bot 自己整理进 `MEMORY.md`；见 2.8 |
| `yonder handoff prompt` | 已实现 | 打印一段英文提示词，贴给别处的对话，让它写出交接并运行（或由 owner 粘贴）`handoff` 命令 |
| `yonder handoff ls` | 已实现 | `[<名字>] [--json]`：最近的在前，每份一行：id、bot、state、时间、via、大小、第一行 |
| `yonder ui snapshot` | 计划（台账页，暂停） | 给 App 台账页的一次性快照，`--json` |
| `yonder ui act` | 计划（台账页，暂停） | App 上的延后、完成、取消、关任务，直接改库并留备注 |

`yonderd` 自己的参数（已实现）：`--once` 跑一轮就退出，`--now <UTC>` 把这一轮当成指定时间（测试和验收用）。

排查 bot 的顺序：叫停（`bot stop`）→ 重置（`bot reset`）→ 诊断（`bot diagnostics`），日志看 `yonder log --bot`。

## 4. yonderd

已实现（`crates/yonder-core/src/{daemon,firing,check_exec,hc}.rs`，`crates/yonder-cli/src/bin/yonderd.rs`）：

- **一轮**：每 `daemon.tick_secs` 秒（默认 15）一轮：把到点的提醒推进一步并写投递，开始到点的检查，把 `pending` 投递合成一条消息发给主 agent。错过多次的周期提醒（比如电脑睡过去）只触发一次，下次时间从现在往后算。
- **检查**：同时最多 2 个，在轮次旁边跑，不卡住轮次。用 `/bin/sh -c`，独立进程组，只带白名单环境变量（`PATH HOME USER SHELL LANG TZ TMPDIR`），60 秒超时整组杀掉，输出上限 1 MiB；投递里回显的输出最多 1500 字，先脱敏再截断。
- **找主 agent**：`hc ls --json --label <lead.label>` 里第一个活着的终端（`lead.label` 默认 `yonder.role=lead`），用 `hc send` 写进去。一条消息最多 256 KiB 的信封，用空行隔开。每次调用 hc 都有时限（`ls` 15 秒，`send` 120 秒）。
- **hc 的错误怎么算**：hc 退出非 0 时从 stderr 读它的错误 JSON。kind 在 `REFUSED_BEFORE_TYPING`（`not_found`、`machine_unknown`、`invalid_params`、`status_mismatch`）里的算没发出去，留着下轮再发；其他 kind、没有 kind、没有 JSON、被信号杀掉，都算可能送到了，记 `lost`。
- **本机通知**：`notify.local`（默认开）时到点也用 macOS 通知提示一下。
- **单实例**：状态目录里的 `yonderd.lock`（flock，内容是 pid）；第二个实例报 `already_running` 退出 1。`--once` 也拿这把锁。`yonder ctl status` 查 yonderd 在不在时会短暂拿这把锁的共享锁（微秒级），所以 yonderd 拿锁失败时在 250 毫秒内（`LOCK_PATIENCE`）每 5 毫秒再试一次才算被占（B3b；之前恰好撞上一次查看的 yonderd 会直接报 `already_running`，见 9.1）。
- **停止**：SIGTERM / SIGINT 后不再开始新的触发、检查和发送，2 秒内退出，退出码 0；发到一半的投递下次启动时记 `lost`。
- **日志**：状态目录下 `logs/yonderd.log`。
- **交接**（B5）：每一轮在给主 agent 投递之后，对有 `waiting` 交接的 bot 各查一次，空闲就投，见 2.8；发它的进程已经不在了的 `sending` 交接记 `lost`。
- **服务**：launchd label `dev.yonder.daemon`（`service.label` 可改），plist 由 `yonder ctl service-plist` 打印。

计划：

- 未排期：监视 bot 的终端状态、自动拉起（运行记录和状态机已有，见 2.7，现在只在命令里推进；B3b 没做）。
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
| bot 日志 | `<home>/logs/bots.log`（B3b，见 2.7） |
| 删掉的 bot | `<home>/bots/.trash/<名字>-<UTC>/`（`bot rm`，B3b） |
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
    └── .trash/            # bot rm 挪进来的目录，不删
```

bot 的工作目录是 `bot.toml` 里的 `cwd`（可以在别的机器上，经 hc 读写）：Yonder 只往里推 `AGENTS.md`，只从里面拉 `MEMORY.md`，不写别的（见 2.6）。台账多了一张 `bot_pushes` 表，记每个 bot 上次推出去的 `AGENTS.md` 的哈希。

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

**bot 页**（已实现，B4，`app/src/bots.rs`；样子见 `docs/DESIGN.md` 的「Bots 页」）：

- 入口：设置 → 电脑卡片 →「Bots」。
- 数据：app 不读台账，经已连上那台电脑的 core 跑 `machine.exec`（`on = None`，只在 core 自己的机器上）：`yonder bot ls --json` 和 `yonder bot status --json`，按名字合成一张列表（`yonder_model::bots::bot_rows`；status 里没有的算 `stopped`）。动作是 `yonder bot start|stop|pause|resume <名字>`。`yonder` 经登录 shell 找：`/bin/sh -c 'exec "${SHELL:-/bin/sh}" -lic '\''exec "$@"'\'' yonder-app "$@"' yonder-app yonder <参数…>`，每个参数独立传。JSON 的类型在 `yonder-model`（`bots.rs`，serde），照 CLI 现有输出写，CLI 不改；JSON 前面有登录 shell 打的字时从第一行 `[` 起读。
- 列表每行：名字、description、状态、机器、harness。状态 `stopped` 没在运行、`starting` 启动中、`ready` 就绪、`failed` 失败、`gone` 已结束、`paused` 已暂停（中性色）、hc 不通时 `unknown` 不确定。坏定义的照样列出，标「定义有误」和第一条 error，没有动作。
- 详情：上面这些 + 终端地址、会话 id、上次原因（`reason`，没有时用 `hc_error`）。按钮：没在运行 / 失败 / 已结束 →「启动」；就绪 →「叫停这一轮」「暂停」；已暂停 →「恢复」；有地址且 core 的终端列表里它没结束 →「看终端」。启动中没有按钮。
- 没有 reset 和 rm（不能撤销，留给命令行）。启动、叫停、暂停、恢复都不确认。动作进行中按钮显示进行中、一次只跑一个；结束后重读列表。失败时原地显示 yonder 在 stderr 的第一行（跳过登录 shell 自己的行），不弹窗。
- 找不到 `yonder`（登录 shell 退出 127）：写「这台电脑上没有 yonder」和怎么装（`cargo install --path crates/yonder-cli`），不显示内部错误。
- 刷新：进页面、动作之后、下拉、页面开着时每 10 秒，离开页面就停。上次的列表按电脑存在 app 的 storage 里，电脑没连上时显示它并标「不是最新的」。

计划：

- 暂停：台账页（手机单列、桌面分栏）、桌面管理页（IM 接入、凭据只写、后台状态）。

## 7. 记忆

- 每个 bot 的记忆是它工作目录里的 `MEMORY.md`，由 bot 自己写；Yonder 不替它写。`bot sync` 把它拉回 `~/.yonder/bots/<名字>/`，留 git 历史（已实现，B2；git 提交由用户自己做，Yonder 不跑 git）。
- 一次性 bot（`kind = "oneoff"`）不写记忆；它在回报里提「经验候选」，由主 agent 决定写到哪。
- 正在做的事进台账，不进记忆。
- `bot reset` 的近况包带上 `MEMORY.md` 的开头（已实现，B3b，见 2.7）；主 agent 冷启动的近况包仍是计划。`yonder handoff` 交来的内容由 bot 自己整理进 `MEMORY.md`（已实现，B5，见 2.8；Yonder 只投递，不替它写）。
- 不做：记忆体检、每天自动整理、记忆搜索（没有 `yonder memory search`）。
- Nowledge 只作为可选身份：`bot.toml` 有 `memory_id` 时，起 bot 时带上 `NMEM_AGENT_ID=<memory_id>`，别的不接。Yonder 不依赖 Nowledge，没有它一切照常。
- `bot sync` 不生成 hc 模板（B2 照此实现）：`bot start` 由 yonder 直接 `hc run` 起 bot（B3a 已实现），模板不再需要；hc 模板目录里原有的软链由用户自己删，Yonder 不碰。

## 8. 里程碑

| 包 | 内容 | 状态 |
|---|---|---|
| M1a | `yonder task`、`yonder remind`（含带检查的提醒）、`ledger.db` | 已完成 |
| M1b | `yonderd`：到点投递、检查、投递状态机、`ctl status`、`ctl service-plist` | 已完成 |
| D1 | 桌面版：`Yonder.app`、钥匙串存密钥、本地时区、桌面默认值和菜单、右键菜单 | 已完成 |
| S0 | 本文和仓库根 `AGENTS.md` | 已完成 |
| B1 | bot 定义格式、`bot new/ls/show/check`、`~/.yonder` 目录 | 已完成 |
| B2 | `bot sync`、`yonder manual` | 已完成 |
| B3a | `bot start/status/stop/session/env set`、`bot check --live`、`bot exec`、`bot_runs` 状态机 | 已完成 |
| B3b | `bot pause/resume/reset/rm/diagnostics`、`yonder log`、`bots.log`、暂停标记（迁移第 5 步） | 已完成（监视和自动拉起没做，未排期） |
| B5 | `yonder handoff`、`handoff ls`、`handoff prompt`、yonderd 投交接（迁移第 6 步） | 已完成 |
| B4 | App 里的 bot 页（列表、状态、启动、叫停、暂停、恢复）；交接卡在 `sending` 超过 10 分钟记 `lost`（迁移第 7 步） | 已完成 |

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
- `bot sync`：推送内容的格式、再同步 `unchanged`、拉回 `MEMORY.md` 和目标没有时不动本地、手改后 `drifted` 不覆盖和 `--force`、远端经 hc、hc 出错只让那个 bot `failed`、oneoff 跳过和不合法的报 error、`--dry-run` 不写文件不改台账也不建台账：`crates/yonder-cli/tests/cli.rs` 的 `bots_sync_to_where_they_work`、`a_dry_run_makes_no_ledger`，`crates/yonder-core/src/sync.rs` 的单元测试，以及 B2 验收第 3–9 条。这些都用假 hc（`tests/fake-hc.sh`，文件在 `$FAKE_HC_ROOT/<machine>/` 下）。
- 本机目标不跟随符号链接、`cwd` 不存在时不建：`sync::tests::local_files_are_never_followed`；`--dry-run` 的只读台账：`store::tests::remembers_pushes_and_reads_without_writing`。
- `yonder manual` 的行数和必须有的内容：`the_manual_is_for_agents`，B2 验收第 2 条。`cwd` 不合法时 `bot check` 报 error：B2 验收第 10 条，`bot::tests` 里的断言。
- 真实 hc（0.1.22）的 `hc file` 输出，B2 时只读实测（写只写在自己建的 `/tmp` 临时目录，用完删掉）：`read --json` 在 stdout 打一行 `{"first","last","lines","text"}`；错误在 stderr 打 `{"error":{"kind","message","hint"}}`：文件不存在 `not_found` 退出 3（父目录不存在也是），机器不认识 `machine_unknown` 退出 4，路径是目录 `invalid_params` 退出 6，不是 UTF-8 `not_text` 退出 1；超过 8 MB 只给前面一段、仍退出 0，`last` 小于 `lines`；`write` 读 stdin、建缺的目录，打 `{"bytes","path"}`。在一台真实的远端机器上只读过不存在的路径：带 `--on` 的相对路径确实从那台机器的家目录算起，报 `not_found`。`yonder bot sync --dry-run` 用真实 hc 跑过一个远端 bot（`cwd` 不存在、没有 `AGENTS.md` 要推）：输出 `MEMORY.md missing`；机器名不对时 `failed`，原因里没有 hc 的 hint。
- bot 起停：本机和远端的 run 参数、`--label yonder.bot=`、会话参数、就绪后发第一条、`already_running`、`stop` 发 `interrupt`、终端没了记 `gone` 和 `not_running`、远端有 env 拒绝、已知对话框和 `blocked` 立即失败不发消息、超时失败、oneoff 要 `--cwd` 和 `--task`、`starting` 卡住后被 `status` 记成 `failed`、hc 调不通时显示 `unknown` 不改台账、远端启动检查归因：`crates/yonder-cli/tests/cli.rs` 的 `bots_start_get_ready_and_stop`、`bots_elsewhere_resume_their_session`、`starts_that_cannot_get_ready_fail_and_say_why`、`a_start_cut_short_is_failed_later`、`oneoff_bots_get_a_place_and_a_task`，`botrun::tests`，`store::tests::runs_move_only_from_where_they_are`、`keeps_sessions_apart_from_runs`，以及 B3a 验收第 3–11 条。这些都用假 hc（`tests/fake-hc.sh`）。
- 登录 shell 包装：每个参数原样到达（含空格、`$`、引号、反引号、`*`）：`botrun::tests::the_login_shell_gets_each_argument_whole`；只有登录 shell 的 profile 里才在 PATH 上的 harness，`check --live` 找得到（没有那个 profile 时报找不到），`bot start` 起得来、参数原样：`harnesses_are_found_and_run_by_the_login_shell`（测试里 `SHELL=/bin/sh`，profile 经 `$ENV` 读）；model 映射：`botrun::tests::models_go_after_the_session`，B3a.1 验收第 3 条。
- 真实 hc 起 `pi`（B3a.1 验收第 4–5 条，本机，临时 home，模板为空、不发消息，用完 `hc kill` + `hc rm`）：`bot check --live` 在 hc exec 的登录 shell 里找到 `pi`，退出 0；`bot start` 3 秒内 `ready`，hc 报 `"status":{"app":"pi","state":"idle"}`；`bot status` 是 `ready`。harness 换成不存在的名字时 `check --live` 退出 1。B3a 时同样的起法因为终端 PATH 太短报 127。
- 暂停、恢复、重置、删除、诊断、日志（B3b）：暂停时 interrupt 再 kill、记 `gone`/`paused`、会话保留、再 pause 不调 hc、暂停着 `start` 报 `bot_paused` 不调 hc、`resume` 续会话和 `not_paused`、`reset` 没 `--yes` 不改东西、有 `--yes` 杀旧终端清会话、新 run 不续会话、第一条带近况包、`diagnostics` 不含文件内容、`rm` 在跑拒绝、挪进 `.trash`、台账忘掉、`log --bot` 按整词：`bots_pause_resume_reset_and_go_to_the_trash`；近况包的转义和 4096 字节按 UTF-8 截断：`botrun::tests::recaps_carry_the_start_of_memory_untrusted`；暂停标记不随 run 变、`forget_bot` 只删这个 bot：`store::tests::pauses_are_the_owners_and_bots_can_be_forgotten`；钥匙串只看有没有、删条目：`keychain::tests`；以及 B3b 验收第 2–7 条（含 env 的 `set`、诊断和日志里没有密钥值、`rm` 删掉测试条目）。都用假 hc。
- yonderd 偶发起不来（B3a.1 验收时 `yonderd_runs_alone_and_stops_on_sigterm` 报 `yonderd never started`）：原因是锁，不是慢。测试一边循环 `yonder ctl status`，一边等 yonderd 起来；`ctl status` 的 `running_pid` 拿共享锁看有没有人占，yonderd 恰好这时拿独占锁就失败、报 `already_running` 退出，测试等满 10 秒。手工复现：一边死循环 `ctl status`，一边起 yonderd，30 次里 2 次起不来。改成拿锁失败时 250 毫秒内重试（见第 4 节）后 60 次 0 次；`daemon::tests::a_look_at_the_lock_does_not_stop_a_start`（别人拿着共享锁 50 毫秒时 `acquire` 仍成功；把重试时限改成 0 它就失败）。测试的时限没改。
- B3b 的真实 hc 冒烟（本机，临时 home，harness `pi`，模型是自部署的那个，模板为空，除了 `reset` 自带的近况包不发别的，用完 `hc kill` + `hc rm`）：B3b 验收第 10 条，builder 跑过一次（没带 model），cindy 在 `929a96a` 上又跑了一次（带 model，另外确认过 hc 记下的 argv 里有 `--model`）。两次都是：`bot start` → `ready`（pi 起来后 hc 报 `idle`）；`bot pause` → 终端 `"state":"finished"`，`bot status` 是 `paused`；`bot resume` → 新终端 `ready`；`bot reset --yes` → 旧终端结束、新终端 `ready`，近况包写的是没有 `MEMORY.md`；最后 `pause` 退出 0；hc 里不留 smoke。
- 交接（B5）：拒收（空、超过 200 KiB、oneoff）不存、prompt 的内容、没在跑时等着、`bot start` 就绪后在第一条之后投、`working` 时等 yonderd 在 `idle` 时投且只投一次、暂停时等到 `resume`、hc 超时记 `lost` 不重发、打字前拒绝的留着下轮投一次、`reset` 后不重投、`handoff ls`、`bots.log` 里的行：`handoffs_go_once_when_the_bot_is_free`；正文的取舍和脱敏、信封转义、状态机只从来源状态走、发送进程不在了记 `lost`、`bot rm` 后等着的记 `lost`：`handoff::tests`；以及 B5 验收第 2–9 条。都用假 hc。
- env 注入：值从 stdin 进钥匙串（`.test` service）、未声明和保留名拒绝、带换行拒绝、`bot exec` 缺值不起、起起来的进程拿到值和 `NMEM_AGENT_ID`、假 hc 日志里没有值、run 行走 `bot exec`：`env_values_go_from_the_keychain_to_the_bot_only`、`keychain::tests::keeps_and_gives_back_values`（用完删掉条目），B3a 验收第 2–3 条。
- 真实 hc（0.1.22）上 B3a 时实测（只起自己的 `sh -c 'sleep …'` 临时终端，用完 `hc kill` 加 `hc rm`；没起 codex、pi、claude，没给别的终端发消息）：`run --bg --json` 打印 `{"address","agent","cwd","name","state","turn"}`；`get --json` 的 `status` 对 sh 是 null，`screen` 字段是 `"exact"`（保存方式，不是文字），屏幕文字要 `read --json` 的 `text`；`status.state` 的取值按 `hc schema` 是 idle、working、done、blocked、error；终端结束后 `get` 是 `"state":"finished"`，带 `exit_code`、`reason`；`interrupt --json` 对活着的终端打印 `{"key":"ctrl+c","outcome":"unknown"}` 退出 0（sh 里的 trap 确实收到了 INT），对结束了的报 `finished` 退出 11；`ls --json --label` 只列带这个 label 的；`rm` 打印 `{"removed":…}`；`exec` 不带 `--json` 时退出码就是程序的，带 `--json` 时退出 0、码在 `code` 里、输出 base64。在一台真实远端机器上起过 `sh -c 'pwd; sleep 4'`：`--cwd .`、`--cwd ..` 从那台机器的家目录算起；`--cwd '~'` 和不存在的目录都让 `run` 报 `spawn_failed`。
- 钥匙串在 hc 终端里能读到：在本机真实 hc 起的 sh 终端里跑 `security find-generic-password -s dev.yonder.bots.test … -w`，拿到了值（条目由 `security` 自己建，用完删掉）；同时看到终端的环境变量名来自 hc core（有 `HOME PATH USER`、`HC_TERMINAL_ID` 等，没有调用者的 `YONDER_HOME`）。
- 交接卡在 `sending`：发送进程看起来还在时，认领正好 10 分钟还不动，超过就记 `lost`、不能再认领：`handoff::tests::a_send_stuck_too_long_is_lost`。
- bot 页的数据类型和 CLI 的实际输出一致（几个 bot、坏定义、ready 带地址和会话、paused）：`crates/yonder-cli/tests/cli.rs` 的 `the_app_reads_what_bot_ls_and_status_print`；合并和跳过登录 shell 前缀：`yonder-model` 的 `bots::tests`。
- 登录 shell 包装的参数原样到达：`app/src/bots.rs` 的 `yonder_runs_in_the_login_shell_with_its_arguments_whole`；失败时取 yonder 的行不取 shell 的：`a_failure_says_yonder_s_first_line_not_the_shell_s`。
- bot 页（headless，隔离 core + 测试 SSH 电脑，登录 shell 的 `.profile` 把假 `yonder` 放上 PATH，假 yonder 记下收到的 argv）：各状态和坏定义的显示、详情里的地址和会话、坏定义没有按钮：`bots_are_listed_with_their_state`；四个动作各跑对的 argv、之后重读列表、没有别的调用：`bots_start_stop_pause_resume_run_yonder`；失败时只显示 stderr 里 yonder 的第一行：`bots_say_what_yonder_said_when_it_fails`；PATH 上没有 yonder 时的提示、不露 127 和 not found：`bots_say_when_yonder_is_not_there`；中文界面、列表和详情里没有 reset / rm（及重置、删除、移除）、没有任何对话框：`bots_have_no_reset_or_rm`。
- 没有 yonder 时 `/bin/sh -lic` 的实际表现（本机手工跑）：stderr 先打 `sh: no job control in this shell`，再打 `yonder-app: exec: yonder: not found`，退出 127。
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
- bot 页的画面：NSH 0.4 headless 不能截图；`--target macos` 试过，停在 Loading bots… 没出图，原因未查；bot 页的界面由 owner 手测。
- bot 页对真实 `yonder` 和真实电脑：只用假 yonder 测过；动作的超时（启动、恢复 240 秒，叫停、暂停 120 秒）是按 `bot start` 默认等 90 秒就绪定的，没在真实 bot 上量过。
- 真实 harness 的就绪：只对 `pi` 实测过（见 9.1 的 B3a.1 冒烟），codex、claude 没起过。就绪靠 hc 的 `status.state` 是 idle 或 done；不报 status 的 harness（如 sh）永远等不到就绪，只能超时。已知对话框的文字（`Hooks need review`、`open in another app`）照任务给的写，没在真实 harness 上见过。
- 远端的登录 shell：包装和 `check --live` 在远端都用那边的 `$SHELL -lic`，没在真实远端上起过 harness；`hc exec --on` 和 hc 终端里的 `$SHELL` 是不是同一个没核对过。
- 登录交互 shell 没有终端（`hc exec`）时，有的 rc 会报错或等输入：只在本机 zsh 上试过（cindy 的 `hc exec -- zsh -lic 'command -v pi'`，以及 B3a.1 冒烟）。
- harness 的登录状态：不查。
- 钥匙串：只在本机、登录着、钥匙串解锁时试过 hc 终端里能读；锁屏或钥匙串锁着时 `bot exec` 会怎样（弹窗还是失败）没试过。条目由 `/usr/bin/security` 建，它在条目的访问列表里；别的程序读会不会弹窗没试过。
- `hc interrupt` 对真实 harness 的效果：只对 sh 试过，结果总是 `outcome: unknown`。B3b 时在真实 hc 上看到：带 `--keep` 的终端，`interrupt` 之后终端仍是 live（zsh 接上）；`hc kill` 之后 `"state":"finished"`、`"reason":"killed"`，带 `--keep` 也一样。
- 会话 id 不会自动抓：Yonder 不从 harness 读会话 id，只用 `bot session <名字> <id>` 记下的。所以 `pause` 后 `resume` 能不能接上之前的对话，取决于有没有人记过会话；没记过就是开新会话（和 `start` 一样，没有近况包）。真实 harness 的 `resume` 参数（codex `resume`、pi `--session`、claude `--resume`）没在真实会话上试过能不能接上。
- `pause` 先 `interrupt` 再立刻 `kill`，中间不等；harness 有没有机会在被杀前存好会话，没看过。
- 交接投给真实 harness：B5 验收第 11 条只看 `pi` 的屏幕上出现了 `kind=handoff` 和正文；pi 会不会照提示把它整理进 `MEMORY.md`，没看。codex、claude 没试过。
- 交接只在 `idle`、`done` 时投：hc 只给 pi 报过这两个状态（见上）；harness 一直不报 status 时交接永远等着。`bot start` 发完第一条以后，真实 harness 多半在 `working`，那时交接要等 yonderd 的下一轮；yonderd 没在跑时就一直等。
- 一次投递连发多条时，后面几条发出去时 bot 已经在处理第一条（不再查 `idle`），真实 harness 怎么排队没看过。
- `hc send` 打字前拒绝的错误 kind 沿用 2.3 的白名单，交接这边没另外核对。
- `reset` 的近况包只在假 hc 上看过格式；真实 harness 收到后会怎么处理，B3b 冒烟只看到发出去之后它还是 `ready`。
- `bot sync` 往真实远端机器写 `AGENTS.md`（`hc file write --on`）：没做过，只在本机 `/tmp` 用 `hc file write` 试过输出格式；远端写入的权限、换行、已有文件的属性是否保留都没看过。
- `hc file write` 是不是原子写（先写临时文件再改名）：没查，远端 `AGENTS.md` 写到一半断了会怎样不知道；下次同步会因为内容既不是上次推的也不是这次要推的而报 `drifted`，要 `--force`。
