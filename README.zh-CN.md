<p align="center">
  <img src="docs/assets/logo.png" alt="Porta" width="200">
</p>

<h1 align="center">Porta</h1>

<p align="center">
  无论是命令还是 AI 智能体，都只按你授予的权限运行。<br>
  由 macOS 和 Linux 内核强制执行。无需容器，无需守护进程。
</p>

<p align="center">
  <a href="README.md"><img alt="English" src="https://img.shields.io/badge/English-d0d7de?style=flat-square"></a>
  <a href="README.ja.md"><img alt="日本語" src="https://img.shields.io/badge/%E6%97%A5%E6%9C%AC%E8%AA%9E-d0d7de?style=flat-square"></a>
  <a href="README.zh-CN.md"><img alt="简体中文" src="https://img.shields.io/badge/%E7%AE%80%E4%BD%93%E4%B8%AD%E6%96%87-24292f?style=flat-square"></a>
</p>

---

```text
$ porta run sh -v ./work --no-net -- -c 'echo ok > out.txt; cat ~/.ssh/id_ed25519; echo x > ~/elsewhere.txt; curl https://example.com'
cat: /Users/me/.ssh/id_ed25519: Operation not permitted
sh: /Users/me/elsewhere.txt: Operation not permitted
curl: (6) Could not resolve host: example.com
[porta] the sandbox refused this run 3 times; what each would have needed:
  file-read-data /Users/me/.ssh/id_ed25519
    → closed by the preset or --deny-read; no mount reopens it
  file-write-create /Users/me/elsewhere.txt
    → -v /Users/me
  …
```

`out.txt` 写入成功；密钥、挂载之外的文件和网络都没有被触及。

Porta 包裹一个进程及其启动的所有子进程。写入仅限于你挂载的目录，凭据存储不可读，
网络收窄到你指定的端口或主机，CPU、内存、进程数和时间都有上限。它在 macOS 上由
Seatbelt 强制执行，在 Linux 上由 Landlock、seccomp 和命名空间强制执行——而不是一个
“礼貌请求”的包装器。内核无法表达的规则会直接拒绝运行，而不是悄悄放宽。

## 安装

```bash
curl -fsSL https://raw.githubusercontent.com/almide/porta/main/scripts/install.sh | bash
```

