# 服务端 musl 静态构建（单文件、无 loader）

`agent2api-server` 目前只有 glibc 动态链接产物。本文记录怎么额外出一份 **aarch64 / x86_64 的 musl 静态包**，以及为什么值得出。

## 1. 为什么

现有 `agent2api-server-linux-arm64.tar.gz` 在常见服务器发行版上跑得好好的，但在三类环境里会卡住：

| 环境 | 症状 | 根因 |
|---|---|---|
| Alpine / distroless / 精简容器 | `no such file or directory`（明明是 ELF） | 缺 `/lib/ld-linux-aarch64.so.1`，内核找不到 PT_INTERP 指的解释器 |
| Android（Magisk / KernelSU 模块） | 起不来，或起来后所有请求 `dns error` | 同上；且 glibc 解析器硬读 `/etc/resolv.conf`，Android 上那个文件不存在或有主 |
| 宿主 glibc 比构建机旧 | `GLIBC_2.xx not found` | 动态链接的固有约束 |

musl 静态构建一次性消掉这三个：二进制没有 PT_INTERP，不需要 loader，不链接宿主 libc，单文件约 14MB。

代价与已知差异：

- **没有 NSS 模块**（`libnss_dns`/`libnss_files` 那套），也没有 `/etc/nsswitch.conf`。musl 自己的解析器直接读 `/etc/resolv.conf`，`hosts` 走内置表。本项目只用 `getaddrinfo` 做 A/AAAA 查询，不受影响。
- **体积**：静态链接把 musl 编进去，14MB 上下（glibc 动态版约 9MB）。
- **musl 的 AAAA 行为**：`AF_UNSPEC` 下它会同时查 A 与 AAAA。某些网络环境会拦 AAAA（实测某台 Android 内核会把 AAAA 查询的 `sendto` 直接判 `EPERM`），但 AAAA 与 A 同包时不受影响 —— 这点比 glibc 好，glibc 是分开发、AAAA 被拦就整次解析失败。

## 2. 怎么编

### 2.1 CI（推荐）

`.github/workflows/build-musl-server.yml` 已写好，只有 `workflow_dispatch`：

```
Actions → build-musl-server → Run workflow
```

产物是 `agent2api-server-linux-{arm64,amd64}-musl.tar.gz` + `.sha256`。
这份工作流刻意不挂 tag 触发 —— 推 tag 仍然只跑 `build.yml` 那个十几秒的派发 job，不会凭空开构建机。等配方稳定后搬去构建仓库、改成跟着 `repository_dispatch` 跑即可。

### 2.2 本机交叉编译（Windows 上也行）

需要：Rust 1.88+、zig 0.13、`cargo-zigbuild`。

```bash
rustup target add aarch64-unknown-linux-musl
cargo install --locked cargo-zigbuild
cd desktop-tauri/src-tauri
cargo zigbuild --release -p agent2api-server --target aarch64-unknown-linux-musl
# 产物：target/aarch64-unknown-linux-musl/release/agent2api-server
```

Windows 上随手编的话有两个坑：

1. 用 `stable-x86_64-pc-windows-gnu` 工具链，别用 msvc —— msvc 的构建脚本会去找 `link.exe`，容易被 MSYS 自带的同名程序抢走，报 `0xC0000022`；
2. 如果宿主 PATH 里没有 zig，`cargo-zigbuild` 会找不到 C 编译器，报 `failed to find tool "aarch64-linux-musl-gcc"`。这时给 zig 做个转发 shim（`zig cc -target aarch64-linux-musl` / `zig ar`），再通过 `CC_aarch64_unknown_linux_musl` / `AR_aarch64_unknown_linux_musl` 指过去；同时要加 `-C link-self-contained=no`，否则 Rust 自带的 `crt1.o` 与 zig 的 musl CRT 会撞 `duplicate symbol: _start`。

## 3. 验证

```bash
file agent2api-server
# → ELF 64-bit LSB executable, ARM aarch64, statically linked, stripped

readelf -l agent2api-server | grep -c INTERP   # 必须是 0
./agent2api-server --version
```

容器里验证时直接把二进制丢进 `scratch` 镜像即可（不需要 `/lib`、不需要 `/etc`）。

## 4. 给需要改解析器路径的场景留个后门

musl 的 `/etc/resolv.conf` 路径是编译期常量。像 Android 模块那种「不能往 `/etc` 写、也不想挂载」的场景，目前的做法是对二进制里 `/etc/resolv.conf`（16 字节）做**等长改写**成自己的私有路径（同样 16 字节）。等长改写不会破坏 ELF 布局，改完直接能跑。

如果以后想让这个更干净，可以给服务端加一个 `AGENT2API_RESOLV_CONF` 环境变量：在启动最早处判断，非空时把内容拷到临时文件并把解析器指过去。这需要改代码，另开 PR 更合适 —— 本文只记录现状。
