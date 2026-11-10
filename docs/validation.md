# zmime 0.1.0：开发交付与验证记录

记录日期：2026-11-10（Asia/Taipei）。正式性能测量使用干净提交 `53ef4d5f42e5ab3017a9541c38872d02f26ad87f`。代码验证使用 `37a8f837538f2073f1a07e07fb74b92dbda8907c`，两者仅有安全说明文档差异，解析实现、测试和 Cargo 文件完全相同。随后的提交补充结果与交付文档。

## 结论与当前状态

已实现一个零第三方依赖、仅使用 `core`、支持调用方工作区的 `#![no_std]` MIME 解析库。RFC 5322 原始头字段、折叠处理、MIME 元数据、嵌套结构、边界扫描、延迟 transfer decoding、基础 RFC 2047 和正文切片接口均有实现和测试。crate 的 MSRV 是 Rust 1.74.0，已使用该版本实际编译并运行测试。

性能目标未全面达成。本次 x86_64 等价结构对照中，极短邮件为 mail-parser 1.0.1 的 3.339 倍，普通邮件为 1.440 倍；八个 CRLF 结构场景中只有一个达到 2 倍。大附件、深层 multipart、长头部、大量附件，以及 Base64 和密集转义 QP 解码仍有性能差距。报告仅采用完整参数消费协议的本轮数据，不提供 full 模式性能结论。

本地开发、测试、基准、版本提交和发布自动化实现已经完成。GitHub 推送和远端 Actions 尚未完成：连接的账户下未发现 `LIghtJUNction/zmime`，仓库查询返回 404；当前 GitHub 连接没有创建仓库的能力，本地 Git 没有可用认证，实际 push 返回无法读取用户名。浏览器也处于 GitHub 未登录状态。代码与提交历史随交付包提供。crates.io 已完成 `cargo publish --dry-run`，没有执行正式发布。

## 使用方式与所有权

解析器接受原始字节，借用它们并把节点记录写进调用方提供的数组。正文和附件仍指向原邮件中的切片。只有显式调用解码方法时，编码内容才会被转换到调用方提供的输出缓冲区。这使“列出附件”不必先读取和转换所有附件内容。

```rust
use zmime::{Limits, Message, Part};

let input = b"Content-Type: text/plain; charset=utf-8\r\n\
Content-Transfer-Encoding: base64\r\n\r\nSGVsbG8h";
let mut parts = [Part::EMPTY; 8];
let message = Message::parse(input, &mut parts, Limits::default()).unwrap();
assert_eq!(message.root().raw_body(), b"SGVsbG8h");

let mut output = [0u8; 32];
assert_eq!(message.root().text(&mut output).unwrap(), "Hello!");
```

需要原始头字段时使用 `HeaderBlock::parse`；需要头字段及 Content-Type、Content-Disposition、Content-Transfer-Encoding 时使用 `Part::parse_headers`；需要完整 MIME 树时使用 `Message::parse`。`Part::parent` 和 `Message::children` 提供树关系，`raw_body` 保留编码字节，`decode_body` 只执行 transfer decoding，`text` 再进行受支持的字符集转换。

可运行示例：`cargo run --example inspect`，覆盖 Subject 解码、MIME 树遍历和附件按需读取。

根 crate 不使用 `std`、`alloc` 或第三方依赖，没有 `build.rs`、C/FFI 或外部运行时。portable 库在编译期 `forbid(unsafe_code)`。测试程序可以使用标准库。具有参考库依赖的 `comparison/` 是单独 workspace，并被排除在发布包及核心依赖图之外。

## 功能边界

| 功能 | 已实现行为 |
| --- | --- |
| RFC 5322 头字段 | 保留字段名、原始值、普通重复字段；大小写无关查找；CRLF/LF；折叠展开 |
| MIME 元数据 | 类型、子类型、disposition、有序参数、注释、引号与转义；默认类型与 multipart/digest 规则 |
| MIME 树 | multipart、多层嵌套、identity 编码的 message/rfc822、前言/尾声、祖先边界识别、原始附件切片 |
| Base64 | MIME 空白、完整四元组、padding 与未使用位检查、明确的容量与非法输入错误 |
| Quoted-Printable | 十六进制转义、CRLF/LF 软换行、行尾空白规则、明确拒绝非法转义 |
| RFC 2047 | B/Q 编码词、相邻编码词空白规则；不把它扩展为地址或注释语法解析器 |
| 字符集 | UTF-8、US-ASCII、ISO-8859-1；其他标签返回 `UnsupportedCharset` |
| 延迟处理 | 结构解析不解码附件；未读取的非法 Base64 内容不会强迫树解析执行解码 |

