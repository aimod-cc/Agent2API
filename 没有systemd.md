没有 systemd
cd /var/www/Agent2API

# 1.（可选）带 Cloudflare Tunnel 的话先配环境变量
#    数据目录要落在一个持久化挂载上，否则容器重建数据丢失
export AGENT2API_DATA_DIR=/var/www/Agent2API/data

# 2. 前台跑（先验证能不能起来）
mkdir -p /var/www/Agent2API/data
./agent2api-server

# 3. 能起来后 Ctrl+C，改用后台常驻（不依赖 systemd 的经典方式）
cd /var/www/Agent2API

AGENT2API_DATA_DIR=/var/www/Agent2API/data \
AGENT2API_CLOUDFLARED_ENABLED=1 \
AGENT2API_CLOUDFLARED_TOKEN="eyJhbGciOi...（控制台复制的整串）" \
nohup ./agent2api-server > /var/www/Agent2API/server.log 2>&1 &
echo $! > /var/www/Agent2API/server.pid


# 4.停服务时别只 kill 二进制——它是父进程，Tunnel 子进程会跟着退出，直接：
kill $(cat /var/www/Agent2API/server.pid)
