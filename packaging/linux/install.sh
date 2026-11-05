#!/usr/bin/env bash
# Agent2API 服务端安装脚本（Linux，无界面形态）。
#
#   sudo bash install.sh              安装 / 重装
#   sudo bash install.sh --upgrade    只换程序与界面，保留数据与配置
#   sudo bash install.sh --uninstall  卸载（保留数据目录，除非再加 --purge）
#
# 脚本假定它与 agent2api-server、ui/、agent2api.service、
# agent2api.env.example 在同一个目录里（发行包解包后就是这个布局）。
#
# ── 做了什么 ────────────────────────────────────────────────
#   1. 建服务账号 agent2api（无登录 shell）
#   2. 装程序到 /opt/agent2api（可执行文件 + ui/）
#   3. 建数据目录 /var/lib/agent2api（属服务账号）
#   4. 首次安装时生成 /etc/agent2api/agent2api.env（从 example 复制）
#   5. 装 systemd 单元并 enable + start
#
# ── systemd 一键更新 ────────────────────────────────────────
# 网关仍以普通账号运行，程序目录仍归 root。独立的按需升级程序通过本机
# socket 接收请求，只能替换官方校验通过的服务端包，不提供通用提权。
# 安装与 --upgrade 共用下面的组件配置，旧安装升级一次后即可面板更新。

set -euo pipefail

SERVICE_NAME="agent2api"
SERVICE_USER="agent2api"
SERVICE_GROUP="agent2api"
INSTALL_DIR="/opt/agent2api"
DATA_DIR="/var/lib/agent2api"
CONF_DIR="/etc/agent2api"
UNIT_PATH="/etc/systemd/system/${SERVICE_NAME}.service"

MODE="install"
PURGE=0
for arg in "$@"; do
  case "$arg" in
    --upgrade)   MODE="upgrade" ;;
    --uninstall) MODE="uninstall" ;;
    --purge)     PURGE=1 ;;
    -h|--help)
      sed -n '2,20p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *)
      echo "未知参数: $arg（可用：--upgrade / --uninstall / --purge）" >&2
      exit 1
      ;;
  esac
done

# ── 前置检查 ────────────────────────────────────────────────
if [ "$(id -u)" -ne 0 ]; then
  echo "错误: 需要 root 权限（请用 sudo 运行）" >&2
  exit 1
fi

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if [ "$MODE" != "uninstall" ]; then
  for required in agent2api-server ui agent2api.service; do
    if [ ! -e "$HERE/$required" ]; then
      echo "错误: 找不到 $HERE/$required —— 请在解包后的发行目录里运行本脚本" >&2
      exit 1
    fi
  done
  if ! command -v systemctl >/dev/null 2>&1; then
    echo "错误: 没找到 systemctl —— 本脚本面向 systemd 发行版" >&2
    echo "      其它 init 系统请直接前台运行 agent2api-server（见 README）" >&2
    exit 1
  fi
fi

