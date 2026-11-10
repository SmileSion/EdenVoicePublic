# 开源与自行发行

## 许可范围

EdenVoice 当前第一方源码采用 **GPL-3.0-only**，覆盖客户端、服务端、管理界面和产品页。项目保留的 Slint 组件采用其 GPLv3 许可选项；其他依赖、模型及图标继续遵循各自的许可。

可以免费使用、自建服务器、修改和商业使用。单纯在自己的设备上使用或修改，不要求公开个人改动；向他人分发程序时，应遵守 GPL 对许可声明、对应源码及修改信息的要求。GPL 不是禁止商用的许可证，也不要求把私人服务器数据、用户聊天或签名私钥公开。完整条款见 [LICENSE](../LICENSE)，依赖声明见 [THIRD_PARTY_NOTICES.md](../THIRD_PARTY_NOTICES.md)。

之前已按 MIT 提供的 EdenVoice 版本保留原有授权，本次变更不追溯收回这些权利。第三方代码的版权和许可不能因根目录许可证变化而移除。

## 自行构建和更新

开发运行不需要官方签名私钥。发布带自动更新能力的安装包时，需要：

1. 生成并妥善备份自己的更新签名密钥。可在 `desktop/` 执行 `npm run tauri -- signer generate --help` 查看工具用法。
2. 同时将 `desktop/src-tauri/tauri.conf.json` 的 `plugins.updater.pubkey` 和 `desktop/src-tauri/updater.pub` 换成同一配套公钥。立即安装和下次启动安装都必须使用自己的验签公钥。公钥可以公开，私钥不能进入 Git。
3. 为本地打包设置 `TAURI_SIGNING_PRIVATE_KEY` / `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`，或按发布脚本使用被忽略的 `.release-keys/updater.key`。项目不使用 GitHub Actions，检查和打包在本地完成；没有密钥时签名发布会失败。
4. 在自己控制的存储中上传安装包，在服务器后台核对并发布。源码公开与更新渠道相互独立，fork 不会因此获得官方客户端的更新签名权限。

客户端资源和发布目录、服务端及 COS 工具包、Docker 镜像会携带 `LICENSE`、`THIRD_PARTY_NOTICES.md` 和 `THIRD_PARTY_LICENSES.md`。最后一份文件从锁定依赖生成；上游包遗漏的许可证按其记录的 Git 提交补入，并在 `third-party/licenses/` 保留来源和哈希。少数仅声明 SPDX 许可的包在 `third-party/license-fallbacks.json` 显式列出，保留原作者及源码声明并附标准许可条款；新增或改版时必须重新核对。

运行 `python3 scripts/generate-third-party-licenses.py --fetch-missing` 更新许可文件（需要 npm 依赖、Cargo/Go 工具和 GitHub CLI）；使用 `--check` 检查它是否与锁文件一致。生成结果包含多个目标平台及构建依赖，是有意保守的许可集合，不表示所有组件都进入某个安装包。原生音频库和模型的额外条款仍保留在 `THIRD_PARTY_NOTICES.md`。

正式打包脚本自动附带对应源码：客户端、服务端和 COS 工具使用 `scripts/package-source.py`，包含实际工作区文件哈希、Cargo/Go 依赖源码及构建和安装脚本；客户端还包含此次构建使用的 RNNoise 与 DeepFilterNet 模型。Docker 镜像内的 `/usr/share/edenvoice/corresponding-source.tar.gz` 包含该镜像的服务端与依赖源码。源码归档须与安装包在同一下载位置提供，保持可用；不要只链接不断变化的 `main`。已上传的旧安装包不会因仓库修改而自动更新。

打包需要 Python 3.11+。客户端源码打包可以设置 `RNNOISE_MODEL_PATH` 指向本次构建使用的模型 `.tar.gz`，未设置时从本机客户端构建缓存查找。保留 `vendor/` 中的安全补丁及原始许可；具体差异和回归测试见 `vendor/README.md`。不得通过忽略漏洞编号来替代这些补丁。