原始内容访问与文本解释是不同接口。原始字节可被保留和读取，但这不表示支持它的字符集。UTF-8 会验证合法性，ASCII 会拒绝高位字节，Latin-1 会明确转换成 UTF-8；GB18030、Shift-JIS、Windows 代码页等不会被悄悄替换或误读。

RFC 2231 的扩展、连续参数保留原始属性，要求解码时返回 `Unsupported`。`text()` 遇到 `charset*=` 或 charset 连续声明明确返回 `UnsupportedCharset`，包括同时存在普通 charset 参数的情况。transfer 编码的 message/rfc822 需要先解析其头字段、把正文解码到另一块缓冲区，再解析解码结果；编码的 multipart 容器会被拒绝。缺失结束边界、重复 MIME 关键字段、相互混淆的嵌套 boundary 和截断结构返回错误。MIME 与注释错误偏移统一相对于调用公开 API 时的完整输入。附件迭代器匹配显式 `Content-Disposition: attachment`，其他部分仍可从完整节点数组读取。

## 有界资源与安全论证

解析使用迭代状态机和父节点索引，节点数量增加时消耗调用方工作区。父索引总是指向已经创建的节点，因此向根遍历能够终止。扫描向前推进，不以递归调用表达邮件深度；边界候选与当前祖先进行匹配，最坏工作量与输入长度乘以允许深度成正比。

| 限制 | 默认值 |
| --- | ---: |
| 输入总长度 | 64 MiB |
| 每部分头字段区 | 256 KiB |
| 每部分逻辑字段数 | 256 |
| 物理头行长度 | 64 KiB |
| MIME 节点总数 | 1,024 |
| 嵌套深度 | 32，根为 0 |
| 注释嵌套 | 16 |
| boundary 长度 | 70 字节 |

这些是资源上限，不代表所有 RFC 行长度都默认严格限制到 998 字节。应用可通过 `Limits` 降低可配置值，设置物理头行上限为 998。调用方数组和解码缓冲区形成额外上限；容量不足返回错误。解析失败时工作区可能已有部分记录，解码失败时输出可能已有前缀；只有成功的 `Result` 才能被作为完整结果使用。

portable 实现是安全 Rust。`unsafe` 集中在 CPU 检测和私有 SIMD 内核，每个访问点有边界及指令前提说明。x86_64 通过 CPU 能力和操作系统向量状态检查后选择 SSSE3/AVX2；aarch64 在目标保证 NEON 时使用对应实现。缓存只保存 CPU 能力位，不包含邮件数据或调用方指针。

内核按剩余输入和输出容量确定是否能够装载、写入完整向量。尾部、padding、跨折行的四元组和错误字节回到标量验证。精确输出存储避免为写一个向量而越过实际解码长度。Miri、标量对照、所有向量位置的非法字节和精确容量测试共同支持这些局部论证；有限测试不等于对任意输入的形式化证明。逐项审计说明位于 `docs/safety.md`。

## 实际完成的质量验证

| 检查 | 本轮已观察结果与范围 |
| --- | --- |
| 核心测试 | 64 项：3 scanner、16 adversarial、27 decode、8 headers、9 MIME、1 零分配测试 |
| MSRV | 实际 Rust 1.74.0：默认与 portable 各 64 项通过；2 个 README doctest 通过 |
| stable | Linux x86_64 默认与 portable 各 64 项通过；核心与 comparison fmt、核心两种特性 clippy、comparison clippy、rustdoc warnings-as-errors 通过 |
| 零分配 | 计数分配器检查解析、头字段展开、transfer decoding 和文本转换路径，观察到零堆分配 |
| 差分 | 默认与 portable 各 9 组通过；新增 96 个嵌套和 quoted 参数输入；正式计时前核对完整字节投影与计时消费者结果 |
| 生成与随机输入 | 300 个生成 multipart；6,000 个确定性随机输入和 1,200 个变异输入；编码往返、边界、注释、折叠、参数及精确容量用例 |
| portable Miri | strict provenance，完整 64 项全部通过 |
| scanner Miri | strict provenance，AVX2 与 aarch64 NEON 各 3 项通过 |
| Base64 / 文本 Miri | strict provenance，AVX2 与 SSSE3 各 7 项 Base64 和 5 项 body_text 通过；NEON 5 项 body_text 通过 |
| MSRV 跨目标构建 | aarch64 默认与 portable、WASM 默认、thumbv7em portable 均使用实际 Rust 1.74.0 通过编译 |
| 发布脚本 | 27 项离线回归通过，覆盖实际临时 Git 远端上的 patch/minor/major、重复执行和 registry/Release 恢复 |
| 基准汇总脚本 | 3 项通过；缺失配对、冲突元数据、非法计时值报错；非等价工作不生成速度比 |
| 工作流检查 | CI、Release、Benchmark 三个工作流均通过 actionlint |
| 包边界与发布预演 | `check_contract.py`、`cargo package --locked`、`cargo publish --locked --dry-run` 通过 |

