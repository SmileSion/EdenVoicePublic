#!/bin/sh
set -eu
[ "$(id -u)" = 0 ] || { echo '请使用 sudo 安装'; exit 1; }
source_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
# Install only. Credentials and explicit activation are separate steps.
install -d -m 755 /opt/edenvoice-cos-guard
install -d -m 700 /etc/edenvoice-cos-guard /var/lib/edenvoice-cos-guard
install -m 755 "$source_dir/cos-traffic-guard" /opt/edenvoice-cos-guard/cos-traffic-guard.new
mv /opt/edenvoice-cos-guard/cos-traffic-guard.new /opt/edenvoice-cos-guard/cos-traffic-guard
if [ ! -e /etc/edenvoice-cos-guard/config.json ]; then
 install -m 600 "$source_dir/config.json" /etc/edenvoice-cos-guard/config.json
fi
install -m 644 "$source_dir/edenvoice-cos-guard.service" /etc/systemd/system/edenvoice-cos-guard.service
systemctl daemon-reload
printf '%s\n' '已安装（尚未启动），现有配置、凭据及状态保持不变。' \
 '下一步：sudo /opt/edenvoice-cos-guard/cos-traffic-guard --init-credentials --credentials /etc/edenvoice-cos-guard/credentials.json'