| 平台 | 方法 |
|---|---|
| macOS (Apple silicon)、Linux (x86-64, arm64) | 上面的脚本：校验 SHA-256，安装了 `cosign` 时还会校验 Sigstore 签名 |
| Debian、Ubuntu | 从 [release](https://github.com/almide/porta/releases) 下载后 `sudo apt install ./porta_<version>_amd64.deb`；在 Ubuntu 23.10+ 上还会加载命名空间所需的 AppArmor 配置 |
| 以其他方式安装的 Ubuntu | 运行一次 `sudo porta setup`，加载同样的配置 |
| GitHub Actions | `- uses: almide/porta@v0.6.16`，之后任意步骤中都可 `porta run …` |
| 其他 (Intel Mac 等) | `almide install github.com/almide/porta --branch main` 从源码构建 |

每个 release 都带有签名和构建来源证明——
[验证 release](docs/cli.md#verifying-a-release)。

## 约束你正在使用的编码智能体

```bash
cd my-project
porta init claude            # 或: porta init codex
porta up -- -p "Fix the failing test"
```

`porta init` 会写出一个带注释的 `porta.toml`，按该智能体实际所需量身定制：项目和智能体
自己的目录可写，其中的设置、钩子和全局指令不可写——这样一次会话无法为下一次会话埋下
东西——API 密钥或令牌按名称传入。macOS 上钥匙串是关闭的，所以登录凭据通过
`ANTHROPIC_API_KEY` 或 `CLAUDE_CODE_OAUTH_TOKEN`（`claude setup-token`）传入。

其他命令同样用参数即可：

```bash
porta run ./installer -v ./sandbox --allow-net '*:443' --timeout 120 --max-procs 500
```

`--` 之前是 porta 的参数，之后是命令的参数。

## 它阻止什么

| | macOS | Linux |
|---|---|---|
| **写入** | 仅 `-v` 挂载、`/tmp`、`/dev` | 相同 |
| **可写挂载内部** | `.git/hooks`、`.git/config`、`.envrc`、shell rc 文件、智能体设置等保持不可写，也无法被重命名移走 | 相同（需要用户命名空间——见下文） |
| **读取** | 除凭据存储外均可读：`~/.ssh`、`~/.aws`、`~/.config/gh`、云与镜像仓库令牌、钥匙串、浏览器配置均关闭；`--read-policy strict` 将读取也限制在挂载内 | 相同（钥匙串除外） |
| **凭据套接字** | SSH agent、gpg-agent、`docker.sock` 除非 `--allow-unix` 否则拒绝 | 相同 |
| **网络** | 开放，或按端口放行 TCP（`--allow-net`），或经 porta 代理按主机放行（`--proxy-allow`），或完全关闭（`--no-net`） | 相同；按端口放行时 UDP 关闭 |
| **其他进程** | 其参数和环境变量不可读；`open(1)` / Launch Services 关闭 | 不可见：独立的 PID 命名空间 |
| **资源** | `--timeout`、`--max-cpu`、`--max-procs`、`--max-file-size`、`--max-memory-mb` | 相同（内存通过 cgroup v2） |
| **环境变量** | 从空开始；只传递 `PATH`、`HOME`、区域设置和终端，其余仅限 `-e` / `--env-pass` 指定的 | 相同 |

关闭哪些内容由预设决定——[`native/presets/default.toml`](native/presets/default.toml)，
可用 `porta explain` 查看——而不是写死在代码里：`--deny-read`、`--protect`、
`--deny-unix` 在其上追加，`--preset <file>` 整体替换。附带理由的完整表格见
[enforcement](docs/enforcement.md)。

## 被拒绝时

```bash
porta explain ./tool -v ./work --allow-net '*:443'   # 用文字描述策略，不执行任何东西
porta run ./tool -v ./work --why                      # 运行后：每次拒绝及其所需的参数
```

```text
[porta] the sandbox refused this run 2 times; what each would have needed:
  file-write-data /home/me/notes/out.txt
    → -v /home/me/notes
  network-outbound remote:*:80
    → --allow-net '*:80'
  to run again with those granted:
    porta run ./tool -v ./work -v /home/me/notes --allow-net '*:80'
```

在 macOS 上，任何失败的运行之后都会从内核的拒绝日志中给出这些信息；在 Linux 上，
`--why` 在沙箱外用 `strace` 跟踪运行。

## 撤销它做的改动

```bash
porta run ./agent -v ./project --snapshot     # 先复制挂载目录；结束后列出改动
porta rollback --yes                          # 恢复原样
```

在 APFS 上是克隆，复制几乎零成本。快照存放在所有挂载之外，命令无法改写自己的撤销手段。

## 对比

同一套[逃逸用例](docs/benchmarks/escapes.md)——真实的逃逸手法，按实际突破的数量计分——
在五个工具下各自以默认配置运行（[完整对比](docs/benchmarks/competitors.md)，包括 porta
落后的项）：

| | porta | [srt](https://github.com/anthropics/sandbox-runtime) | [Fence](https://github.com/fencesandbox/fence) | [nono](https://github.com/nolabs-ai/nono) | [landrun](https://github.com/Zouuup/landrun) |
|---|---|---|---|---|---|
| macOS：尝试 / 逃逸 | **22 / 0** | 17 / 5 | 14 / 5 | 13 / 5 | — |
| Linux：尝试 / 逃逸 | **28 / 0** | 23 / 2 | 21 / 3 | 24 / 5 | 22 / 7 |

其他工具有而 porta 没有的：srt 和 Fence 默认关闭网络；Fence 按名称过滤命令；nono
可在运行中申请更多权限。porta 的开销：macOS 约 16 ms，Linux 2–6 ms
（[overhead](docs/benchmarks/overhead.md)）。

## 局限

- **网络默认开放。** 用 `--no-net`、`--allow-net` 或 `--proxy-allow` 关闭。
- **在 Linux 上，部分保护需要用户命名空间**——挂载内的钩子、凭据套接字、隐藏其他进程。
  Ubuntu 23.10+ 会限制它们，直到 `.deb` 或 `sudo porta setup` 加载配置；没有配置时
  porta 照常运行，并说明哪些仍然开放。
- **共享内核。** 这是进程沙箱，不是虚拟机：内核漏洞不在防护范围内。
  [威胁模型](docs/threat-model.md)列出了防护与不防护的内容。
- **由作者测试。** 逃逸用例、模糊测试和猴子测试均公开并在 CI 中运行；尚未经过第三方审计。

## 其他功能

- **无环境权限的 WASM。** `porta run plugin.wasm --profile worker` 运行核心模块或
  WASI 0.2/0.3 组件，不接触宿主文件系统和网络，fuel、内存和时间均有上限；每个导入都
  需要你授予的能力。
- **循环本身是 WASM 的智能体。** `porta agent agent.toml` 将决策循环和每个工具放在
  独立实例中运行，模型凭据留在宿主，崩溃的运行可恢复且不会重复已完成的写入
  （[agent runtime](docs/agent-runtime.md)）。
- **一个 API 加一份日志。** `--proxy-allow api.example.com --proxy-audit egress.jsonl`
  只放行该主机，并记录每一次决策。

## 文档

- [CLI 参考](docs/cli.md) — 所有命令、参数、`porta.toml` 键和退出码
- [Enforcement](docs/enforcement.md) — 各平台究竟阻止什么、如何阻止
- [威胁模型](docs/threat-model.md) — 防护什么，不防护什么
- [基准测试](docs/benchmarks/README.md) — 逃逸用例、对比、开销
- [架构](docs/architecture.md) — 策略由 Almide 决定，由 Rust 强制执行

基于 [Almide](https://github.com/almide/almide) 和 [Wasmtime](https://wasmtime.dev) 构建。Apache-2.0。