开发检查入口：`npm --prefix desktop ci`、`npm --prefix desktop run test:install`、`npm --prefix desktop test` 和 `npm --prefix desktop run test:admin`。后台测试自动创建随机密码和回环地址的隔离演示进程。独立视觉演示脚本需要各自的演示服务；真实 WebRTC 媒体测试使用 `EDEN_SCREEN_BROWSER=1` 运行服务端的 `TestBrowserDecodesScreenAndAudio`，并将 `PLAYWRIGHT_MODULE` 指向 desktop 中安装的 Playwright。

安装 Gitleaks 与 cargo-audit 并准备 Chromium 后，可用 `bash scripts/check-release.sh` 运行本地发布检查。可通过 `GITLEAKS_BINARY`、`CARGO_AUDIT_BINARY` 指定工具路径；原生模型下载不稳定时设置 `RNNOISE_MODEL_PATH`。安全扫描默认不忽略漏洞编号，维护状态提示需要单独评估。该脚本不发布安装包、不使用官方签名私钥，也不改变仓库可见性。

`cargo audit` 当前保留的提示是上游停止维护状态，主要来自音频包装、旧 Slint 参考界面及平台托盘依赖；没有通过忽略规则隐藏它们。Go 模块扫描可能提示 `x/crypto/openpgp` 已停止维护，但本项目没有导入该包，符号和包级扫描均无命中。每次更新依赖仍需复查，不能将此次结果当作后续版本的豁免。

具体构建命令见 [中文版 README](../README.zh-CN.md#检查与打包)，管理员发布流程见[签名更新说明](automatic-updates-2026-09-08.md)。

## 示例与私人数据

文档中的 `voice.example.com`、`203.0.113.0/24` 与 `198.51.100.0/24` 地址仅用于演示，不能用于连接作者的私人服务器。请使用自己的部署地址和身份指纹。示例默认服务器密码公开可见，部署前必须按使用场景修改。

不要提交实际部署配置、服务器或客户端身份私钥、云服务凭据、自动更新私钥、聊天/档案数据和未脱敏的日志。`.gitignore` 已覆盖常用本地配置与数据路径；自定义路径仍需自行排除。公开下载地址、更新验签公钥和示例数据可以保留。

历史技术文档保留当时的测试结论，不等于当前所有平台都已完成实机验证。历史脱敏会改变提交哈希，旧文档内的提交号只能作为当时记录。

## 公开仓库

本仓库以已审查的源码快照作为首个提交。

## 2026-10-24 修复验证

- 133 个可达历史提交及待发布工作区的 Gitleaks 扫描无泄漏命中。
- npm 干净安装、生产构建和漏洞审计通过；前端 76 项测试、后台五组隔离浏览器流程通过。
- Rust 核心 318 项、解析器安全回归 4 项、Tauri 原生层 41 项通过；两套应用锁文件的漏洞和内存安全告警均已修复，仍保留上游维护状态提示。
- Linux GLib 指针回移在现有 Debian 构建镜像的优化模式下通过回归。五分钟模拟音频压力测试通过；外部真实语音 FEC 矩阵需要 `EDENVOICE_SPEECH_PCM`，未提供素材时不计为通过。
- 服务端与 COS 工具测试、静态检查和调用链漏洞检查通过。amd64/arm64 测试发行包包含三份许可文件及对应源码；客户端源码归档的三个 Cargo 工程可离线解析，并保留两类降噪所需模型。
- 真实浏览器与 Go 服务端的 WebRTC 视频/音频、声音开关、隐私暂停、晚加入观看者、恢复及资源释放通过。像素断言兼容 VP8 管线的标准/扩展色域表示，仍拒绝原始彩色画面和未解码黑帧。
- 完整 Dockerfile 因公共基础镜像下载超时未完成构建；使用同一静态服务器、许可和对应源码生成的离线运行镜像可启动。没有将该结果表述为完整 Docker 构建通过。
- Windows/macOS/Linux 原生采集授权、Windows 系统音频回声以及跨设备实机验收仍按 README 的平台说明进行；浏览器合成媒体测试不能替代它们。


参考：[Slint 的许可选项](https://github.com/slint-ui/slint/blob/v1.17.1/LICENSE.md)、[GitHub 历史清理说明](https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/removing-sensitive-data-from-a-repository)。