Miri 通过目标指令特性配置选择要解释的 SIMD 后端，不在解释器中运行主机 CPUID/XGETBV。CPU 检测使用架构前提证明和原生执行验证。aarch64 的本地证据来自交叉编译和 Miri；没有把它描述为真实 ARM 硬件性能结果。

GitHub Actions 已配置 Linux、Windows、macOS、原生 ARM、MSRV、stable、格式、lint、文档、跨目标、Miri、差分、打包及发布预演。因为远端仓库尚不可用，本次没有对应的 hosted Actions 运行记录；Windows/macOS 原生测试和 ARM runner 的实际执行仍等待首次远端 CI。

## 对比协议与正式结果

参考库固定为 `mail-parser = "=1.0.1"`，传递依赖由 `comparison/Cargo.lock` 固定。参考项目需要 Rust 1.98+，不改变核心 crate 的 1.74 MSRV。参考解析器的非 MIME 字段配置为原始值，双方都处理 MIME 元数据并保留其余头字段字节；地址、日期语义解析及字符集转换不混入结构计时。

**本轮使用协议版本 2：参数读取与反转义都位于计时区内。** 主基准在计时前逐一比较所有原始字段、Content-Type/Disposition 参数、transfer encoding、父子关系和正文切片的完整字节投影，并检查计时消费者结果一致。源码、生成语料和二进制摘要随环境记录保留，运行中发生变化会停止测量。`headers` 处理根头字段和 MIME 元数据；`structure_reuse` 遍历完整受支持结构，双方复用存储并保持解码延迟；transfer 模式对相同输入解码出相同字节，双方预留输出空间。额外的 `structure_fresh_reference` 保留在 CSV 中，因为存储生命周期不同而不给出加速比。

字符集策略差异单独测试：mail-parser 1.0.1 把 ISO-8859-1 标签按 Windows-1252 解释；zmime 执行字面 ISO-8859-1 转换。该差异不计入结构或 transfer 测量。

测试输入有八类邮件，每类包含 CRLF 与 LF，共 16 个邮件 fixture；另有 16 个解码负载。非法和截断邮件通过独立策略测试覆盖，不把 zmime 的严格拒绝与参考库的容错恢复计算为相同工作量。

正式测量环境为 Intel Xeon Platinum 8573C、Linux 6.18.57、Rust 1.99.1，固定 CPU 8，薄 LTO、单 codegen unit，没有额外 `RUSTFLAGS`。每组预热 32 次，再用相同迭代次数测量双方；执行顺序交替，9 组样本，目标为较快实现的每组至少约 20 ms。default 和 portable 两次测量均记录相同的干净源码提交，每次保留 1,152 条计时样本及 54 次新进程首次解码观察。

下表单位是每次操作的微秒。比值为 `mail-parser 时间 / zmime 时间`，大于 1 表示 zmime 更快。headers 计入根头字段及所有 MIME 参数；structure 计入完整结构及所有元数据，保留附件延迟解码。

