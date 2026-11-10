# v0.7.6：COS 与管理端控制客户端更新

更新发布与源码仓库是否公开无关。更新不请求 GitHub，也不需要公开管理后台端口。登录服务器的客户端通过现有控制连接收到已发布版本信息；安装包从管理员配置的 COS HTTPS 固定链接下载。

## 使用流程

1. 手动安装 v0.7.6 客户端，并部署本次对应架构服务端（保留原数据目录、配置、身份文件）。
2. 把客户端包上传到 COS，保持公有读、私有写。不要上传 `.release-keys`。
3. 管理后台 → 客户端更新 → 对应平台 → 配置：填写版本号、HTTPS 下载链接、`.sig` 完整文本、更新说明。
4. 保存草稿。核对后点击确认发布；发布前服务端 HEAD 校验公网链接，失败不发布。
5. 客户端启动 15 秒后首次检查，之后每分钟检查当前服务器的发布信息；设置页可以手动检查。版本必须高于当前版本才提示。

后台支持 Windows x64、macOS Apple Silicon/Intel、Linux x64/ARM64 独立配置。本次产物：Windows x64、macOS Apple Silicon、Linux x64 客户端；Linux amd64、arm64 服务端。

只有服务器管理员/MASTER 绑定的非只读 Web 账号能修改。写请求要求 CSRF，配置使用 revision 防止并发覆盖，按操作写入审计。草稿、已发布和暂停状态独立，保存在安全数据目录 `client-updates.json`。暂停只停止后续通知与新下载，不撤回已下载的包。

## 文件选择

| 平台 | 手动安装 | 后台更新链接 |
|---|---|---|
| Windows x64 | `EdenVoice_0.7.6_x64-setup.exe` | 同一 exe |
| macOS Apple Silicon | `.dmg` | `EdenVoice.app.tar.gz` |
| Linux x64 | `.AppImage` | 同一 AppImage |

签名必须选择更新链接指向文件所对应的 `.sig`。每个平台目录中的 `admin-update-平台.json` 已填好版本和签名，下载链接的桶名/地域是占位符，需要替换成真实文件地址。可以用环境变量 `EDENVOICE_COS_BASE_URL` 在打包时生成真实模板。无需向 COS 上传或维护 latest.json。

建议对象路径：`releases/v0.7.6/平台/文件名`。不要覆盖已发布包，后续修改使用新版本目录。下载链接必须 HTTPS、无账号密码、无临时鉴权查询参数。域名认证完成前可以使用 COS 默认下载域名。

## 安装行为

- 用户确认后才下载；可以立即安装重启或下次启动安装。
- 下载期间进入通话，安装前再次确认是否结束通话；未确认时保留为待安装。
- 下载进度显示百分比或已下载 MB。检查/下载网络失败静默处理。
- 完整下载通过固定公钥验签后，最后写入待安装记录。再次安装缓存时重新验签。
- 下次启动安装在语音核心连接之前进行，使用缓存包和本地元数据，不要求 GitHub、COS或语音服务器在线。
- Windows 启动安装器后退出，并让 NSIS 重启应用；系统仍可能要求权限确认。如果 OS 拒绝启动安装器而核心已关闭，重启原客户端恢复使用。
- macOS/Linux 安装成功后重启。Linux 自动更新针对 AppImage。真实跨版本替换仍需实机验收。

## 本地安装适配

Tauri 官方 updater 的安装上下文必须从 HTTP manifest 创建。这里将安全控制连接收到的元数据通过仅绑定 `127.0.0.1`、随机端口、单次响应、数秒自动退出的本地适配器交给插件；不向外网提供接口。配置的 insecure transport 仅用于这个固定回环地址；所有管理员配置的远端安装包链接仍强制 HTTPS，文件签名始终校验。没有客户端 JavaScript updater 插件权限。

## 打包与签名

`package-release.sh` 生成本机/Windows 交叉编译包；`package-linux-container.sh` 在本地容器生成 Linux 包；`package-server-release.sh` 生成两个 Linux 服务端包。更新公钥在 `desktop/src-tauri/updater.pub` 与配置中一致。

私钥在本机忽略目录 `.release-keys/updater.key`，请单独备份，不要上传 COS 或提交 Git。后续发布使用同一密钥。本地打包也可通过 `TAURI_SIGNING_PRIVATE_KEY` / `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` 环境变量指定密钥。

本机 macOS 依赖已有兼容修复，打包时使用 `EDENVOICE_SKIP_APM_PATCH=1` 保留现有依赖补丁。`RNNOISE_MODEL_PATH` 可以复用 SHA-256 校验过的模型包，Linux 容器脚本会以只读方式挂载该模型。

## 检查

服务端完整 Go 测试、新增更新持久化/并发/权限/内网链接拦截测试、客户端构建与原生验签测试、管理端真实浏览器草稿发布回归、客户端确认/延期/通话/静默失败回归。打包后另用固定公钥验证实际更新包签名，并生成上传文件 SHA-256 清单。
