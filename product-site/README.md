# EdenVoice 产品介绍页

无构建依赖的静态网站。页面介绍 v0.8.5 的实际功能，包括房间内屏幕共享、共同游戏档案与回顾卡片。继续使用客户端风格的 CSS 房间场景，不依赖生成背景图。

## 产品版本与可下载安装包

`releases.json` 分别维护页面介绍的产品版本和实际可下载的安装包：

- `product_version`：页面介绍的产品版本，当前为 `0.8.5`。
- `version`：已发布安装包的默认版本，当前为 `0.8.1`；没有独立版本字段的平台以此为准。
- `versions`：各平台安装包的真实版本。当前 Windows 为 `0.8.1`，macOS、Linux 与两个服务端压缩包为 `0.8.0`。
- `downloads.windows-x86_64`：Windows x64 安装 EXE。
- `downloads.darwin-aarch64`：Apple Silicon DMG，**不是自动更新用的 app.tar.gz**。
- `downloads.linux-x86_64`：Linux x64 AppImage。
- `downloads.server-linux-amd64`、`downloads.server-linux-arm64`：服务端压缩包。

**只有完成对应平台打包、上传并核验链接后，才更新该平台的 URL 与 `versions`。** 产品功能或服务端先发布，不代表所有平台的新安装包已经可下载。下载卡片会逐平台显示安装包版本，并提示较早的包可能不包含全部功能；不构造尚不存在的 v0.8.5 下载地址。

先上传安装包并核验链接，再原子替换 `releases.json` 即可，无需修改 HTML、JavaScript 或重新构建。网页每次加载以 `cache: no-store` 读取配置；已打开的页面刷新后更新。若未来启用 CDN，需避免缓存该 JSON 或发布时刷新它。

所有版本采用三段数字（例如 `0.8.5`，不带 v）。所有下载地址必须为不含用户名密码的完整 HTTPS URL。删除某个平台键、填空字符串或 null 可暂停该平台入口。请求失败、版本格式错误或含非法 URL 时禁用下载并显示重试；不保留旧 URL 兜底。动态文本使用 textContent，不注入 HTML。

产品页配置与语音服务器后台的客户端更新配置独立。产品页只负责首次安装下载；自动更新仍需在服务器后台按平台填写版本、更新包 URL 和对应 .sig 签名，保存草稿并确认发布。

## 文件和部署

- `index.html`：产品介绍及下载位置；不硬编码安装包 URL。
- `releases.json` / `downloads.js`：产品与安装包版本、下载配置与校验、加载、重试。
- `app.js`：房间场景、屏幕共享与全屏示意、档案筛选、计时演示、阅读进度。
- `styles.css`：桌面/移动布局与减少动态效果支持。
- `guide.html`、`faq.html`：新用户指南、单 UDP 端口部署说明及常见问题。
- `assets/app-icon.svg`：当前品牌图标。

将上述文件按原结构放在网站根目录或子目录。首页为 index.html，JSON 与 JS 必须同时上传。通过 HTTP(S) 访问；file:// 下浏览器可能禁止读取 JSON。禁用 JavaScript 时仍可阅读产品介绍和指南，下载区提示启用 JavaScript。

```sh
python3 -m http.server 8703 --bind 127.0.0.1 --directory product-site
```

浏览器验证脚本：`scripts/product-downloads.test.mjs`，支持通过 PLAYWRIGHT_MODULE / CHROME_EXECUTABLE 指定本地浏览器运行时。覆盖分平台版本、JSON 切换、下载失败重试、交互演示和移动布局。

产品页不公开私人语音服务器地址，不提供体验服务器复制入口。演示画面使用虚构昵称与活动，不建立实际共享、录音或语音连接。
