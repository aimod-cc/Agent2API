#!/usr/bin/env python3
import argparse
from contextlib import closing, contextmanager
import hashlib
import json
import os
from pathlib import Path
import resource
import select
import shutil
import signal
import socket
import sqlite3
import struct
import subprocess
import sys
import tarfile
import tempfile
import time


CPUS = ("qemu64", "Nehalem", "IvyBridge", "Haswell-v2")


def command(args, **kwargs):
    return subprocess.run(args, check=True, timeout=120, **kwargs)


def require_elf(path):
    with path.open("rb") as source:
        header = source.read(20)
    if len(header) < 20 or header[:6] != b"\x7fELF\x02\x01" or struct.unpack_from("<H", header, 18)[0] != 62:
        raise RuntimeError(f"不是 Linux x86_64 可执行文件：{path}")


def extract_package(package, directory):
    if package.suffix == ".deb":
        command(["dpkg-deb", "-x", str(package), str(directory)])
        binary = directory / "usr/bin/agent2api"
        return binary, None, True
    if package.suffix == ".AppImage":
        require_elf(package)
        executable = directory / package.name
        shutil.copy2(package, executable)
        executable.chmod(0o700)
        with (directory / "extract.log").open("wb") as output:
            command([str(executable), "--appimage-extract"], cwd=directory, stdout=output, stderr=output)
        appdir = directory / "squashfs-root"
        return appdir / "usr/bin/agent2api", appdir, True
    if package.name.endswith(".tar.gz"):
        with tarfile.open(package) as archive:
            members = archive.getmembers()
            # 发布附件只需常规文件和目录，不能通过链接或绝对路径写出暂存目录。
            for member in members:
                destination = (directory / member.name).resolve()
                if not destination.is_relative_to(directory.resolve()) or not (member.isfile() or member.isdir()):
                    raise RuntimeError(f"不安全的包内条目：{member.name}")
            archive.extractall(directory, members=members)
        return directory / "agent2api-server", None, False
    raise RuntimeError(f"不支持的安装包：{package}")


def stop(process):
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        process.wait(timeout=5)
        return
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        pass
    # D-Bus / WebKit 可能比主进程活得久；只回收这次创建的进程组。
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    process.wait(timeout=5)


@contextmanager
def display_server(directory):
    with (directory / "xvfb.log").open("wb") as output:
        process = subprocess.Popen(
            ["Xvfb", "-displayfd", "1", "-screen", "0", "1024x768x24", "-nolisten", "tcp"],
            stdout=subprocess.PIPE, stderr=output, start_new_session=True,
        )
        try:
            if not select.select([process.stdout], [], [], 15)[0]:
                raise RuntimeError("虚拟显示器启动超时")
            number = process.stdout.readline().decode().strip()
            if not number.isdigit():
                raise RuntimeError("虚拟显示器启动失败")
            yield f":{number}"
        finally:
            process.stdout.close()
            stop(process)


def environment(directory, port, display):
    values = {"PATH": "/usr/local/bin:/usr/bin:/bin", "LANG": "C.UTF-8"}
    for key, name in (("HOME", "home"), ("XDG_CONFIG_HOME", "config"),
                      ("XDG_CACHE_HOME", "cache"), ("XDG_DATA_HOME", "share"),
                      ("XDG_RUNTIME_DIR", "run"), ("AGENT2API_PROXY_HOME", "data")):
        path = directory / name
        path.mkdir(mode=0o700)
        values[key] = str(path)
    values.update({
        "AGENT2API_HOST": "127.0.0.1", "AGENT2API_PROXY_PORT": str(port),
        "AGENT2API_SELF_UPDATE": "0", "TOKIO_WORKER_THREADS": "2",
        "NO_AT_BRIDGE": "1", "LIBGL_ALWAYS_SOFTWARE": "1",
        "GDK_BACKEND": "x11", "DISPLAY": display,
    })
    return values


def appimage_environment(appdir, values):
    # AppRun 的 exec 会脱离 user-mode QEMU；直接运行包内 ELF，沿用其库路径和 usr/ 工作目录。
    values.update({
        "APPDIR": str(appdir),
        "LD_LIBRARY_PATH": ":".join(str(appdir / path) for path in (
            "usr/lib", "usr/lib/x86_64-linux-gnu", "usr/lib64", "lib", "lib/x86_64-linux-gnu", "lib64")),
        "XDG_DATA_DIRS": f"{appdir}/usr/share:/usr/local/share:/usr/share",
        "GTK_DATA_PREFIX": str(appdir), "GTK_THEME": "Adwaita:light",
        "GTK_EXE_PREFIX": str(appdir / "usr"), "GTK_PATH": str(appdir / "usr/lib/gtk-3.0"),
        "GSETTINGS_SCHEMA_DIR": str(appdir / "usr/share/glib-2.0/schemas"),
        "GI_TYPELIB_PATH": str(appdir / "usr/lib/girepository-1.0"),
        "GTK_IM_MODULE_FILE": str(appdir / "usr/lib/gtk-3.0/3.0.0/immodules.cache"),
        "GDK_PIXBUF_MODULE_FILE": str(appdir / "usr/lib/gdk-pixbuf-2.0/2.10.0/loaders.cache"),
        "GIO_EXTRA_MODULES": str(appdir / "usr/lib/x86_64-linux-gnu/gio/modules"),
    })


