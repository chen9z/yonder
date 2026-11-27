# AGENTS.md

Yonder：跑在自己电脑上的个人 agent 底座。agent 跑在 herdr-core（`hc`）里，Yonder 管台账、提醒、投递、bot，并提供 Android 和 macOS 的 app。规格见 `docs/spec.md`。

## 代码地图

- `crates/yonder-model`：共享类型和信封 v1，只依赖 serde、serde_json（Android 也编它）。
- `crates/yonder-core`：存储（`ledger.db`）、配置、提醒规则、检查执行、投递状态机、hc 调用（`hc.rs`）、bot 定义（`bot.rs`）、bot 同步（`sync.rs`）、bot 起停和运行状态机（`botrun.rs`）、钥匙串里的 bot env（`keychain.rs`）。
- `crates/yonder-cli`：两个二进制，`yonder`（命令行）和 `yonderd`（常驻后台）；集成测试在 `tests/cli.rs`，假 hc 是 `tests/fake-hc.sh`。
- `crates/herdc-*`：app 用的 SSH、协议、终端、引擎和测试工具（`herdc-testkit` 起隔离的 hc core）。
- `app/`：NSH app，独立 workspace，同一份代码出 Android 和 macOS 版；只能依赖 `yonder-model`。
- `docs/spec.md`：规格；`docs/DESIGN.md`：手机 app 的界面和交互。

## 命令

```sh
. ./env.sh               # 先 source：工具链路径
./scripts/check.sh       # clippy + 根 workspace 和 app/ 的全部测试，最后一行 all good
sh app/macos.sh          # 打 ad hoc 签名的 Yonder.app，最后一行是它的路径
sh app/apk.sh --debug    # 调试签名的 APK；不带 --debug 用正式签名
```

不要另建 target 目录或 worktree；构建前后看一眼磁盘。

## 工程规则

1. `docs/spec.md` 是唯一依据。改范围、命令、架构时，和代码在同一次提交里更新它；实测得出的结论写回它的「待验证」一节（从待验证挪到已实测，写明依据）。
2. 长期存在的状态（投递、提醒、bot 运行状态）任何非终态都要有出路：超时、重启、对方消失时会走到哪一态要写清楚；迟到的结果不能覆盖已经更新的状态。规则分散在几处时，收拢成一个显式状态机（参照 `docs/spec.md` 的投递状态机）。
3. hc 只在 `crates/yonder-core/src/hc.rs` 里调用，其他模块用它的接口；测试用假 hc 或 `herdc-testkit` 的隔离 core，不碰真实的 herdr-core。
4. 不凭记忆断言第三方（hc、`security`、NSH、launchd）的行为：查源码或实测；没实测的写进 `docs/spec.md` 的「待验证」。
5. 改界面要截图，逐张看过再提交。
6. 不改验收命令、不为了过测试改断言；改断言要说明理由。
7. 密钥不进命令行参数、不进日志、不进提示词；加密失败就报错，不退回明文。

## 公开仓库

仓库是公开的。代码、注释、测试、文档里不出现个人信息：家目录绝对路径、真实机器名、Tailscale 主机、域名、真实 agent 名、会话 id。示例名用 nova（主 agent）、gpu-box、homelab、ops、trainer，路径用 `/tmp/example` 或临时目录。

提交前跑这两条，都要退出 1（没有命中）：

```sh
git grep -inE '(/User[s]/che[n]|/home/che[n]|workmachin[e]|echoni[l]|tailscale\.[a-z0-9-]+|\bcind[y]\b|\ble[o]\b|\bott[o]\b|\bopen-je[v]\b|\bstrat[a]\b|\bninfe[r]\b)' -- 'crates/yonder-*' 'app/' 'docs/spec.md' 'AGENTS.md'
git grep -inE '(\bche[n]\b|mac ?min[i]|\bubunt[u]\b|\bag[y]\b|antigravit[y]|chen9[z]/|/User[s]/)' -- 'docs/spec.md' 'AGENTS.md'
```

每个词最后一个字母加了方括号，匹配的东西不变，只是这两行不会命中它们自己。
