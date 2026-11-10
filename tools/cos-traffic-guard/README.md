# EdenVoice COS 下载流量守护（Go 独立二进制）

无需 Python、Go 运行环境。Linux AMD64/ARM64 静态包包含程序、500 MB 配置、systemd 服务、安装脚本及 CAM 权限模板。独立运行，不修改语音服务。

配置模板使用占位桶名。先把 `config.json` 的 `bucket`、`region`、`probe_key` 和 `cam-policy.json` 中的地域、APPID、桶名替换为自己的配置；不要直接部署占位值。默认北京时间每日外网下行流量累计达到 **500,000,000 字节**即封堵整个桶的匿名下载。每轮完成后等 60 秒再次检查，读取 300 秒粒度监控点求和；同时复查昨天，防止午夜延迟数据漏掉。第一次启用若昨天已经超限也会触发。

## 创建腾讯云子账号和密钥

1. 主账号打开 **访问管理 CAM → 用户 → 用户列表 → 新建用户 → 自定义创建**，用户名可用 `edenvoice-cos-guard`，启用编程访问。仅供脚本使用时不需要控制台登录。
2. **策略 → 新建自定义策略 → 按策略语法创建**，粘贴包内 `cam-policy.json`，命名 `EdenVoiceCOSGuard`。将该策略关联到此子账号。只需要监控读取、指定桶 GetBucketPolicy 和 PutBucketPolicy，不要授予 AdministratorAccess 或 COS 全权限。
3. 打开子账号详情 → **API 密钥 → 新建密钥**，保存 SecretId/SecretKey。SecretKey 只在创建时显示。
4. 密钥不要粘贴进聊天、命令参数或 git；用下方交互命令在自己的 SSH 终端输入，两项均不回显。程序以 0600 权限写入文件。临时凭据可手动在该私密 JSON 中增加 `token`，到期必须替换。

- [官方子账号密钥步骤](https://cloud.tencent.cn/document/product/598/37140)
- [CAM 控制台](https://console.cloud.tencent.com/cam)

## 安装和配置

普通 x86_64 服务器选择 AMD64 包，ARM 服务器选择 ARM64 包。

```bash
tar -xzf edenvoice-cos-guard-linux-amd64.tar.gz
cd edenvoice-cos-guard-linux-amd64
sudo sh install.sh

# 在你自己的 SSH 终端运行，按提示粘贴 SecretId / SecretKey
sudo /opt/edenvoice-cos-guard/cos-traffic-guard \
  --init-credentials \
  --credentials /etc/edenvoice-cos-guard/credentials.json
```

该命令不覆盖已有凭据。更换密钥时先停止守护程序，安全移走旧凭据文件，再初始化新文件；验证后删除旧密钥。

先运行只读检查（不封堵），核对监控和桶：

```bash
sudo /opt/edenvoice-cos-guard/cos-traffic-guard \
  --config /etc/edenvoice-cos-guard/config.json \
  --credentials /etc/edenvoice-cos-guard/credentials.json \
  --state-dir /var/lib/edenvoice-cos-guard
```

确认配置后启动：

```bash
sudo systemctl enable --now edenvoice-cos-guard
sudo journalctl -u edenvoice-cos-guard -f
```

日志 `traffic` 给出日期、累计字节和阈值；`below_threshold` 表示未超限；只读时 `would_block` 表示已达到阈值；`blocked_verified` 表示已写入规则、读回校验并探测到匿名下载为 403。`check_failed` 表示未确认保护成功，必须处理权限、凭据、网络或监控问题。不要把服务 active 当作防护验证通过。

## 封堵和恢复

超限时追加 SID 为 `EdenVoiceTrafficGuardDenyPublicDownloads` 的匿名 GetObject 拒绝规则，不改 ACL、不删除文件，保留原策略条目和未知字段；修改前在状态目录备份原策略并检查是否有并发修改。COS 写入策略不是条件更新，执行期间不要同时在控制台编辑桶策略。

先落盘封堵标记，再操作云 API；失败会重试，重启和跨天不自动解除。实际生效后仍定期确认策略及匿名访问。`probe_key` 必须是自己桶中实际存在且可匿名下载的文件；删除或迁移它时需同步更新配置。

手动恢复：

1. `sudo systemctl stop edenvoice-cos-guard`。
2. COS 控制台 → 桶 → 权限管理 → 桶访问策略，仅删除上述 SID 对应的规则，保留其他规则。不要直接恢复整份旧备份覆盖后续配置。
3. 删除 `/var/lib/edenvoice-cos-guard/blocked.json`，保留策略备份。
4. 查清原因、确认下载恢复后再决定何时启动。今天或昨天仍超限，启动后会再次封堵。

## 限制

- 这是监控熔断，不是实时计费封顶。数据有延迟或缺点，在途下载也可能继续，真实流量可能超过 500 MB。500 MB 可能只够一个较大的安装包。
- 整桶公开下载会停止，同桶的静态网站也受影响；不影响独立语音服务。
- 只针对当前公开匿名 COS 直链；有权限的签名请求、CDN 缓存和其他计费项不在此规则保护范围。不会锁掉账号管理文件的权限。
- 监控缺失/异常不会伪装成 0，但在尚未触发时也不会自动封堵；已有封堵标记则持续尝试封堵。应关注错误日志，不能靠监控延迟方案保证最多只花 1 元。
- 单元和 HTTP 集成测试不等于真实桶验证。配置凭据后需先核对只读监控数据，实际触发后以 `blocked_verified` 和下载 403 验证。

## 从源码测试与打包

```bash
go test -race ./...
go vet ./...
./package.sh
```

输出 Linux AMD64/ARM64 tar.gz 和 SHA256SUMS.txt。源码使用独立 go.mod/go.sum，不改变服务端依赖。

## 腾讯云依据

- [COS 监控指标：InternetTraffic、300 秒 sum；监控地域固定广州，桶维度使用完整桶名](https://cloud.tencent.cn/document/product/248/45140)
- [访问策略评估：Deny anyone 与签名访问的区别](https://cloud.tencent.com/document/product/436/41516)
- [官方 Go 桶策略 API](https://cloud.tencent.com/document/product/436/51472)

## 2026-09-09 线上核对修正（1.0.1）

此桶的 `InternetTrafficDown` 返回全零，实际下载流量在 `InternetTraffic`；两种 bucket/APPID 维度返回相同结果。修正为后者，与腾讯云防盗刷指引一致。只读诊断命令 `--diagnose` 对比指标，不修改策略。另兼容无策略时带 COS 请求 ID 的空正文 404，以及 `NoSuchPolicy` / `NoSuchBucketPolicy`；其他 404 不当作可覆盖的空策略。
