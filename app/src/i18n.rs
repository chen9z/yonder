//! The app's words in the person's language. They are written in English
//! where they are said (`t!("…")`); Chinese is looked up here. The phone's
//! language picks, unless the settings say which.

use nsh_core::prelude::*;
use serde::{Deserialize, Serialize};
use std::fmt::Display;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Language {
    #[default]
    System,
    English,
    Chinese,
}

/// The language the settings chose (kept with the other [`crate::prefs`]).
pub fn chosen() -> Signal<Language> {
    global::<Language>()
}

/// Whether the app speaks Chinese now. Read in a reactive scope, it
/// follows the setting and the phone's language.
pub fn chinese() -> bool {
    match chosen().get() {
        Language::System => locale().as_str().starts_with("zh"),
        language => language == Language::Chinese,
    }
}

/// `en` in the app's language.
pub fn tr(en: &'static str) -> &'static str {
    if chinese() { zh(en).unwrap_or(en) } else { en }
}

/// Words made elsewhere (the engine's notes and questions, the commands'
/// descriptions) in the app's language, when they are known here.
pub fn tr_string(en: &str) -> String {
    chinese().then(|| zh_string(en)).flatten().unwrap_or_else(|| en.to_owned())
}

/// `en`'s Chinese: looked up, or made from a template's, with what filled
/// it (in Chinese too, if known).
fn zh_string(en: &str) -> Option<String> {
    zh(en).map(str::to_owned).or_else(|| {
        let (said, args) = TEMPLATES.iter().find_map(|t| Some((zh(t)?, filled(t, en)?)))?;
        Some(fill(said, &args.iter().map(|a| a as &dyn Display).collect::<Vec<_>>()))
    })
}

/// The engine's words made from a template, as it makes them.
const TEMPLATES: &[&str] = &[
    // A space closed: its name, its tabs, its terminals.
    "{} \u{2014} {}, {}",
    "{} tabs",
    "{} terminals",
    "{} needs you",
    "{} finished",
    "{} failed",
    "{} needs a core update; run hc machine update {}",
    "{} needs login; run hc machine login {}",
    "Commit failed: {}",
    "Cannot read this machine's layout: {}",
    "Herdr has no machine called {}. See hc machine ls; machines added with --temp stay out of Herdr.",
];

/// What filled `template`'s `{}`s to make `said`, if it was made from it.
fn filled(template: &str, said: &str) -> Option<Vec<String>> {
    let mut parts = template.split("{}");
    let mut rest = said.strip_prefix(parts.next()?)?;
    let mut args = Vec::new();
    for part in parts {
        let at = if part.is_empty() { rest.len() } else { rest.find(part)? };
        args.push(zh_string(&rest[..at]).unwrap_or_else(|| rest[..at].to_owned()));
        rest = &rest[at + part.len()..];
    }
    rest.is_empty().then_some(args)
}

/// A template's `{}`s filled in order.
pub fn fill(template: &str, args: &[&dyn Display]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut args = args.iter();
    let mut rest = template;
    while let Some(at) = rest.find("{}") {
        out.push_str(&rest[..at]);
        if let Some(arg) = args.next() {
            out.push_str(&arg.to_string());
        }
        rest = &rest[at + 2..];
    }
    out.push_str(rest);
    out
}

/// `t!("Spaces")`, `t!("Forget {}?", name)`: English as written, or what
/// the app's language says.
macro_rules! t {
    ($en:literal) => {
        $crate::i18n::tr($en)
    };
    ($en:literal, $($arg:expr),+ $(,)?) => {
        $crate::i18n::fill($crate::i18n::tr($en), &[$(&$arg as &dyn ::std::fmt::Display),+])
    };
}

/// The Chinese for the app's English.
fn zh(en: &str) -> Option<&'static str> {
    Some(match en {
        "Needs login" => "需要登录",
        "Needs update" => "需要更新",
        "Fold or unfold {}" => "折叠或展开 {}",
        "<1m" => "<1分钟",
        "{}m" => "{}分钟",
        "{}h" => "{}小时",
        "{}d" => "{}天",
        // The computers, and adding one.
        "COMPUTERS" => "电脑",
        "Add a computer" => "添加电脑",
        "Machine" => "机器",
        "Display name" => "显示名称",
        "Edit connection" => "编辑连接",
        "Test and save" => "测试并保存",
        "Leave blank to keep the current login." => "留空沿用当前登录凭据。",
        "Long-press a computer to edit, rename or forget it." => "长按电脑可编辑连接、重命名或移除。",
        "Enter a port from 1 to 65535." => "请输入 1 到 65535 之间的端口。",
        "This connection is already saved." => "此连接已保存。",
        "Connection not found." => "找不到此连接。",
        "Forget {}?" => "忘记 {}？",
        "Its key stays trusted nowhere else; adding it again asks for the password." => "它的主机密钥不再受信任；再次添加时需要输入密码。",
        "Forget" => "忘记",
        "Enter the computer and your user name on it." => "请填写电脑地址和你在它上面的用户名。",
        "Reaching the computer…" => "正在连接电脑…",
        "Trust {}?" => "信任 {}？",
        "{}\n{}\n\nCompare it with what the computer shows for\nssh-keygen -lf /etc/ssh/ssh_host_*_key.pub" => {
            "{}\n{}\n\n请与电脑上这条命令的输出比对：\nssh-keygen -lf /etc/ssh/ssh_host_*_key.pub"
        }
        "Trust" => "信任",
        "Logging in…" => "正在登录…",
        "Reach the computer running Herdr over SSH: its name or address on this network, or its Tailscale name." => {
            "通过 SSH 连接运行 Herdr 的电脑：填写它在局域网里的名字或地址，或者它的 Tailscale 名字。"
        }
        "Computer" => "电脑",
        "User" => "用户名",
        "Port" => "端口",
        "Session" => "会话",
        "Show the source" => "显示源码",
        "Show as it reads" => "显示排版后的样子",
        "Source" => "源码",
        "Preview" => "预览",
        "No network." => "没有网络。",
        "{} isn't taking SSH connections." => "{} 不接受 SSH 连接。",
        "{} doesn't answer." => "{} 没有回应。",
        "Can't find {} on this network." => "在这个网络上找不到 {}。",
        "Can't reach {}." => "连不上 {}。",
        "Show the password" => "显示密码",
        "Show" => "显示",
        "Hide" => "隐藏",
        "{} didn't take the password kept for it." => "{} 不接受保存的密码。",
        "Enter the password" => "重新输入密码",
        "Password for {}" => "{} 的密码",
        "Wrong user name or password." => "用户名或密码不对。",
        "The computer didn't answer the login." => "电脑没有回应登录。",
        "The computer didn't answer in time" => "电脑太久没有回应",
        "Type a branch name." => "输入一个分支名。",
        "Notes" => "提醒",
        "Important only" => "只提醒要紧的",
        "All" => "全部",
        "Off" => "不弹出",
        "This computer's key was never trusted. Edit the connection to check it." => "这台电脑的主机密钥没有确认过。请编辑连接，重新核对。",
        "Git doesn't take that as a branch name." => "git 不接受这个分支名。",
        "Put some letters or digits in the branch name." => "分支名里要有字母或数字。",
        "Move {}" => "移动 {}",
        "A session is letters, digits and . _ - / only." => "会话名只能用字母、数字和 . _ - /。",
        "Optional: another herdr's HC_SESSION on that computer." => "可选：那台电脑上另一个 herdr 的 HC_SESSION。",
        "Password" => "密码",
        "Kept on this phone, sealed: nothing is left on the computer." => "加密保存在手机上，电脑上不留任何东西。",
        "Connect" => "连接",
        "Cancel" => "取消",

        // The line.
        "Connecting to {}…" => "正在连接 {}…",
        "No network. Reconnecting when it's back…" => "没有网络，恢复后自动重连…",
        "{} isn't taking SSH connections. Trying again…" => "{} 拒绝了 SSH 连接，正在重试…",
        "{} doesn't answer. Trying again…" => "{} 没有响应，正在重试…",
        "Can't find {} on this network. Trying again…" => "在当前网络找不到 {}，正在重试…",
        "Can't reach {}. Trying again…" => "连不上 {}，正在重试…",
        "Connection lost. Reconnecting…" => "连接断开，正在重连…",
        "Not connected" => "未连接",

        // The workspace.
        "Spaces" => "空间",
        "Agents" => "Agents",
        "Pinned" => "已置顶",
        "Needs you" => "等你处理",
        "Done" => "完成",
        "Working" => "工作中",
        "Idle" => "空闲",
        "Priority" => "按优先级",
        "Grouped by space" => "按空间分组",
        "Sort agents" => "Agents 排序",
        "GENERAL" => "通用",
        // The main agent.
        "Main" => "主 Agent",
        "Main agent" => "主 Agent",
        "Main agent: {}" => "主 Agent：{}",
        "No main agent" => "不设主 Agent",
        "None" => "未设置",
        "Make it the main agent" => "设为主 Agent",
        "No longer the main agent" => "不再作为主 Agent",
        "Your main agent" => "你的主 Agent",
        "Choose the agent you talk to. It shows here, ready for what you say, with the agents it hands work to above it." => "选一个你平时对话的 agent。它会显示在这里，随时听你说；它分派出去的 agent 列在上面。",
        "{} isn't running" => "{} 没有在运行",
        "Its space is kept. Start it there again, or choose another." => "它的空间还在，可以在那里重新启动它，或者换一个。",
        "Choose an agent" => "选择 agent",
        "Message {}" => "给 {} 发消息",
        "Message {} · / for commands" => "给 {} 发消息，/ 用命令",
        "Show the terminal" => "显示终端",
        "Show the conversation" => "显示对话",
        "No Pi session found for it." => "没找到它的 Pi 会话文件。",
        "It is waiting for an answer in its terminal." => "它在终端里等你回答，先去终端处理。",
        "Not sent" => "没发出去",
        "Can't show it as a conversation" => "没法显示成对话",
        "Earlier context summed up" => "前面的内容已压缩",
        "Failed: {}" => "出错了：{}",
        "Stopped" => "已中断",
        "{} images" => "{} 张图片",
        "{} steps" => "{} 步",
        "1 step" => "1 步",
        "1 message" => "1 条消息",
        "{} messages" => "{} 条消息",
        "Worked {}" => "工作了 {}",
        "{} s" => "{} 秒",
        "{} min" => "{} 分钟",
        "{} h {} min" => "{} 小时 {} 分钟",
        "Working · {}" => "正在工作 · {}",
        "From {}" => "{} 发来",
        "Show all" => "展开全部",
        "Clear" => "清空",
        "Context {}%" => "上下文 {}%",
        "Context {}% used" => "上下文已用 {}%",
        "(no result yet)" => "（还没有结果）",
        "Working…" => "正在工作…",
        "Send a picture" => "发图片",
        "Picture not sent" => "图片没发出去",
        "Type an answer" => "输入回答",
        "Still working" => "还在工作",
        "It didn't stop yet; its terminal shows why." => "还没停下来，终端里能看到原因。",
        "Couldn't stop it" => "没能停下",
        "Show earlier messages" => "显示更早的消息",
        "New messages" => "有新消息",
        "Go to the newest" => "回到最新",
        "Your message" => "你的消息",
        "Write it again" => "放进输入框",
        "Edit" => "修改",
        "Editing your message" => "正在修改这条消息",
        "Cancel editing" => "取消修改",
        "Not changed" => "没改成",
        "That message isn't in the conversation any more." => "这条消息已经不在对话里了。",
        "Edit it once it's done, or stop it first." => "等它做完再改，或者先让它停下。",
        "Commands go in once it's done." => "等它做完再用命令。",
        "Going back to before it…" => "正在回到这条消息之前…",
        "Couldn't edit it" => "没能修改",
        "A moment" => "稍等",
        "It is still going back to before your message." => "还在回到那条消息之前，稍等再发。",
        "It is asking something" => "它在问你",
        "Answer it first, then edit." => "先回答它，再修改。",
        "It went back to before that message, but your words didn't go: send them again." => "已经回到那条消息之前，但新的话没发出去，再发一次就行。",
        "Something is open in its terminal; nothing changed." => "它的终端里开着别的东西，什么都没改。",
        "Its session tree didn't open." => "它的会话树没打开。",
        "Its tree didn't show only your messages." => "会话树没切到只看你的消息。",
        "That message isn't where it was in its tree; nothing changed." => "会话树里那条消息的位置对不上，什么都没改。",
        "It didn't go back." => "它没有退回去。",
        "It asked about a summary in a way not known." => "它问是否摘要的方式不认识。",
        "Its field didn't empty." => "它的输入框没清空。",
        "Named “{}”" => "会话改名为“{}”",
        "Model: {}" => "模型换成 {}",
        "Thinking: {}" => "思考强度：{}",
        "Sum up the context so far" => "压缩上下文",
        "Start a new session" => "开始新会话",
        "Name this session" => "给会话起名",
        "Change the model" => "换模型",
        "How hard it thinks" => "调整思考强度",
        "Go back to an earlier point" => "回到之前某一步",
        "A new session from an earlier message" => "从之前的消息分出新会话",
        "Copy this session" => "复制当前会话",
        "Open another session" => "打开另一个会话",
        "This session's details" => "查看会话信息",
        "Save the session as a file" => "把会话导出成文件",
        "Pi's settings" => "Pi 的设置",
        "Load extensions and skills again" => "重新加载扩展和技能",
        "The message is on the clipboard." => "消息已复制到剪贴板。",
        "Sending…" => "发送中…",
        "Queued: it reads it once this step is done" => "已排队，这一步做完它就会看到",
        "Sent" => "已发送",
        "What it was given and gave back is on the clipboard." => "输入和结果已复制到剪贴板。",
        "GIVEN" => "输入",
        "FAILED" => "失败",
        "GAVE BACK" => "结果",
        "Space bar" => "空格",
        "Tab key" => "Tab 键",
        "{} is asking" => "{} 在问你",
        "Open the terminal" => "打开终端",
        "The keys go to its terminal; what you write below answers there too." => "按键直接发到它的终端；在下面写的字也会作为回答发过去。",
        "Yesterday {}" => "昨天 {}",
        "{}/{} {}" => "{}月{}日 {}",
        "No agent status yet. Agents show up here when they report their status. Older versions and agents inside tmux may not." => "还没有 agent 状态。agent 报告状态后会出现在这里；旧版本和 tmux 里的 agent 可能不会报告。",
        "Fold or unfold its sub-agents" => "折叠或展开它的子 agent",
        "Spaces and agents" => "空间和 Agents",
        "New space on {}" => "在 {} 上新建空间",
        "New space there" => "在这台机器上新建空间",
        "Loading…" => "加载中…",
        "More" => "更多",
        "New space" => "新空间",
        "No spaces yet" => "还没有空间",
        "Loading spaces…" => "正在加载空间…",
        "Herdr's layout is unavailable here" => "这里无法使用 Herdr 的布局",
        "It shows on the desktop." => "它显示在桌面端。",
        "The {} pane" => "{} 面板",
        "Open" => "打开",
        "Put away" => "收起",
        "Put away tab" => "收起标签页",
        "{} put away" => "已收起 {} 个",
        "Worktrees…" => "工作树…",
        "{} more spaces" => "还有 {} 个空间",
        "{} of {}" => "{} / {}",
        "Previous file" => "上一个文件",
        "Next file" => "下一个文件",
        "open" => "已打开",
        "detached" => "游离",
        "Switch space" => "切换空间",
        "All spaces and agents" => "全部空间和 agent",
        "{} more" => "还有 {} 条",
        "Fewer" => "收起",
        "Cannot load the phone's key: {}" => "读不到手机的密钥：{}",
        "Rename" => "重命名",
        "Rename {}" => "重命名 {}",
        "More options" => "更多选项",
        "Close" => "关闭",
        "Not now" => "稍后",
        "Tab" => "标签页",
        "{} tabs" => "{} 个标签页",
        "{} ago" => "{}前",
        "1 tab" => "1 个标签页",
        "{} \u{2014} {}, {}" => "{} \u{2014} {}，{}",
        "{} terminals" => "{} 个终端",
        "New tab here" => "在这里新建标签页",
        "Add a machine…" => "添加机器…",

        // The more sheet.
        "Split right" => "向右分屏",
        "Split down" => "向下分屏",
        "Zoom" => "放大",
        "Pane size" => "窗格大小",
        "Unzoom" => "取消放大",
        "Files" => "文件",
        "Changes" => "改动",
        "Worktrees" => "工作树",
        "Rename tab" => "重命名标签页",
        "Settings" => "设置",
        "Close pane" => "关闭面板",
        "Close pane?" => "关闭面板？",
        "Close tab?" => "关闭标签页？",
        "{} is {}. Closing the tab ends it." => "{}：{}。关闭标签页会结束它。",
        "{} is {}. Closing the pane ends it." => "{}：{}。关闭面板会结束它。",

        // The terminal.
        "{} lines back" => "已回看 {} 行",
        "Back to the latest output" => "回到最新输出",
        "Ended" => "已结束",
        "Ended (exit {})" => "已结束（退出码 {}）",
        "Select text" => "选择文字",
        "Open {}" => "打开 {}",
        "Copy all" => "全部复制",
        "Copied" => "已复制",
        "Add it to ~/.ssh/authorized_keys on the computer." => "把它加到电脑的 ~/.ssh/authorized_keys 里。",
        "The terminal's text is on the clipboard." => "终端文字已复制到剪贴板。",
        "What the program copied is on the clipboard." => "程序复制的内容已放到剪贴板。",
        "Paste" => "粘贴",
        "Write a message" => "写消息",
        "Jump to a terminal" => "跳到终端",
        "A space, an agent, a folder…" => "空间、agent、目录…",
        "No terminal is called that" => "没有这样的终端",
        "helper" => "子 agent",
        "Type, paste or speak it" => "输入、粘贴，或用语音输入",
        "It goes in as one paste, then Enter." => "整段粘贴进去，再按回车。",
        "Key bar" => "按键栏",
        "{} keys" => "{} 个键",
        "ON THE BAR" => "栏上的键",
        "MORE KEYS" => "更多按键",
        "YOUR OWN" => "自定义",
        "Words it types, like /compact" => "要输入的文字，如 /compact",
        "Add the key" => "添加这个键",
        "Then press Enter" => "输完按回车",
        "{}, then Enter" => "{}，然后回车",
        "As at first" => "恢复默认",
        "{} earlier" => "{} 前移",
        "{} later" => "{} 后移",
        "Take {} off" => "移除 {}",
        "Enter" => "回车",
        "Hide keyboard" => "收起键盘",
        "Left" => "左",
        "Down" => "下",
        "Up" => "上",
        "Right" => "右",
        "Pipe" => "竖线",
        "Tilde" => "波浪号",
        "Slash" => "斜杠",
        "Dash" => "减号",

        // Agents.
        "Pin to the top" => "置顶",
        "Unpin" => "取消置顶",
        "Stop it" => "停止",
        "Stop this agent?" => "停止这个 agent？",
        "It ends with its sub-agents." => "它和它的子 agent 都会结束。",
        "Stop" => "停止",
        "{}, sub-agent" => "{}，子 agent",
        "Keep" => "保留",
        "Go to" => "前往",
        "Send" => "发送",
        "blocked" => "等你处理",
        "working" => "工作中",
        "done" => "完成",
        "idle" => "空闲",

        // Commands and the new-menu.
        "New" => "新建",
        "Terminal" => "终端",
        "New worktree…" => "新建工作树…",
        "Open worktree…" => "打开工作树…",
        "Remove worktree…" => "删除工作树…",
        "{} failed" => "{} 失败",
        "{} needs you" => "{} 等你处理",
        "{} finished" => "{} 完成了",
        "{} needs a core update; run hc machine update {}" => "{} 需要更新 core：运行 hc machine update {}",
        "{} needs login; run hc machine login {}" => "{} 需要登录：运行 hc machine login {}",
        "Commit failed: {}" => "提交失败：{}",
        "Cannot read this machine's layout: {}" => "读不了这台机器的布局：{}",
        "Herdr has no machine called {}. See hc machine ls; machines added with --temp stay out of Herdr." => "Herdr 里没有叫 {} 的机器。看看 hc machine ls；用 --temp 加的机器不在 Herdr 里。",
        "Sure?" => "确定吗？",
        "OK" => "好",

        // Settings.
        "TERMINAL" => "终端",
        "Font size" => "字号",
        "{} pt" => "{} 磅",
        "Smaller text" => "缩小字号",
        "Larger text" => "放大字号",
        "Fit terminals to the phone" => "终端按手机尺寸排版",
        "Fitted, the terminal you look at takes the phone's rows and columns; else it keeps the desktop's size, drawn smaller." => "打开时，正在看的终端按手机的行列排版；关掉则保持电脑上的尺寸，缩小显示。",
        "Copy the public key" => "复制公钥",
        "On a computer, in ~/.ssh/authorized_keys, it logs the phone in without the password." => "放进电脑的 ~/.ssh/authorized_keys，手机就不用密码登录。",
        "Reorder spaces" => "调整顺序",
        "Show all {}" => "显示全部 {} 个",
        "Show less" => "收起",
        "Pin" => "置顶",
        "Pin or unpin" => "置顶或取消置顶",
        "Home folder" => "主目录",
        "COMPUTER" => "电脑",
        "Connecting…" => "正在连接…",
        "Leave this computer" => "断开这台电脑",
        "THIS PHONE'S KEY" => "这台手机的密钥",
        "Copy" => "复制",
        "ABOUT" => "关于",
        "Copy diagnostics" => "复制诊断信息",
        "Paste it where you tell what went wrong." => "报告问题时粘贴进去即可。",
        "Language" => "语言",
        "System" => "跟随系统",
        "Appearance" => "外观",
        "Light" => "浅色",
        "Dark" => "深色",

        // Files and changes.
        "Loading files…" => "正在加载文件…",
        "No files" => "没有文件",
        "Loading file…" => "正在加载文件…",
        "Empty file" => "空文件",
        "File list limited to {} entries per folder and {} total" => "文件列表最多显示每个文件夹 {} 项、共 {} 项",
        "Loading changes…" => "正在加载改动…",
        "No uncommitted changes" => "没有未提交的改动",
        "Showing the first {} changed files" => "只显示前 {} 个改动的文件",
        "Commit message" => "提交说明",
        "Commit" => "提交",
        "Committing…" => "正在提交…",
        "Loading diff…" => "正在加载 diff…",

        // Worktrees.
        "Worktrees of {}" => "{} 的工作树",
        "Reading worktrees…" => "正在读取工作树…",
        "filter worktrees" => "筛选工作树",
        "No worktree matches" => "没有匹配的工作树",
        "main checkout" => "主检出",
        "New worktree" => "新建工作树",
        "New worktree of {}" => "新建 {} 的工作树",
        "branch name" => "分支名",
        "Create" => "创建",
        "Creating worktree…" => "正在创建工作树…",
        "Remove worktree" => "删除工作树",
        "Remove worktree {}?" => "删除工作树 {}？",
        "Remove" => "删除",
        "Closes the space and deletes {}. The branch stays." => "会关闭这个空间并删除 {}，分支保留。",
        "\n\nIt has uncommitted changes: they will be lost." => "\n\n它有未提交的改动，会丢失。",
        "\n\nSubmodule {} has commits that aren't pushed anywhere: they will be lost." => "\n\n子模块 {} 有没推送到任何地方的提交，会丢失。",
        "There is more to lose now. Remove it anyway?" => "现在会丢失更多内容。仍然删除吗？",
        "It changed since this opened" => "打开之后它已经变了",
        "This is the repository's main checkout, not a worktree." => "这是仓库的主检出，不是工作树。",
        "Not in a git repository" => "不在 git 仓库里",
        "Not a git repository" => "不是 git 仓库",

        // Adding a machine.
        "Add a machine" => "添加机器",
        "Its terminals and spaces show here beside this computer's. The computer dials it over ssh, so it is reached as the computer reaches it." => {
            "它的终端和空间会和这台电脑的显示在一起。由电脑通过 ssh 连接它，所以电脑怎么连得到，这里就怎么连。"
        }
        "READING ~/.SSH/CONFIG…" => "正在读取 ~/.ssh/config…",
        "YOUR SSH CONFIG LISTS NO HOSTS" => "你的 ssh 配置里没有 host",
        "FROM YOUR SSH CONFIG" => "来自你的 ssh 配置",
        "OR TYPE HOW SSH REACHES IT" => "或者填写 ssh 连接它的方式",
        "Add" => "添加",
        "Added" => "已添加",
        "Add {}" => "添加 {}",
        "Add the machine typed" => "添加填写的机器",
        "also {}" => "也叫 {}",
        "{} is a Mac: allow its folders once" => "{} 是 Mac：需要授权一次文件夹访问",
        "macOS asks on its own screen before anything reads Desktop, Documents or Downloads, and waits. Give {} Full Disk Access there." => {
            "macOS 在读取桌面、文稿或下载文件夹前会在它自己的屏幕上询问并一直等待。请在那台 Mac 上给 {} 完全磁盘访问权限。"
        }
        "Open the setting there" => "在那台 Mac 上打开设置",
        "Open the setting" => "打开设置",

        // The engine's questions and notes.
        "Rename space" => "重命名空间",
        "Empty: named after its folder" => "留空：用文件夹名命名",
        "Empty: its number" => "留空：用它的编号",
        "Close space?" => "关闭空间？",
        "Layout unavailable" => "布局不可用",
        "Layout changed elsewhere" => "布局在别处改了",
        "The other screen's layout is shown." => "现在显示的是另一块屏幕的布局。",
        "This machine's layout is unavailable. Try again when it reconnects." => "这台机器的布局不可用，重连后再试。",
        "Terminal failed to start" => "终端启动失败",
        "Could not change terminal pane" => "无法更换终端所在的面板",
        "This terminal already has a pane change in progress." => "这个终端的面板正在更换中。",
        "A terminal's pane must stay on its own machine." => "终端的面板必须留在它自己的机器上。",
        "Cannot preview this diff (too large or unreadable)" => "无法预览这个 diff（太大或无法读取）",
        "Cannot preview this file (binary, too large, or unreadable)" => "无法预览这个文件（二进制、太大或无法读取）",
        "Binary file · no text diff" => "二进制文件 · 没有文本 diff",
        "Binary file changed" => "二进制文件有改动",
        "No text diff available" => "没有可显示的文本 diff",
        "Type a commit message." => "请填写提交说明。",
        "This file is no longer available" => "这个文件已经不在了",
        "Binary file · no text preview" => "二进制文件 · 无法预览文本",
        "Cannot list this folder" => "无法列出这个文件夹",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_fill_in_order() {
        assert_eq!(fill("{} is {}. Closing the pane ends it.", &[&"Codex", &"working"]), "Codex is working. Closing the pane ends it.");
        assert_eq!(fill("{}：{}。关闭面板会结束它。", &[&"Codex", &"工作中"]), "Codex：工作中。关闭面板会结束它。");
        assert_eq!(fill("no args", &[]), "no args");
    }

    #[test]
    fn words_made_from_a_template_have_theirs() {
        assert_eq!(zh_string("Codex needs you").as_deref(), Some("Codex 等你处理"));
        assert_eq!(zh_string("mini needs login; run hc machine login mini").as_deref(), Some("mini 需要登录：运行 hc machine login mini"));
        assert_eq!(zh_string("Commit failed: nothing to commit").as_deref(), Some("提交失败：nothing to commit"));
        assert_eq!(zh_string("Something else"), None);
        assert_eq!(zh_string("home \u{2014} 1 tab, 3 terminals").as_deref(), Some("home \u{2014} 1 个标签页，3 个终端"));
        for t in TEMPLATES {
            assert!(zh(t).is_some_and(|z| z.matches("{}").count() == t.matches("{}").count()), "{t}");
        }
    }

    /// Every `t!("…")` in the app's sources has its Chinese, with as many `{}`.

    #[test]
    fn every_word_said_has_its_chinese() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src");
        let mut missing = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.ends_with("i18n.rs") {
                continue;
            }
            let source = std::fs::read_to_string(&path).unwrap();
            // `t!(` itself, not the end of `format!(`.
            for said in source.split("t!(\"").skip(1).zip(source.split("t!(\"")).filter(|(_, before)| !before.ends_with(|c: char| c.is_alphanumeric())).map(|(said, _)| said) {
                let Some(end) = said.find("\"") else { continue };
                let en = said[..end].replace("\\n", "\n");
                match zh(&en) {
                    Some(zh) if zh.matches("{}").count() == en.matches("{}").count() => {}
                    _ => missing.push(format!("{}: {en:?}", path.file_name().unwrap().to_string_lossy())),
                }
            }
        }
        assert!(missing.is_empty(), "no Chinese for:\n{}", missing.join("\n"));
    }

    /// What the palette shows of every command, and the engine's questions.
    #[test]
    fn the_engines_words_are_known() {
        for en in ["Close space?", "Rename tab", "Rename space", "Not a git repository"] {
            assert!(zh(en).is_some(), "{en}");
        }
    }
}