| CRLF 场景 | headers：zmime µs | headers：参考 µs | headers 比值 | structure：zmime µs | structure：参考 µs | structure 比值 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 极短邮件 | 0.082 | 0.206 | 2.511× | 0.087 | 0.290 | 3.339× |
| 普通邮件 | 0.618 | 0.795 | 1.288× | 0.612 | 0.881 | 1.440× |
| 1 MiB 附件 | 0.245 | 0.372 | 1.519× | 37.515 | 31.688 | 0.845× |
| 八层 multipart | 0.219 | 0.319 | 1.458× | 5.062 | 4.270 | 0.844× |
| 新闻邮件 | 0.205 | 0.344 | 1.677× | 2.836 | 2.725 | 0.961× |
| 约 70 KiB 折叠头部 | 3.624 | 3.331 | 0.919× | 3.697 | 3.394 | 0.918× |
| 128 个附件 | 0.218 | 0.354 | 1.626× | 78.187 | 60.094 | 0.769× |
| 嵌套 message/rfc822 | 0.222 | 0.364 | 1.638× | 0.766 | 0.974 | 1.272× |

1 MiB 解码输出目标的 transfer 对比：

| 编码输入 | zmime µs | mail-parser µs | 比值 |
| --- | ---: | ---: | ---: |
| 无折行 Base64 | 137.552 | 92.842 | 0.675× |
| MIME 折行 Base64 | 346.478 | 138.398 | 0.399× |
| 密集转义 QP | 2449.330 | 1771.520 | 0.723× |
| ASCII QP | 248.426 | 338.555 | 1.363× |

portable 对照保留参考库自身的优化，比较 zmime 开启与关闭 SIMD 后的变化。两轮参考时间也完整保留，便于检查共享环境波动。该消融描述 zmime 自身的内核作用。

主表是预热后的吞吐；冷启动数据单独记录新进程首次调用与该进程内预热循环，排除进程启动和缓冲区准备，不计算跨库冷启动加速比。

完整 headers、LF、transfer、消融与首次调用表位于 `comparison/RESULTS.md`。`results/2026-11-10-reviewed-default/` 和 `results/2026-11-10-reviewed-portable/` 各保留 raw.csv、summary.csv、cold.csv、environment.json，包含所有样本、中位数、最小/最大值和环境。本轮两种构建的源码与语料摘要相同，测量前后源码与二进制均未变。历史目录的参数消费协议不同，不参与本轮结论。

## 复现命令

以下命令从仓库根目录执行。基准项目另需 Rust 1.98+ 和 Python 3.9+。未安装对应 toolchain/target 时先使用 rustup 安装。

```sh
cargo +1.74.0 test --locked --all-targets
cargo +1.74.0 test --locked --all-targets --no-default-features
cargo +1.74.0 test --locked --doc
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo clippy --locked --all-targets --no-default-features -- -D warnings
RUSTDOCFLAGS='-D warnings' cargo doc --locked --no-deps --all-features
python3 scripts/check_contract.py
python3 -B -m unittest discover -s scripts -p 'test_*.py' -v
python3 -B -m unittest discover -s comparison -p 'test_*.py' -v
cargo test --locked --manifest-path comparison/Cargo.toml
cargo test --locked --manifest-path comparison/Cargo.toml --no-default-features
cargo package --locked
cargo publish --locked --dry-run

python3 comparison/run.py --output comparison/results/local --samples 9 --target-ms 20
python3 comparison/run.py --portable --output comparison/results/local-portable --samples 9 --target-ms 20
python3 comparison/report.py comparison/results/local comparison/results/local-portable > comparison/RESULTS.md
```

Linux 上可以加 `--cpu 8` 重现本次绑定；其他环境应选择它们实际允许的 CPU，runner 会记录绑定信息。Miri 的完整后端命令在 `docs/safety.md`。运行 benchmark 时应避免同时执行编译、测试或其他高负载任务。

## CI bump 与发布恢复

`Benchmark` workflow 可选 x86_64 / 原生 aarch64、SIMD / portable，自动保留相同验证协议的样本和环境 artifact。CI、发布和基准任务均设置执行超时。

`Release` workflow 提供 `workflow_dispatch`，仅允许从受信任的默认分支启动。它准备版本提交，再直接调用可复用的 CI workflow 检验指定 SHA，通过后才发布、创建 tag 和 GitHub Release。验证不依赖 `GITHUB_TOKEN` 创建的事件触发第二次工作流。

| bump 选项 | 从 0.1.0 出发 | 用途 |
| --- | --- | --- |
| `none` | 0.1.0 | 首次发布当前版本或恢复失败的发布 |
| `patch` | 0.1.1 | 修订版本 |
| `minor` | 0.2.0 | 次版本，patch 归零 |
| `major` | 1.0.0 | 主版本，minor/patch 归零 |