# 升级组件由脚本生成：保持发行包原有的三个安装文件清单，不依赖额外附件。
configure_updater() {
  local unit_user unit_group unit_exec unit_work
  unit_user=$(systemctl show "$SERVICE_NAME" --property=User --value) || return 1
  unit_group=$(systemctl show "$SERVICE_NAME" --property=Group --value) || return 1
  unit_exec=$(systemctl show "$SERVICE_NAME" --property=ExecStart --value) || return 1
  unit_work=$(systemctl show "$SERVICE_NAME" --property=WorkingDirectory --value) || return 1
  if [ "$unit_user" != "$SERVICE_USER" ] || [ "$unit_group" != "$SERVICE_GROUP" ] \
    || [[ "$unit_exec" != *"path=$INSTALL_DIR/agent2api-server ;"* ]] \
    || [ "$unit_work" != "$INSTALL_DIR" ]; then
    echo "⚠️ 检测到自定义服务配置，保持原样，未启用一键更新。"
    echo "   请按原部署方式升级；一键更新需要官方安装路径和服务账号。"
    return
  fi
  local state_dir="/var/lib/agent2api-updater"
  local dropin="${UNIT_PATH}.d"
  install -d -m 0755 -o root -g root "$state_dir" "$dropin" || return 1
  if [ -L "$state_dir/ready.json" ]; then
    echo "错误: 升级就绪文件不能是符号链接" >&2
    return 1
  fi
  if [ ! -e "$state_dir/ready.json" ]; then
    install -m 0660 -o root -g "$SERVICE_GROUP" /dev/null "$state_dir/ready.json" || return 1
  fi
  chown root:"$SERVICE_GROUP" "$state_dir/ready.json" || return 1
  chmod 0660 "$state_dir/ready.json" || return 1
  if [ ! -e "$CONF_DIR/updater.env" ]; then
    printf '# 管理员可在此指定受信任的发布仓库及 HTTPS_PROXY；默认使用官方索引。\n' > "$CONF_DIR/updater.env" || return 1
    chmod 0600 "$CONF_DIR/updater.env" || return 1
  fi
  cat > /etc/systemd/system/agent2api-updater.socket <<'EOF' || return 1
[Unit]
Description=Agent2API local update channel
[Socket]
ListenStream=/run/agent2api-updater/control.sock
SocketUser=root
SocketGroup=agent2api
SocketMode=0660
DirectoryMode=0755
Accept=yes
MaxConnections=4
[Install]
WantedBy=sockets.target
EOF
  cat > /etc/systemd/system/agent2api-updater@.service <<EOF || return 1
[Unit]
Description=Agent2API restricted update worker
CollectMode=inactive-or-failed
[Service]
Type=exec
User=root
Group=root
ExecStart=$INSTALL_DIR/agent2api-server --systemd-updater
WorkingDirectory=/
StandardInput=socket
StandardOutput=socket
StandardError=journal
Environment=AGENT2API_UPDATER_UID=$(id -u "$SERVICE_USER")
EnvironmentFile=-$CONF_DIR/updater.env
NoNewPrivileges=true
PrivateTmp=true
ProtectHome=true
ProtectSystem=strict
ReadWritePaths=$INSTALL_DIR $state_dir
RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6
RuntimeMaxSec=600
TimeoutStopSec=10
EOF
  # 只新增自己负责的 drop-in，不覆盖主单元、用户 drop-in 或环境配置。
  cat > "$dropin/50-updater.conf" <<EOF || return 1
[Unit]
After=agent2api-updater.socket
[Service]
ReadWritePaths=$state_dir/ready.json
EOF
  systemctl daemon-reload || return 1
  systemctl enable --now agent2api-updater.socket || return 1
  echo "→ 一键更新组件已配置（网关权限保持不变）"
}

# ── 卸载 ────────────────────────────────────────────────────
if [ "$MODE" = "uninstall" ]; then
  echo "→ 停止并禁用服务"
  systemctl stop "$SERVICE_NAME" 2>/dev/null || true
  systemctl disable "$SERVICE_NAME" 2>/dev/null || true
  systemctl disable --now agent2api-updater.socket 2>/dev/null || true
  systemctl stop 'agent2api-updater@*.service' 2>/dev/null || true
  rm -f /etc/systemd/system/agent2api-updater.socket /etc/systemd/system/agent2api-updater@.service
  rm -f "${UNIT_PATH}.d/50-updater.conf"
  rmdir "${UNIT_PATH}.d" 2>/dev/null || true
  rm -f "$UNIT_PATH"
  systemctl daemon-reload
  echo "→ 删除程序目录 $INSTALL_DIR"
  rm -rf "$INSTALL_DIR"
  if [ "$PURGE" -eq 1 ]; then
    echo "→ 删除数据（$DATA_DIR）与配置（$CONF_DIR）"
    rm -rf "$DATA_DIR" "$CONF_DIR"
    if id -u "$SERVICE_USER" >/dev/null 2>&1; then
      userdel "$SERVICE_USER" 2>/dev/null || true
    fi
  else
    echo "→ 数据与配置已保留：$DATA_DIR / $CONF_DIR"
    echo "   （连它们一起删用 --purge）"
  fi
  echo "✅ 卸载完成"
  exit 0
fi

# ── 升级：只换程序，不碰数据与配置 ──────────────────────────
if [ "$MODE" = "upgrade" ]; then
  if [ ! -d "$INSTALL_DIR" ]; then
    echo "错误: $INSTALL_DIR 不存在 —— 这台机器还没装过，请用 install.sh（不带参数）" >&2
    exit 1
  fi
  echo "→ 停止服务"
  systemctl stop "$SERVICE_NAME"
  # 先备份旧二进制：新版本起不来时能立刻换回去
  if [ -f "$INSTALL_DIR/agent2api-server" ]; then
    cp -f "$INSTALL_DIR/agent2api-server" "$INSTALL_DIR/agent2api-server.bak"
  fi
  echo "→ 替换程序与界面"
  install -m 0755 "$HERE/agent2api-server" "$INSTALL_DIR/agent2api-server"
  rm -rf "$INSTALL_DIR/ui"
  cp -r "$HERE/ui" "$INSTALL_DIR/ui"
  chown -R root:root "$INSTALL_DIR"
  # 组件写入失败也要把已经升级的主服务拉起来，不让迁移故障变成停机。
  if ! configure_updater; then
    systemctl start "$SERVICE_NAME" || true
    echo "错误: 程序已替换，但一键更新组件配置失败，请检查上面的错误后重跑升级。" >&2
    exit 1
  fi
  echo "→ 启动服务"
  systemctl start "$SERVICE_NAME"
  echo "✅ 升级完成（旧版本备份在 $INSTALL_DIR/agent2api-server.bak）"
  systemctl status "$SERVICE_NAME" --no-pager --lines=5 || true
  exit 0
