# Web 管理控制台

管理端作为服务端内置静态页面和 HTTP API 交付，不依赖 Node.js、数据库或独立前端进程。默认关闭，不会占用原语音端口；启用后有独立监听、HTTP 超时和登录限流。Web 服务启动失败会记录错误，语音服务继续运行。

## 页面与功能

| 页面 | 能力 |
| --- | --- |
| 总览 | 在线连接、成员身份数、活跃房间、CPU、RSS、流量趋势、媒体队列与观影数量 |
| 资源与性能 | Go CPU/堆、协程、Linux 主机内存和负载、数据分区容量、媒体流量、Worker/Writer 深度与丢弃 |
| 在线连接 | 精确会话 ID、身份、房间、TCP/UDP 地址、心跳、连接时长、加密状态和客户端链路报告；移动、禁言/解除禁言、移出房间、断开连接 |
| 房间 | 创建、编辑名称/上级/排序/人数/音质、独立网络策略调整、频道管理员授予/撤销、删除及结束共同观影 |
| 成员 | 已知身份、在线/离线、多会话、最后活动、角色调整、封禁/解封与头像 |
| 封禁 | 原因、操作者、到期时间、永久封禁与解除 |
| 操作记录 | 本次进程最近 200 条管理事件，包括客户端和 Web 管理操作；完整记录输出到服务器结构化日志 |
| Web 账号 | MASTER 创建、改绑、设只读、停用和重置账号密码；所有登录用户可修改自己的密码 |

界面包括搜索、表格分页、移动端导航、确认对话框、提交防重复、失败提示、登录过期处理。高影响操作在提交前展示目标及影响。删除房间沿用现有安全备用房间迁移逻辑，不能删除有子房间或无安全备用房间的频道。临时房间需由实际语音参与者创建，Web 可以管理已存在的临时房间。

## 权限与账号

Web 账号绑定已存在的语音身份 Client ID，只接受服务器 MASTER 或 ADMIN。不会创建虚拟语音成员，也不会绕过现有权限检查。

- **MASTER**：房间、成员、服务器角色、封禁及 Web 账号管理；不能通过此页面修改 MASTER 归属。
- **ADMIN**：复用现有服务器管理员权限，不能操作 MASTER 或变更服务器角色；不能管理 Web 账号。
- **只读账号**：绑定有效管理员身份，查看运行数据，但不能执行管理操作；可以修改自己的 Web 密码。
- 绑定身份被撤权、角色变化或封禁后，会话不能继续使用。禁用/改绑账号、重置或修改密码会使所有旧会话失效。

频道管理员没有服务器范围的 Web 访问权；仍使用语音客户端管理自己有权限的频道。

密码使用 bcrypt cost 12，长度 12–72 字节，存储在 `<data_dir>/web-accounts.json` 的哈希字段。文件权限 `0600`、目录 `0700`，更新采用临时文件同步后原子替换。会话使用随机令牌的 HttpOnly / SameSite=Strict cookie；HTTPS 使用 Secure 属性。令牌仅保留于服务端内存与 cookie，不放入 localStorage。会话最长 8 小时，30 分钟没有请求后失效；服务端重启也会使会话失效。

写入接口验证固定 Origin 与 CSRF token，登录同样验证 Origin。固定 Host 校验防止通过非配置域名访问；所有 API 都必须登录。登录密码验证最多同时执行一次，IP / 账号限流、会话数和管理写频率均有限制。页面使用同源静态资源、CSP 与输出转义，不加载第三方脚本。API 不导出密码哈希、媒体密钥、客户端临时密钥或头像原始存储全集。