def database_ready(path):
    if not path.is_file():
        return False
    try:
        with closing(sqlite3.connect(f"{path.as_uri()}?mode=ro", uri=True, timeout=0.2)) as connection:
            return connection.execute("SELECT count(*) FROM sqlite_master WHERE type='table'").fetchone()[0] > 0
    except sqlite3.Error:
        return False


def check_cpu(qemu, cpu, binary, appdir, desktop, directory, display, timeout):
    directory.mkdir()
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        port = listener.getsockname()[1]
    values = environment(directory, port, display)
    values["AGENT2API_UI_DIR"] = str(binary.parent / "ui")
    guest = dict(values)
    if appdir:
        appimage_environment(appdir, guest)
    args = [qemu, "-cpu", cpu]
    # 包内动态库只能传给目标程序，不能污染 QEMU、D-Bus 或虚拟显示器自身。
    for key, value in guest.items():
        if values.get(key) != value:
            args.extend(["-E", f"{key}={value}"])
    args.append(str(binary))
    if desktop:
        args.extend(["--autostart"])
        args = ["dbus-run-session", "--"] + args
    started = time.monotonic()
    ready_since = None
    success = False
    log_path = directory / "startup.log"
    with log_path.open("wb") as output:
        process = subprocess.Popen(args, cwd=appdir / "usr" if appdir else directory, env=values, stdout=output,
                                   stderr=subprocess.STDOUT, start_new_session=True)
        try:
            while time.monotonic() - started < timeout and process.poll() is None:
                try:
                    with socket.create_connection(("127.0.0.1", port), timeout=0.2):
                        listening = True
                except OSError:
                    listening = False
                text = log_path.read_text(errors="replace")
                logged_ready = ("服务已就绪" in text if desktop else "headless 运行中" in text)
                if listening and logged_ready and database_ready(directory / "data/agent2api.db"):
                    ready_since = ready_since or time.monotonic()
                    if time.monotonic() - ready_since >= 2:
                        success = True
                        break
                else:
                    ready_since = None
                time.sleep(0.2)
            exit_code = process.poll()
        finally:
            stop(process)
    success = success and exit_code is None
    result = {"cpu": cpu, "passed": success, "exit_before_cleanup": exit_code,
              "seconds": round(time.monotonic() - started, 2), "log": str(log_path)}
    print(json.dumps(result, ensure_ascii=False), flush=True)
    if not success:
        print(log_path.read_text(errors="replace")[-6000:], flush=True)
    return result


def run(args):
    if not sys.platform.startswith("linux"):
        raise RuntimeError("兼容检查必须在 Linux 上运行")
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
    if args.timeout < 5:
        raise RuntimeError("启动检查超时必须至少为 5 秒")
    qemu = shutil.which(args.qemu)
    if not qemu:
        raise RuntimeError(f"找不到 CPU 模拟器：{args.qemu}")
    command([qemu, "--version"])
    # 发布检查必须由无外网的网络命名空间或 --network none 容器执行。
    interfaces = {name for _, name in socket.if_nameindex()}
    if interfaces - {"lo"}:
        raise RuntimeError("请在无外网的独立网络命名空间中运行兼容检查")
    results = []
    with tempfile.TemporaryDirectory(prefix="agent2api-cpu-") as temporary:
        root = Path(temporary)
        with display_server(root) as display:
            for index, value in enumerate(args.packages):
                package = Path(value).resolve(strict=True)
                directory = root / str(index)
                directory.mkdir()
                binary, appdir, desktop = extract_package(package, directory)
                require_elf(binary)
                digest = hashlib.sha256(package.read_bytes()).hexdigest()
                print(f"检查 {package.name}，SHA256 {digest}", flush=True)
                for cpu in CPUS:
                    result = check_cpu(qemu, cpu, binary, appdir, desktop, directory / cpu, display, args.timeout)
                    result.update({"package": package.name, "sha256": digest})
                    results.append(result)
                    if args.logs:
                        target = Path(args.logs) / f"{package.name}-{cpu}.log"
                        target.parent.mkdir(parents=True, exist_ok=True)
                        shutil.copyfile(result["log"], target)
                        result["log"] = str(target.resolve())
        if args.logs:
            (Path(args.logs) / "results.json").write_text(json.dumps(results, ensure_ascii=False, indent=2))
    if not all(result["passed"] for result in results):
        raise RuntimeError("安装包未通过 CPU 兼容检查，拒绝发布")
    print("所有安装包均通过老款和新款 CPU 启动及数据库初始化检查。")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="检查 Linux amd64 发布安装包的 CPU 启动兼容性")
    parser.add_argument("packages", nargs="+")
    parser.add_argument("--qemu", default="qemu-x86_64")
    parser.add_argument("--timeout", type=int, default=60)
    parser.add_argument("--logs", help="保留启动日志和结果的目录")
    try:
        run(parser.parse_args())
    except (RuntimeError, OSError, subprocess.SubprocessError, tarfile.TarError) as error:
        print(f"兼容检查失败：{error}", file=sys.stderr)
        sys.exit(1)