fi

# ── 全新安装 ────────────────────────────────────────────────
echo "→ 创建服务账号 $SERVICE_USER"
if ! id -u "$SERVICE_USER" >/dev/null 2>&1; then
  # --system：系统账号，不占普通用户 UID 段；
  # --no-create-home + /usr/sbin/nologin：不允许登录，数据目录单独给。
  useradd --system --no-create-home --shell /usr/sbin/nologin \
    --user-group "$SERVICE_USER" 2>/dev/null \
    || useradd --system --no-create-home --shell /usr/sbin/nologin "$SERVICE_USER"
fi

echo "→ 安装程序到 $INSTALL_DIR"
mkdir -p "$INSTALL_DIR"
install -m 0755 "$HERE/agent2api-server" "$INSTALL_DIR/agent2api-server"
rm -rf "$INSTALL_DIR/ui"
cp -r "$HERE/ui" "$INSTALL_DIR/ui"
chown -R root:root "$INSTALL_DIR"

echo "→ 准备数据目录 $DATA_DIR"
mkdir -p "$DATA_DIR"
chown -R "$SERVICE_USER:$SERVICE_GROUP" "$DATA_DIR"
chmod 0750 "$DATA_DIR"

echo "→ 准备配置 $CONF_DIR/agent2api.env"
mkdir -p "$CONF_DIR"
if [ ! -f "$CONF_DIR/agent2api.env" ]; then
  if [ -f "$HERE/agent2api.env.example" ]; then
    cp "$HERE/agent2api.env.example" "$CONF_DIR/agent2api.env"
  else
    # 发行包里没带 example（理论上不该发生）时给个能跑的最小配置
    printf 'AGENT2API_HOST=0.0.0.0\nAGENT2API_PROXY_PORT=3065\n' \
      > "$CONF_DIR/agent2api.env"
  fi
  chmod 0640 "$CONF_DIR/agent2api.env"
  chown root:"$SERVICE_GROUP" "$CONF_DIR/agent2api.env"
  echo "   已生成（首次安装）"
else
  echo "   已存在，保持不动（重装不会覆盖你的配置）"
fi

echo "→ 安装 systemd 单元"
install -m 0644 "$HERE/agent2api.service" "$UNIT_PATH"
systemctl daemon-reload
configure_updater
systemctl enable "$SERVICE_NAME" >/dev/null
systemctl restart "$SERVICE_NAME"

# ── 启动结果 ────────────────────────────────────────────────
sleep 2
if systemctl is-active --quiet "$SERVICE_NAME"; then
  PORT=$(grep -E '^AGENT2API_PROXY_PORT=' "$CONF_DIR/agent2api.env" 2>/dev/null \
    | tail -1 | cut -d= -f2 | tr -d '[:space:]')
  PORT=${PORT:-3065}
  cat <<EOF

✅ 安装完成

   面板     http://<本机IP>:${PORT}/
   网关     http://<本机IP>:${PORT}/v1

   配置文件 ${CONF_DIR}/agent2api.env
   数据目录 ${DATA_DIR}
   查看日志 journalctl -u ${SERVICE_NAME} -f

接下来：
   1. 浏览器打开上面的面板地址，首次进入会引导注册管理员账号
   2. 登录后在「网关 Key」页创建一把 API Key
   3. 客户端 base_url 填 http://<本机IP>:${PORT}/v1，api_key 填刚建的那把

⚠️  未创建 API Key 之前 /v1/* 会拒绝服务（fail-closed），这是预期行为。
EOF
else
  echo "❌ 服务没能启动。排查："
  echo "   systemctl status $SERVICE_NAME"
  echo "   journalctl -u $SERVICE_NAME -n 50 --no-pager"
  exit 1
fi
