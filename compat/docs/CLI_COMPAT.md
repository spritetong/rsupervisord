# rsupervisord: CLI 兼容性分析 (rsupervisorctl vs. supervisorctl) (CLI_COMPAT.md)

| Document Version | Status | Target Language | Scope |
| :--- | :--- | :--- | :--- |
| **v1.1.0** | Draft / For Review | Rust (Edition 2024) | `rsupervisorctl` 客户端命令格式与 Python `supervisorctl` 的兼容性分析、逐项需求、示例与优先级分类;可执行基准见 §9 / [`../compat/README.md`](../compat/README.md) |

---

## 1. 目的与范围

本文只回答一个问题:**`rsupervisorctl` 的命令行格式如何对齐 Python `supervisorctl`**。

**基准与参考**

- **权威基准**:Python Supervisor **4.2.5** 的 `supervisor/supervisorctl.py`(命令面以它为准)。
- **参考实现**:Go `ochinchina/supervisord` 的 `ctl.go`(commit `7a73369`)。
- **现状基线**:`rsupervisorctl`(`src/cli/args.rs`、`src/cli/commands.rs`、`src/cli/client.rs`)。

**两个兼容目标(务必分清,避免重复劳动)**

| 目标 | 含义 | 契约落点 | 本文是否覆盖 |
| :--- | :--- | :--- | :--- |
| **A — stock `supervisorctl` 直连** | 用户直接用 Python 的 `supervisorctl` 二进制连我们的 daemon | **XML-RPC 方法面 + `[supervisorctl]` 配置段**(见 `SUPERVISORD_COMPAT.md` #5) | 否(CLI 语法无关) |
| **B — `rsupervisorctl` 语法对齐** | 写给 `supervisorctl` 的脚本/肌肉记忆可直接用于 `rsupervisorctl` | 本文的命令、参数、输出、退出码 | **是** |

> A 才是真正的 drop-in 契约。B 成本低、值得做,但**不要借 B 去重造 A 已提供的能力**。

---

## 2. 优先级定义

| 级别 | 含义 | 判定标准 |
| :--- | :--- | :--- |
| **P0** | 兼容性破坏 / 脚本契约 | 不改则与 Python 语义冲突,或破坏已有脚本/自动化。必做。 |
| **P1** | 重要但低风险 | Python 常用命令,实现成本低、无冲突。 |
| **P2** | 次要 / 依赖搁置项 | 依赖 #3/#4 搁置项,或成本偏高、可降级。 |
| **不支持** | 明确不做 | **成本过高且 Go 版也未实现**;或有更优替代。 |

---

## 2.1 兼容定位:契约 vs 体验(总纲)

一条判断规则:

> **某个输出会不会被旧脚本用管道消费?** 会 → **契约面**,必须与 Python 逐字 / 逐字节 / 逐退出码对齐;
> 不会 → **体验面**,自由设计,且应刻意做得比 Python 现代。

立场:**兼容是为了迁就不愿意迁移的旧应用,只覆盖它们会踩的接口;不是为 Python 的 UX 当老好人。**
下文 §3–§7 每项均按此拆成两层标注。

### 契约面(旧脚本依赖,oracle 测试锁死)

| 面 | 项 | 为什么是契约 |
| :--- | :--- | :--- |
| 命令语法 + namespec | `stop mygroup:*`、`start all` | 脚本逐字调用 |
| LSB 退出码 | `status >/dev/null; [ $? -eq 3 ]` | 这套 CLI 价值最高的兼容点 |
| `status` 非 TTY 纯文本 | `status \| grep RUNNING` | 模板 `%(namespec)-33s%(state)-10s%(desc)s`(见 §4.2) |
| `pid` 纯数字输出 | `pid=$(supervisorctl pid web)` | 数值直取 |
| `tail` / `maintail` 字节上界 | `tail -100 web \| wc -c` | 字节语义 |
| `reread` / `update` 行 | `reread \| grep available` | `name: available\|changed\|disappeared` |
| `start` / `stop` / `restart` 结果行 | `\| grep -q started` | `namespec: started\|stopped` 与 `ERROR (...)`,在 stdout |
| `avail` 非 TTY 列 | `avail \| awk '{print $1}'` | 列位固定 |
| `-u/-p/-s/-c`(短 + 长名) | `supervisorctl -u a -p b status` | 脚本固定调用 |

### 体验面(自由 → 刻意现代)

| 面 | Python 现状 | rsupervisord 的做法 |
| :--- | :--- | :--- |
| `version` | 打印 daemon 版本 `4.2.5` | 打印自身身份 `rsupervisorctl <ver> (protocol supervisor 4.2.5)`;保留兼容标记,主体是"自己" |
| `help` / `--help` | flat 命令列表 | clap 渲染:分组、示例、别名标注 |
| 错误细节 | 混在 stdout 结果行里 | stdout 只走契约行;细节进 **stderr**,结构化、带上下文 |
| `shutdown` / `reload` 文案 | `Shut down` / `Restarted supervisord` | 按自己的口气;退出码仍是契约 |
| TTY 下的表格 / 颜色 | 逐字固定 | 表格 + 状态着色 + 高亮(已有,保留) |

### 落地机关

1. **测试只钉契约面**:`test_cli.py`(stock `supervisorctl` 直连)与 §10 脚本化验收逐项断言
   退出码 / stdout 文本形状 / 字节上界;**体验面绝不入测试**——`version`/`help` 的文案、
   TTY 表格渲染、错误 prose 均不设断言,现代化有完全自由且不被测试冻结。
2. **契约面交给 oracle**:`test_cli.py`(stock `supervisorctl` 直连)与 §9 验收逐项断言,
   由 XML-RPC/#5 门控自动放行。

---

## 3. 总览

### 3.1 命令面

| 命令 | Python 4.2.5 | Go 参考 | rsupervisorctl 现状 | 优先级 | 定位 |
| :--- | :--- | :--- | :--- | :--- | :--- |
| `status` | ✅ 支持 namespec/`all` | ✅ | ⚠️ 有,但表格输出、无退出码 | **P0** | 契约(TTY 表格=体验) |
| `help` | ✅ | ❌ | ❌ | **P0** | 体验 |
| `version` | ✅ | ⚠️ 顶层 `version` | ❌ | **P0** | 体验(语法需存在) |
| `pid` | ✅ | ✅ | ❌ | **P0** | 契约 |
| `shutdown` | ✅ | ✅ | ❌ | **P0** | 契约(退出码)/文案体验 |
| `reload` | ✅(重启 daemon) | ✅(同 Python) | ⚠️ 现为热重载,待改名 | **P0** | 契约(语义:重启 daemon) |
| `reload-config` | ❌(扩展) | ❌ | ✅(现 `reload` 改名) | 保留扩展 | 扩展(热重载) |
| `reread` | ✅ | ✅ | ❌ | **P0** | 契约 |
| `update` | ✅ | ✅ | ⚠️ 能力在 `reload` 里 | **P0** | 契约 |
| `start` / `stop` / `restart` | ✅ namespec/`all` | ✅ | ⚠️ 有,namespec 语义待对齐 | **P0** | 契约 |
| `tail` | ✅ `[-f\|-N] <name> [stdout\|stderr]` | ⚠️ 无 `-N` | ⚠️ 形态不同(`-n` 行、无 channel) | **P0** | 契约 |
| `signal` | ✅ | ✅ | ❌ | **P1** | 契约 |
| `avail` | ✅ | ❌ | ❌ | **P1** | 契约(非 TTY 列) |
| `open` | ✅ | ❌ | ❌ | **P1** | 体验 |
| `maintail` | ✅ | ❌ | ❌ | **P1** | 契约 |
| `clear` | ✅ | ✅ | ❌ | **P2** | 契约 |
| `add` / `remove` | ✅ | ✅ | ❌ | **P2**(依赖 #3/#4) | 契约 |
| `fg` | ✅ | ✅(简化实现) | ❌ | **P2**(参考 Go) | 扩展(非真 PTY,Go 简化形态) |
| `quit` / `exit` / `^D` | ✅ | ❌ | ❌ | **不支持**(随交互 shell) | 不支持 |
| (交互 shell) | ✅ | ❌ | ❌ | **不支持** | 不支持 |
| `events` | ❌(扩展) | ✅ | ✅ | 保留扩展 | 扩展 |
| `stdin` | ❌(扩展) | ❌(仅 XML-RPC) | ✅ | 保留扩展 | 扩展 |

### 3.2 客户端参数

| 参数 | Python 4.2.5 | Go 参考 | rsupervisorctl 现状 | 优先级 |
| :--- | :--- | :--- | :--- | :--- |
| `-c/--configuration` | ✅ | ❌(自动探测) | ⚠️ `-c/--config` | **P0**(加别名) |
| `-s/--serverurl` | ✅ | ✅ | ⚠️ `-s/--server` | **P0**(加别名) |
| `-u/--username` | ✅ | ⚠️ `-u/--user` | ⚠️ `-u/--user` | **P0**(加别名) |
| `-p/--password` | ✅ | ⚠️ `-P/--password` | ❌ `-P/--password` | **P0**(改 `-p`) |
| `-k/--key` | ❌ | ❌ | ✅ | 保留扩展 |
| `--allow-unelevated` | ❌ | ❌ | ✅ | 保留扩展 |
| `-i/--interactive` | ✅ | ❌ | ❌ | **不支持** |
| `-r/--history-file` | ✅ | ❌ | ❌ | **不支持** |

---

## 4. 全局行为(跨命令)

### 4.1 退出码(P0)

**需求**:采用 Python 的 LSB 退出码,使 shell 脚本可用 `$?` 判定。

| 码 | 含义 | 触发示例 |
| :--- | :--- | :--- |
| 0 | SUCCESS | 命令成功 |
| 1 | GENERIC | 一般错误、缺参(stop/restart/signal/clear/tail) |
| 2 | INVALID_ARGS | `start` 缺进程名;未知命令 |
| 3 | NOT_RUNNING(init 层为 UNIMPLEMENTED_FEATURE) | `status` 有任一进程处于 STOPPED 态 |
| 4 | UNKNOWN(init 层为 INSUFFICIENT_PRIVILEGES) | `status` 指定了不存在的进程;upcheck 失败 |
| 5 | NOT_INSTALLED | API 版本不匹配 |
| 7 | NOT_RUNNING | 对已停止/死进程执行 start/stop/pid |

**示例**

```bash
rsupervisorctl status web >/dev/null; echo $?   # web 未运行 -> 3
rsupervisorctl status nosuch; echo $?           # 不存在   -> 4
rsupervisorctl start nosuch; echo $?            # 不存在   -> 7 (死进程类)
rsupervisorctl start; echo $?                   # 缺参     -> 2
```

> 现状:`rsupervisorctl` 仅返回 0/1(anyhow 错误)。需在 `src/cli/commands.rs` 统一映射退出码。

### 4.2 输出格式(P0)

**需求**:`status` 在**非 TTY**(管道/重定向)时输出 Python 兼容的纯文本,便于脚本解析;TTY 下可保留现有彩色表格。

Python 模板:`'%(namespec)-33s%(state)-10s%(desc)s'`

**示例**

```
web                          RUNNING   pid 1234, uptime 0:00:10
db                           STOPPED   Not started
```

> 现状:`src/cli/commands.rs:60` 使用 `tabled` 圆角表格。建议:`std::io::stdout().is_terminal()` 为 false 时走纯文本分支。

### 4.3 namespec 与 `all`(P0)

**需求**:统一支持 Python 的三种名字形式:

| 形式 | 语义 | 示例 |
| :--- | :--- | :--- |
| `name` | 单进程(无冒号时 group==name) | `start web` |
| `group:process` | 组内指定进程 | `start mygroup:worker` |
| `group:*` | 组内全部进程 | `stop mygroup:*` |
| `all` | 全部进程 | `restart all` |

**示例**

```bash
rsupervisorctl status mygroup:*
rsupervisorctl stop mygroup:*
rsupervisorctl start all
```

> 现状:`client.rs` 已处理 `all` 与 `group:*`(client.rs:120/148/150),但 **bare `name` 当 group 的语义、`status` 的 `group:*` 过滤、不存在名字的错误文本/退出码**未对齐。

### 4.4 交互 shell(不支持)

**需求**:Python 无参数或 `-i` 时进入 REPL(`supervisor> ` 提示符、启动即 `status`、tab 补全、`quit`/`exit`/`^D`、交互模式恒返回 0)。

**决策**:**不支持**。理由:

- Go 版(`ctl.go`)为子命令式,**同样未实现**交互 shell。
- 成本高(需引入 `rustyline`、补全、历史、命令分发),而一次性子命令已覆盖全部自动化/脚本场景。

**替代**:所有操作以一次性子命令提供;`quit`/`exit`/`-i`/`-r` 一并归入本项不支持。

---

## 5. 客户端参数需求〔契约面〕

> 参数是旧脚本的固定调用点,均属**契约面**;`§5.2` 的 rsupervisord 独有扩展除外。

### 5.1 P0

#### 5.1.1 `-p/--password`(短选项修正)

**需求**:密码短选项改为 `-p`(Python 约定);`-P` 保留为兼容别名。当前 `-P` 与 Python 冲突。

```bash
# Python 写法(必须可用)
rsupervisorctl -u admin -p secret status
# 旧写法仍可用(别名)
rsupervisorctl -u admin -P secret status
```

#### 5.1.2 `-s/--serverurl`

**需求**:新增长名 `--serverurl`(保留 `-s`/`--server`)。值支持 `http://` 与 `unix://`;缺省 `http://localhost:9001`。

```bash
rsupervisorctl --serverurl http://127.0.0.1:9001 status
rsupervisorctl --serverurl unix:///run/rsupervisord.sock status
```

#### 5.1.3 `-u/--username`

**需求**:新增长名 `--username`(保留 `-u`/`--user`)。

```bash
rsupervisorctl -u admin -p secret status
rsupervisorctl --username admin --password secret status
```

#### 5.1.4 `-c/--configuration`

**需求**:新增长名 `--configuration`(保留 `-c`/`--config`)。当 #2(INI)落地后,应能从 `[supervisorctl]` 段读取 `serverurl`/`username`/`password` 作为缺省。

```bash
rsupervisorctl -c /etc/supervisord.conf status
rsupervisorctl --configuration /etc/supervisord.conf status
```

### 5.2 保留扩展

`-k/--key`(Bearer token)与 `--allow-unelevated` 为 rsupervisord 独有,保留,不与 Python 冲突。

---

## 6. 命令需求(逐项)

> 约定:每条给出 **语法 / 语义需求 / 示例 / 现状差距**。

### 6.1 P0

#### 6.1.1 `help`〔体验面〕

**语法**:`help [action]`
**语义**:无参列出全部动作;带参打印该动作帮助。
**示例**

```bash
rsupervisorctl help
rsupervisorctl help start
```

**定位**:体验面(见 §2.1)。**不逐字对齐** Python 的 flat 列表,用 `clap` 现代渲染:分组、示例、别名标注。
**现状**:无。可用 `clap` 帮助文本转接实现。

#### 6.1.2 `version`〔体验面〕

**语法**:`version`
**语义**(**不对齐 Python 输出**):打印 rsupervisord 自身身份;附一行兼容协议标记。不再复读 `4.2.5`。
**示例**

```bash
rsupervisorctl version
# rsupervisorctl 0.6.0 (protocol supervisor 4.2.5)
```

**定位**:体验面(语法需要存在即可,格式自由;见 §2.1)。
**现状**:无(有 `--version` 但那是客户端自身版本,语义不同——本次把 `version` 的语义修正为"打印自身身份",`--version` 归并/保持)。

#### 6.1.3 `pid`〔契约面〕

**语法**:`pid [name…]` / `pid all`
**语义**:无参=daemon PID;`all`=每个子进程一行;指定名=该进程 PID;PID==0 时退出码 7。输出**仅数字/行**,可被 `pid=$(...)` 消费。
**定位**:契约面。
**示例**

```bash
rsupervisorctl pid          # -> 4321
rsupervisorctl pid web      # -> 4567
rsupervisorctl pid all
```

**现状**:无。

#### 6.1.4 `shutdown`〔契约面·文案体验〕

**语法**:`shutdown`
**语义**:关闭远端 daemon。接受参数时报错(退出码 1)。交互模式需确认;非交互直接执行。
**定位**:退出码为契约;输出文案 `Shut down` 属体验面,可按自己的口气(C中一致即可)。
**示例**

```bash
rsupervisorctl shutdown
```

**现状**:无。

#### 6.1.5 `reload`(语义裁决)〔契约面〕

**语法**:`reload`
**语义(对齐 Python)**:重启远端 daemon(停全部→重读配置→再启动);接受参数报错。
**定位**:契约面——`reload` 的**语义**必须与 Python 一致(重启 daemon);输出文案属体验面。
**输出**:契约行 `Restarted supervisord`(单一、稳定);daemon 实现见 §9 相关的重启(main loop)设计。

> **⚠️ 高危冲突**:当前 `rsupervisorctl reload` 是**热重载配置**(不重启 daemon),与 Python 相反。Python 中:
> - `reread` = 仅重读配置、**不增删**
> - `update` = 重读 + 增删 + 重启受影响组
> - `reload` = **重启 daemon**

**示例**

```bash
rsupervisorctl reload        # 重启 daemon
```

**现状差距**:需把现有热重载能力改名 **`reload-config`**(扩展命令,见 §6.4),并把 `reload` 改为重启 daemon。daemon 侧两条路分离:热重载仍走 `/api/v1/reload`,`reload` 走新重启(main loop)语义。

#### 6.1.6 `reread`〔契约面〕

**语法**:`reread`
**语义**:重读配置,**不增删进程**。输出变更清单:每行 `name: available|changed|disappeared`,无变更输出 `No config updates to processes`。
**定位**:契约面(行可被 `reread \| grep available` 消费)。
**示例**

```
$ rsupervisorctl reread
web: changed
api: available
```

**现状**:无(能力部分在现 `reload` 里)。

#### 6.1.7 `update`〔契约面〕

**语法**:`update [gname…]` / `update all`
**语义**:重读配置 + 增删 + 重启受影响组。输出:`gname: stopped` / `gname: removed process group` / `gname: updated process group` / `gname: added process group`。
**定位**:契约面(行可被脚本消费)。
**示例**

```bash
rsupervisorctl update
rsupervisorctl update mygroup
rsupervisorctl update all
```

**现状**:现有 `reload` 能力即此项,建议改名迁移。

#### 6.1.8 `status`(改造)〔契约面〕

**语法**:`status [name…|gname:*|all]`
**语义**:无参或 `all`=全部;支持 `group:*` 与多名字;不存在名字输出 `X: ERROR (no such group|process)` 并置退出码 4;任一进程 STOPPED 置退出码 3。
**定位**:契约面——**非 TTY** 下必须输出 Python 模板 `%(namespec)-33s %(state)-10s %(desc)s` 纯文本并可被 `grep`/`awk` 消费;**TTY** 输出现有表格+着色属体验面,自由。
**示例**

```bash
rsupervisorctl status
rsupervisorctl status web api
rsupervisorctl status mygroup:*
```

**现状差距**:输出为表格(见 §4.2);缺退出码;`group:*` 过滤待核对。

#### 6.1.9 `start` / `stop` / `restart`(对齐)〔契约面〕

**语法**

- `start <name…>` / `start all` / `start gname:*`
- `stop <name…>` / `stop all` / `stop gname:*`
- `restart <name…>` / `restart all` / `restart gname:*`

**定位**:契约面——结果行 `namespec: started|stopped` 与错误行 `namespec: ERROR (…)` **在 stdout**,可被 `\| grep -q started` 消费。
**语义**:支持 namespec 与 `all`;`restart` = stop+start,**不重读配置**;`start` 缺参退出码 2,其余缺参退出码 1;死进程类错误退出码 7;输出 `namespec: started|stopped`,错误 `namespec: ERROR (…)`。

**示例**

```bash
rsupervisorctl start web api
rsupervisorctl stop mygroup:*
rsupervisorctl restart all
```

**现状差距**:已有 `-a/--async`、`-t/--timeout`(**Python 无此参数,保留为扩展**);需对齐 namespec/`all`/退出码/输出文本。

#### 6.1.10 `tail`(改造)〔契约面〕

**语法**:`tail [-f|-N] <name> [stdout|stderr]`
**语义**:默认 `stdout`;默认取末尾 **1600 字节**;`-f` 持续跟随;`-N` 取末尾 N 字节。
**定位**:契约面——字节上界可被 `tail -100 web \| wc -c` 校验。

**示例**

```bash
rsupervisorctl tail web            # 末尾 1600 字节 stdout
rsupervisorctl tail web stderr
rsupervisorctl tail -100 web       # 末尾 100 字节
rsupervisorctl tail -f web         # 持续跟随
```

**现状差距**:现为 `tail <name> [-f] [-n lines]`(行数、无 channel)。需:

- 新增 `stdout|stderr` 位置参数;
- 支持 `-N` 字节修饰;
- 字节语义按 `SUPERVISORD_COMPAT.md` #8 的**降级实现**(行级 ring buffer 近似);
- `-n` 保留为扩展别名。

### 6.2 P1

#### 6.2.1 `signal`〔契约面〕

**语法**:`signal <sig> <name…>` / `signal <sig> all` / `signal <sig> gname:*`
**语义**:发送信号;需 ≥2 参;输出 `namespec: signalled`。
**定位**:契约面(结果行随 start/stop/restart 规则)。
**示例**

```bash
rsupervisorctl signal HUP nginx
rsupervisorctl signal TERM all
```

**现状**:无。依赖 daemon 的信号能力。

#### 6.2.2 `avail`〔契约面〕

**语法**:`avail`
**语义**:列出全部已配置进程;模板 `'%(name)-32s %(inuse)-9s %(autostart)-9s %(priority)s'`,`inuse`=in use/avail,`autostart`=auto/manual,`priority`=`group_prio:process_prio`。
**定位**:契约面——**非 TTY** 列位固定,可被 `awk '{print $1}'` 消费;TTY 高亮属体验面。
**示例**

```
web                              in use    auto      999:999
api                              avail     manual    999:999
```

**现状**:无(有 `/api/v1/status` 可复用)。

#### 6.2.3 `open`〔体验面〕

**语法**:`open <url>`
**语义**:切换当前会话的 serverurl,仅接受 `http://` 或 `unix://`。
**定位**:体验面(会话级操作,无脚本消费场景;语法对齐即可)。
**示例**

```bash
rsupervisorctl open unix:///run/rsupervisord.sock
```

**现状**:无(廉价)。

#### 6.2.4 `maintail`〔契约面〕

**语法**:`maintail [-f|-N]`
**语义**:tail **daemon 自身**日志;默认 1600 字节。
**定位**:契约面——字节上界与 `tail` 同规则。
**示例**

```bash
rsupervisorctl maintail
rsupervisorctl maintail -f
```

**现状**:无。依赖 daemon 主日志可读(挂 `SUPERVISORD_COMPAT.md` #8/#12)。

### 6.3 P2

#### 6.3.1 `clear`〔契约面〕

**语法**:`clear <name…>` / `clear all`
**语义**:清空进程日志;输出 `namespec: cleared`。
**定位**:契约面(结果行随 start/stop/restart 规则)。
**示例**

```bash
rsupervisorctl clear web
rsupervisorctl clear all
```

**现状**:无。需日志清理能力(截断文件 + 清空 ring buffer)。

#### 6.3.2 `add` / `remove`〔契约面〕

**语法**:`add <name…>` / `remove <name…>`
**语义**:运行时激活/移除配置中的组;`remove` 对仍在运行的组报错。
**定位**:契约面(命令语法/退出码;结果行随通用规则)。
**示例**

```bash
rsupervisorctl add newgroup
rsupervisorctl remove oldgroup
```

**现状**:无。**依赖 #3/#4(已落地)**,转为 N/A——daemon 侧 `addProcessGroup`/`removeProcessGroup` 与 XML-RPC 已实现(见 `XMLRPC_COMPAT.md`),剩 CLI 接线。

#### 6.3.3 `fg`〔扩展〕

**语法**:`fg <name>`
**语义**:前台接管:跟随 stdout+stderr,并把终端输入转发到进程 stdin。
**示例**

```bash
rsupervisorctl fg web
```

**现状**:无。Go 版以"双 logtail + stdin 循环"简化实现(非真 PTY)。建议对齐 Go 的简化版,不做终端原始模式。依赖 `sendProcessStdin`(#7)。

### 6.4 扩展命令(非 Python,保留)

| 命令 | 说明 |
| :--- | :--- |
| `events` | SSE 实时系统事件流(rsupervisord 独有) |
| `stdin <name> <chars>` | 向进程 stdin 注入(Python 仅 XML-RPC 暴露,无 CLI) |
| `reload-config` | 零停机热重载(原 `reload` 改名;`reload` 归位为"重启 daemon"的 Python 语义) |

### 6.5 不支持

| 项 | 原因 |
| :--- | :--- |
| 交互 shell(REPL) | 成本高且 Go 版未实现;一次性子命令已覆盖脚本场景 |
| `-i/--interactive` | 同上 |
| `-r/--history-file` | 同上(readline 历史,仅交互 shell 有意义) |
| `quit` / `exit` / `^D` | 同上(仅交互 shell 有意义) |

---

## 7. 验收

**脚本化验收(退出码 + 纯文本)**

```bash
set -e
rsupervisorctl status >/dev/null || [ $? -eq 3 ]     # 有停止进程
rsupervisorctl start all
rsupervisorctl status | grep -q RUNNING
rsupervisorctl tail -100 web | wc -c                  # 不超过 100 字节
rsupervisorctl signal HUP web
rsupervisorctl pid web
rsupervisorctl update
rsupervisorctl reread
rsupervisorctl reload                                  # 重启 daemon
rsupervisorctl shutdown
```

**参数兼容验收**

```bash
rsupervisorctl -u admin -p secret status              # -p 可用
rsupervisorctl --serverurl http://127.0.0.1:9001 status
rsupervisorctl --configuration /etc/supervisord.conf status
```

---

## 8. 与其它文档的关系

- 服务端 XML-RPC 方法面(目标 A 的真契约):见 `SUPERVISORD_COMPAT.md` §7 #5。
- `tail` 字节偏移的降级实现:见 `SUPERVISORD_COMPAT.md` §7 #8。
- `sendProcessStdin`(数据面,`fg`/`stdin` 依赖):见 `SUPERVISORD_COMPAT.md` §7 #7。

---

## 9. 兼容测试基线

两个兼容目标分别对应两套可执行基准(见 [`../compat/README.md`](../compat/README.md)):

- **目标 B(native 语法对齐)**:[`../compat/tests/test_native_cli.py`](../compat/tests/test_native_cli.py) 对编译出的 `rsupervisorctl` 做**契约面**冒烟验证(`status` / `start` / `stop` / `restart` / `tail` / `stdin` / `reload-config`;退出码 + 状态断言,**不断言 UX 文案**),默认靶标下 **5 passed**(CLI 改名落地前 `reload-config` 一例 `skip`,落地后自动回归 5 passed)。
- **目标 A(未改动的 stock `supervisorctl` 直连)**:[`../compat/tests/test_cli.py`](../compat/tests/test_cli.py) 的 27 例为 oracle,先在 Python 4.2.5 上 **27 passed**;对编译 bin 因 `/RPC2` 未实现而统一 `xfail`(同 [`XMLRPC_COMPAT.md`](./XMLRPC_COMPAT.md) §12)。

即:§6/§7 列出的 P0/P1 需求,一旦服务端具备 XML-RPC(§7 #5),上述 oracle 会**自动**由 `xfail` 转为逐项断言。

---

## 10. 契约 vs 体验 — 测试分工

契约面与体验面在测试中的待遇**截然不同**:

| 层 | 测试策略 | 断言点 |
| :--- | :--- | :--- |
| **契约面** | **严格硬化**(oracle/§7 脚本化验收) | 退出码、stdout 文本形状、字节上界、命令语法 |
| **体验面** | **不设断言、也设防"被测试"** | `version`/`help` 文案、TTY 表格、错误 prose、stderr 细节——**严禁进入验证脚本** |

要点:

- `test_cli.py`(stock `supervisorctl` 直连)是契约面的权威 oracle;`test_native_cli.py` 只做
  **契约面**的 native 冒烟(`status`/`start`/`stop`/`restart`/`tail`/`pid`/`reread`/`update`…),
  且每条只断言退出码与 stdout 可机器消费的形式,**不断言任何 UX 文案**。
- 现代化是自由选择:**不许用测试把"现代"钉死**(例如不写"`version` 不得含 `4.2.5`"),
  也不许用测试把"Python 原样"钉死。契约不倒退、现代化不冻结。
- 契约面的每一条断言都源自 §2.1 / §6 / §7;体验面在验证脚本中**没有**对应条目。
