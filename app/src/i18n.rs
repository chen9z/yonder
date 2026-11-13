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
        "Herdr" => "Herdr",
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
        "Long-press a computer to rename or forget it." => "长按电脑可重命名或移除。",
        "Long-press a computer to forget it." => "长按电脑可以忘记它。",
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
        "Leave this phone's key on the computer" => "在电脑上留下手机的钥匙",
        "Then the password isn't kept" => "这样就不保存密码",
        "The password is kept on this phone, sealed" => "密码加密保存在手机上",
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
        "Sort agents: Priority" => "Agents 排序：按优先级",
        "Sort agents: Grouped by space" => "Agents 排序：按空间分组",
        "No agent status yet. Agents show up here when they report their status. Older versions and agents inside tmux may not." => "还没有 agent 状态。agent 报告状态后会出现在这里；旧版本和 tmux 里的 agent 可能不会报告。",
        "Fold or unfold its sub-agents" => "折叠或展开它的子 agent",
        "Spaces and agents" => "空间和 Agents",
        "New space on {}" => "在 {} 上新建空间",
        "New space there" => "在这台机器上新建空间",
        "Loading…" => "加载中…",
        "Menu" => "菜单",
        "More" => "更多",
        "New tab" => "新标签页",
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
        "Close" => "关闭",
        "Not now" => "稍后",
        "Tab" => "标签页",
        "Space" => "空间",
        "{} tabs" => "{} 个标签页",
        "{} ago" => "{}前",
        "1 tab" => "1 个标签页",
        "{} \u{2014} {}, {}" => "{} \u{2014} {}，{}",
        "{} terminals" => "{} 个终端",
        "1 terminal" => "1 个终端",
        "New tab here" => "在这里新建标签页",
        "Add a machine…" => "添加机器…",
        "Its spaces show here too" => "它的空间也会显示在这里",

        // The more sheet.
        "New terminal" => "新终端",
        "Split right" => "向右分屏",
        "Split down" => "向下分屏",
        "Zoom" => "放大",
        "Pane size" => "窗格大小",
        "Unzoom" => "取消放大",
        "Files" => "文件",
        "Changes" => "改动",
        "Worktrees" => "工作树",
        "Rename tab" => "重命名标签页",
        "Commands" => "命令",
        "Settings" => "设置",
        "Leave" => "断开",
        "Close pane" => "关闭面板",
        "Close pane?" => "关闭面板？",
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
        "The terminal's text is on the clipboard." => "终端文字已复制到剪贴板。",
        "What the program copied is on the clipboard." => "程序复制的内容已放到剪贴板。",
        "Paste" => "粘贴",
        "Key bar" => "按键栏",
        "{} keys" => "{} 个键",
        "ON THE BAR" => "栏上的键",
        "MORE KEYS" => "更多按键",
        "As at first" => "恢复默认",
        "Earlier" => "前移",
        "{} earlier" => "{} 前移",
        "Later" => "后移",
        "{} later" => "{} 后移",
        "Take off" => "移除",
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
        "No agents running" => "没有正在运行的 agent",
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
        "Search commands, spaces, agents" => "搜索命令、空间、agent",
        "Agent · {} · {}" => "Agent · {} · {}",
        "Next" => "下一步",
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
        "Else they keep the desktop's size, drawn smaller" => "关掉则保持电脑上的尺寸，缩小显示",
        "COMPUTER" => "电脑",
        "Connecting…" => "正在连接…",
        "Leave this computer" => "断开这台电脑",
        "THIS PHONE'S KEY" => "这台手机的密钥",
        "Copy" => "复制",
        "ABOUT" => "关于",
        "Copy diagnostics" => "复制诊断信息",
        "Paste it where you tell what went wrong." => "报告问题时粘贴进去即可。",
        "LANGUAGE" => "语言",
        "Language" => "语言",
        "System" => "跟随系统",

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

        // The commands' descriptions (the palette's).
        "Close a space and end its terminals (asks first unless confirmed)." => "关闭空间并结束它的终端（除非已确认，否则先询问）。",
        "Rename a space (asks when no name; an empty name: called after where its first terminal is again)." => {
            "重命名空间（没给名字时会询问；名字留空则重新以第一个终端所在位置命名）。"
        }
        "Move a space to a position (1 = first)." => "把空间移到某个位置（1 = 第一个）。",
        "Close a tab and end its terminals." => "关闭标签页并结束它的终端。",
        "Rename a tab (asks when no name; an empty name: called by its number again)." => "重命名标签页（没给名字时会询问；名字留空则恢复为编号）。",
        "Move a tab to a position in its space (1 = first)." => "把标签页移到所在空间的某个位置（1 = 第一个）。",
        "Close a pane and end its terminal." => "关闭面板并结束它的终端。",
        "Take a terminal's pane away and disable core keep; it keeps running (a sub-agent shows under its parent again)." => {
            "去掉终端的面板并取消 core 的保留；它继续运行（子 agent 重新显示在父 agent 下面）。"
        }
        "Pin an agent to the top of the agent lists (pinned true) or unpin it (false); every Herdr screen shows it." => {
            "把 agent 置顶到 agent 列表（pinned 为 true）或取消置顶（false）；所有 Herdr 屏幕都会同步。"
        }
        "End a terminal and its sub-agents." => "结束终端和它的子 agent。",
        "Show a space (by position or id)." => "显示某个空间（按位置或 id）。",
        "Show the next space." => "显示下一个空间。",
        "Show the previous space." => "显示上一个空间。",
        "Show a tab (by position in the space, or id)." => "显示某个标签页（按空间内的位置或 id）。",
        "Put a tab away on this screen (default: the shown one): out of the lists, still running. It comes back when shown or when an agent in it needs you." => {
            "在这块屏幕上收起标签页（默认当前的）：从列表里拿掉，但继续运行。显示它或其中的 agent 需要你时会回来。"
        }
        "Bring a settled tab back to the lists." => "把收起的标签页放回列表。",
        "Show the next tab." => "显示下一个标签页。",
        "Show the previous tab." => "显示上一个标签页。",
        "Show only this pane of its tab, or all again." => "只显示标签页里的这个面板，或恢复显示全部。",
        "Focus a pane (or the pane showing a terminal)." => "聚焦面板（或显示某个终端的面板）。",
        "Swap a pane (default: the focused one) with its neighbor in a direction, or with another pane of its tab (target)." => {
            "把面板（默认聚焦的那个）和某个方向的相邻面板交换，或和同一标签页的另一个面板（target）交换。"
        }
        "Show a terminal in a pane for now, in place of its own (no terminal: back)." => "暂时在面板里显示另一个终端（不给终端则恢复）。",
        "Show an agent (sub-agents too)." => "显示某个 agent（子 agent 也可以）。",
        "Show the next agent." => "显示下一个 agent。",
        "Show the previous agent." => "显示上一个 agent。",
        "Size a pane or split of a tab." => "调整标签页里面板或分屏的大小。",
        "Make a space: its name (default: none, so it's called after where its first terminal is), the folder its terminals start in (default: the focused terminal's), its first tab's name, optionally an agent (argv) and its first message (task), and the machine it lives on (default: the shown space's; empty string: the core's own)." => {
            "新建空间：名字（默认不起名，以第一个终端所在位置命名）、终端的起始文件夹（默认聚焦终端的）、第一个标签页的名字，可选的 agent（argv）和它的第一条消息（task），以及它所在的机器（默认当前空间的；空字符串表示 core 本机）。"
        }
        "Make a tab in a space (default: the shown one) with a terminal in folder (default: the focused terminal's), optionally an agent (argv) and its first message (task); or with a mod as its only pane (`mod`, its `props`). With no space open, a new tab starts a space." => {
            "在空间（默认当前的）里新建标签页，终端在 folder（默认聚焦终端的文件夹），可选 agent（argv）和它的第一条消息（task）；或者只放一个 mod 面板（`mod` 和它的 `props`）。没有打开的空间时，新标签页会新建一个空间。"
        }
        "Split a pane (default: the focused one) \"right\" or \"down\" with a new terminal, or with a mod (`mod`, its `props`) beside it." => {
            "把面板（默认聚焦的那个）向 \"right\" 或 \"down\" 分屏，放一个新终端，或者一个 mod（`mod` 和它的 `props`）。"
        }
        "Start a terminal: as a new tab (place \"tab\", default), or \"right\"/\"down\" of the focused pane." => {
            "启动终端：作为新标签页（place 为 \"tab\"，默认），或放在聚焦面板的 \"right\"/\"down\"。"
        }
        "Give a running terminal without a pane (a sub-agent) a pane: a new tab, or \"right\"/\"down\"." => {
            "给没有面板的运行中终端（子 agent）一个面板：新标签页，或 \"right\"/\"down\"。"
        }
        "Where right-clicks in a terminal (default: the focused one) go when its program takes the mouse: \"program\" or \"menu\"." => {
            "程序接管鼠标时，终端（默认聚焦的那个）里的右键交给谁：\"program\" 或 \"menu\"。"
        }
        "Focus the pane to the \"left\", \"right\", \"up\" or \"down\"." => "聚焦 \"left\"、\"right\"、\"up\" 或 \"down\" 方向的面板。",
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
        for c in herdc_engine::COMMANDS {
            assert!(zh(c.about).is_some(), "{}: {}", c.name, c.about);
        }
        for en in ["Close space?", "Rename tab", "Rename space", "Not a git repository"] {
            assert!(zh(en).is_some(), "{en}");
        }
    }
}