安全设计参考：[OWASP Session Management](https://cheatsheetseries.owasp.org/cheatsheets/Session_Management_Cheat_Sheet.html)、[Go bcrypt API](https://pkg.go.dev/golang.org/x/crypto/bcrypt)。

## 首次配置

先在语音客户端完成 MASTER 认领。然后在服务器本机初始化 Web 账号；未指定绑定 ID 时使用当前 MASTER。为避免旧进程仍持有账号缓存，初始化/重置账号时应停止服务，再初始化并重新启动。

```bash
# bash 示例：隐藏输入密码，不将密码放入命令行参数或配置文件
read -r -s -p 'Web admin password: ' web_admin_password
printf '\n'
printf '%s' "$web_admin_password" | ./voice-server --config /etc/edenvoice/config.toml --web-admin-init admin
unset web_admin_password
```

也可以通过 `--web-admin-client-id <已有 MASTER 或 ADMIN 的 ClientID>` 明确指定绑定身份。同名账号会被重置，因此该命令只在服务器本机提供，不开放无认证远程初始化接口。

配置文件加入：

```toml
[web_admin]
enabled = true
listen_addr = "127.0.0.1:9101"
```

启动新版服务端。通过 SSH 隧道访问，不需要开放新的公网端口：

```bash
ssh -L 9101:127.0.0.1:9101 <server-user>@<server-address>
```

浏览器打开 `http://127.0.0.1:9101`。默认 Origin 就是该地址，请保持主机名及端口一致。

### 公网 HTTPS

推荐保持 HTTP 监听回环地址，由 HTTPS 反向代理提供访问。设置准确的外部 Origin：

```toml
[web_admin]
enabled = true
listen_addr = "127.0.0.1:9101"
public_url = "https://admin.example.com"
```

代理需保留原始 Host，管理页面部署在独立域名根路径。例如已有 TLS Nginx 站点内：

```nginx
location / {
    proxy_pass http://127.0.0.1:9101;
    proxy_set_header Host $http_host;
    proxy_read_timeout 20s;
}
```

管理端不信任 `X-Forwarded-For`，代理部署下登录 IP 限流按代理地址计数；仍有独立账号限流。不要将外部明文 HTTP 代理到此入口。也可配置 `tls_cert` 和 `tls_key` 由 Go 直接提供 HTTPS；非回环监听没有证书时会拒绝启动。证书获取、续期和域名配置由运维环境提供。

## 指标口径与通话优先

- 资源/媒体指标至多每 5 秒采样一次，缓存最多 120 个采样点；没有后台高频 CPU/内存探测。首次采样不足以计算 CPU 差值时显示 `—`。
- 页面每 5 秒刷新；页面隐藏或手动暂停后停止轮询。图表是当前管理服务生命周期中被请求时采集的数据，不是全天时序数据库。
- Go CPU 是运行时估算的累计 CPU 时间差，单核 100%，多核可能超过 100%；不是整机 CPU 使用率。
- Go 堆、进程 RSS、主机内存分别展示。Linux 提供 `/proc` 资源；其他平台不能读取的数据是 `—`，不伪装为 0。
- 流量和队列只覆盖服务器媒体中继，不能代表 P2P 直连带宽或主观听感。
- 连接列表显示成员请求的传输偏好，并明确不是实际路由证明；RTT/丢包/抖动来自客户端报告，汇总仅使用最近 15 秒的报告，详情保留时间戳。
- 64 位房间、会话和观影 epoch 以字符串发送给浏览器，避免 JavaScript 精度丢失；提交时服务器校验并还原整数。
- 不触碰语音采集、DSP 或 UDP 热路径。管理操作复用客户端的鉴权、状态更新、持久化和广播逻辑。

## 本机演示与验证

```bash
cd server
EDENVOICE_DEMO_PASSWORD='<至少12字节的演示密码>' go run ./cmd/admin-demo
```

打开 `http://127.0.0.1:9181`。提供 `admin`、`operator`、`viewer` 三个账号，使用上述同一演示密码。该命令使用临时数据目录与样例身份，语音成员是模拟的；登录、管理操作、头像、持久化接口和本机资源读取均走真实实现。它不会连接生产服务器。

自动化：

```bash
cd server
go test -race ./...
# 启动上述演示后，另开终端运行浏览器回归（需 Playwright / Chromium）
EDENVOICE_DEMO_PASSWORD='<演示密码>' node scripts/admin-ui.test.cjs
```

可用环境变量 `PLAYWRIGHT_MODULE`、`CHROME_EXECUTABLE` 指定现有运行时，`ADMIN_ARTIFACT_DIR` 指定截图目录。浏览器测试只允许回环 URL，并核对样例服务器名称后才执行修改，避免误用于生产服务器。

本轮开发未修改生产服务器配置、未开放公网管理端，也未改动 `v0.7` 标签。上线需先部署包含管理端的新版服务端，再完成账号初始化和 HTTPS / 隧道访问配置。