bump 同步根 `Cargo.toml`、根锁文件和比较项目锁文件中的 zmime 版本，自动提交并推送。发布任务只在 `cargo publish` 步骤取得 `CARGO_REGISTRY_TOKEN`，不把它作为命令行参数、测试输入或文件内容。

发布前验证默认分支来源、已测试 SHA、版本、tag 与 registry 状态。已经存在的 tag 必须指向相同提交；非 commit 对象或冲突会停止。已经存在的 crates.io 版本必须未 yanked，且归档 SHA-256 完全一致才可恢复使用。上传成功后等待 registry 确认，再创建 tag/Release；已有兼容 Release 保留，未完成 draft 可补齐。失败时不强推 tag，也不把 registry 网络错误当成版本未存在。

同一次 Actions run 重试不会重复 bump；新的恢复运行应选择 `none`。如果 crate 已上传而后续 tag/Release 失败，优先从原运行执行 Re-run failed jobs，保留原来的源码、版本与验证结果。完整状态及恢复说明在 `docs/release.md`，离线状态转移回归测试在 `scripts/test_release.py`。

远端启用还需要可写的目标仓库、允许版本提交/tag 的仓库规则，以及配置在仓库 Secrets 中的 `CARGO_REGISTRY_TOKEN`。token 不应粘贴到聊天或源码中。当前阻塞点是目标 GitHub 仓库与认证；配置完成后才有依据声称推送、远端 CI 或正式发布成功。

## 交付文件索引与依据

| 路径 | 内容 |
| --- | --- |
| `src/` | 解析、头字段、MIME 参数、错误、扫描、解码、CPU 检测 |
| `tests/` | RFC/边界/错误/随机与属性/零分配测试 |
| `comparison/` | 固定参考库、差分、生成语料、基准、原始结果及报告 |
| `.github/workflows/` | 跨平台 CI、受控版本 bump 与发布、手动架构基准 |
| `examples/inspect.rs` | 可运行的头字段、树遍历、正文与附件解码示例 |
| `scripts/` | 零依赖/包边界检查、发布状态机、发布回归测试 |
| `docs/safety.md` | 有界资源、逐项 unsafe 访问论证和 Miri 复现 |
| `docs/release.md` | 发布与恢复指南 |
| `LICENSE-MIT`、`LICENSE-APACHE` | MIT 或 Apache-2.0 双许可 |

规范范围依据 RFC 5322、2045、2046、2047、2183；RFC 2231 被明确限制为原始值保留。参考实现固定版本及其实际 API 是比较方法的一部分，不用旧版本额外执行的解码工作作为速度优势。

- [RFC 5322：Internet Message Format](https://www.rfc-editor.org/rfc/rfc5322)
- [RFC 2045：MIME 格式与 transfer encoding](https://www.rfc-editor.org/rfc/rfc2045)
- [RFC 2046：MIME 类型与 multipart](https://www.rfc-editor.org/rfc/rfc2046)
- [RFC 2047：头字段编码词](https://www.rfc-editor.org/rfc/rfc2047)
- [RFC 2183：Content-Disposition](https://www.rfc-editor.org/rfc/rfc2183)
- [RFC 2231：参数扩展](https://www.rfc-editor.org/rfc/rfc2231)
- [mail-parser 1.0.1 文档](https://docs.rs/mail-parser/1.0.1/mail_parser/)
- [Miri 文档](https://github.com/rust-lang/miri)

## 主张与证据复核

| 主张 | 证据 | 状态 |
| --- | --- | --- |
| 核心零依赖、仅 core、零堆分配 | Cargo metadata、no_std、嵌入式编译、分配计数 | 已验证 |
| 解析与解码受限于输入及工作区 | Limits、迭代树、容量与异常输入测试 | 已实现并验证 |
| SIMD 访问范围有明确边界 | 每处 unsafe 的范围说明、精确容量测试、标量对照、Miri | 已验证所列路径 |
| 性能表只计算相同工作 | 完整输出投影、计时内参数消费、配对与协议检查、原始 CSV | 已验证；代表性 2× 目标未全面达到 |
| 版本递增与发布恢复有实现 | 临时 Git 远端与 registry/Release 状态测试 | 本地通过，远端执行受仓库访问阻塞 |

复核覆盖功能贡献、接口与错误说明、实验强度、覆盖范围及设计边界五个方面。表中
性能主张直接对应本轮样本；原生平台执行状态、字符集边界、发布状态分别说明。
